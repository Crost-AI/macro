//! Direct Rust `turso_core::IO`/`File` feasibility adapter for worker-local OPFS.
//!
//! JavaScript OPFS handles never enter a `Send + Sync` Rust value. They live in
//! a worker-thread-local, owner/session-scoped registry and are addressed by
//! monotonically allocated numeric IDs. The Turso trait objects contain only
//! owner, session, and handle IDs. This is a standalone spike, not production
//! cache code.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

use js_sys::{Function, Reflect};
use std::{
    cell::RefCell,
    collections::HashMap,
    fmt::Write as _,
    io::ErrorKind,
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use turso_core::{
    io::{FileId, FileSyncType},
    Buffer, Clock, Completion, CompletionError, Connection, Database, File, LimboError,
    MonotonicInstant, OpenFlags, OpenOptions, Result as CoreResult, Row, SqliteDialect, Statement,
    StepResult, Value, WallClockInstant, IO,
};
use wasm_bindgen::{prelude::*, JsCast};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    DedicatedWorkerGlobalScope, DomException, File as WebFile, FileSystemDirectoryHandle,
    FileSystemFileHandle, FileSystemGetDirectoryOptions, FileSystemGetFileOptions,
    FileSystemReadWriteOptions, FileSystemSyncAccessHandle,
};

const DATABASE_PATH: &str = "graphql-cache.db";
const DATABASE_WAL_PATH: &str = "graphql-cache.db-wal";
const TRANSACTION_PROBE_PATH: &str = "transaction-modes.db";
const TRANSACTION_PROBE_WAL_PATH: &str = "transaction-modes.db-wal";
const DIRECT_PROBE_PATH: &str = "direct-file.bin";
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = performance, js_name = now)]
    fn performance_now() -> f64;

    #[wasm_bindgen(js_namespace = globalThis, js_name = __tursoOpfsKillProgress)]
    fn report_kill_progress(commit_count: u32, finite_bound: u32, main_size: f64, wal_size: f64);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct OwnerId(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SessionId(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CloseToken(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct HandleId(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionKind {
    Database,
    DirectProbe,
    BeginProbe,
}

impl SessionKind {
    fn paths(self) -> &'static [PathSpec] {
        match self {
            Self::Database => &DATABASE_PATHS,
            Self::DirectProbe => &DIRECT_PATHS,
            Self::BeginProbe => &BEGIN_PATHS,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Database => "database",
            Self::DirectProbe => "direct-probe",
            Self::BeginProbe => "begin-probe",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PathSpec {
    path: &'static str,
    direct: bool,
    allow_no_lock: bool,
}

const DATABASE_PATHS: [PathSpec; 2] = [
    PathSpec {
        path: DATABASE_PATH,
        direct: true,
        allow_no_lock: false,
    },
    PathSpec {
        path: DATABASE_WAL_PATH,
        direct: false,
        allow_no_lock: true,
    },
];
const DIRECT_PATHS: [PathSpec; 1] = [PathSpec {
    path: DIRECT_PROBE_PATH,
    direct: true,
    allow_no_lock: false,
}];
const BEGIN_PATHS: [PathSpec; 2] = [
    PathSpec {
        path: TRANSACTION_PROBE_PATH,
        direct: true,
        allow_no_lock: false,
    },
    PathSpec {
        path: TRANSACTION_PROBE_WAL_PATH,
        direct: false,
        allow_no_lock: true,
    },
];

type ClosedResetState = (SessionKind, Vec<(&'static str, u64)>, bool);

struct RegisteredHandle {
    path: &'static str,
    handle: FileSystemSyncAccessHandle,
}

#[derive(Debug, Default)]
struct FaultPlan {
    max_write_chunk: Option<usize>,
    next_write_error: Option<CompletionError>,
    fail_next_close: bool,
}

#[derive(Debug, Default)]
enum Lifecycle {
    #[default]
    Unowned,
    Idle {
        owner: OwnerId,
    },
    Opening {
        owner: OwnerId,
        session: SessionId,
        kind: SessionKind,
    },
    Active {
        owner: OwnerId,
        session: SessionId,
        kind: SessionKind,
        operation_active: bool,
    },
    Closing {
        owner: OwnerId,
        session: SessionId,
        kind: SessionKind,
    },
    Closed {
        owner: OwnerId,
        token: CloseToken,
        kind: SessionKind,
        sizes: Vec<(&'static str, u64)>,
        inject_recreation_conflict: bool,
    },
    Resetting {
        owner: OwnerId,
        token: CloseToken,
        kind: SessionKind,
    },
    Poisoned {
        owner: OwnerId,
        reason: String,
    },
}

#[derive(Default)]
struct HandleRegistry {
    next_owner: u32,
    next_session: u32,
    next_close_token: u32,
    next_handle: u32,
    lifecycle: Lifecycle,
    by_id: HashMap<HandleId, RegisteredHandle>,
    by_path: HashMap<&'static str, HandleId>,
    faults: FaultPlan,
}

impl HandleRegistry {
    fn claim_owner(&mut self) -> CoreResult<OwnerId> {
        if !matches!(self.lifecycle, Lifecycle::Unowned) {
            return Err(internal_error("OPFS registry already has an owner"));
        }
        self.next_owner = checked_next(self.next_owner, "owner token")?;
        let owner = OwnerId(self.next_owner);
        self.lifecycle = Lifecycle::Idle { owner };
        Ok(owner)
    }

    fn release_owner(&mut self, owner: OwnerId) -> CoreResult<()> {
        match self.lifecycle {
            Lifecycle::Idle { owner: current } if current == owner => {
                self.lifecycle = Lifecycle::Unowned;
                Ok(())
            }
            _ => Err(internal_error(
                "owner release requires an idle matching registry",
            )),
        }
    }

    fn start_opening(&mut self, owner: OwnerId, kind: SessionKind) -> CoreResult<SessionId> {
        match self.lifecycle {
            Lifecycle::Idle { owner: current } if current == owner => {}
            Lifecycle::Poisoned { .. } => {
                return Err(internal_error("OPFS registry is poisoned"));
            }
            _ => {
                return Err(internal_error(
                    "session open requires an idle matching owner",
                ));
            }
        }
        self.next_session = checked_next(self.next_session, "session token")?;
        let session = SessionId(self.next_session);
        self.lifecycle = Lifecycle::Opening {
            owner,
            session,
            kind,
        };
        self.faults = FaultPlan::default();
        Ok(session)
    }

    fn register(
        &mut self,
        owner: OwnerId,
        session: SessionId,
        spec: PathSpec,
        handle: FileSystemSyncAccessHandle,
    ) -> CoreResult<HandleId> {
        match self.lifecycle {
            Lifecycle::Opening {
                owner: current_owner,
                session: current_session,
                kind,
            } if current_owner == owner
                && current_session == session
                && kind.paths().iter().any(|allowed| allowed.path == spec.path) => {}
            _ => {
                return Err(internal_error(
                    "handle registration is outside its opening session",
                ))
            }
        }
        if self.by_path.contains_key(spec.path) {
            return Err(internal_error("path was registered twice"));
        }
        self.next_handle = checked_next(self.next_handle, "numeric OPFS handle ID")?;
        let id = HandleId(self.next_handle);
        self.by_path.insert(spec.path, id);
        self.by_id.insert(
            id,
            RegisteredHandle {
                path: spec.path,
                handle,
            },
        );
        Ok(id)
    }

    fn activate(&mut self, owner: OwnerId, session: SessionId) -> CoreResult<SessionKind> {
        let kind = match self.lifecycle {
            Lifecycle::Opening {
                owner: current_owner,
                session: current_session,
                kind,
            } if current_owner == owner && current_session == session => kind,
            _ => return Err(internal_error("activation does not match opening session")),
        };
        if kind
            .paths()
            .iter()
            .any(|spec| !self.by_path.contains_key(spec.path))
        {
            return Err(internal_error("activation is missing an allowed path"));
        }
        self.lifecycle = Lifecycle::Active {
            owner,
            session,
            kind,
            operation_active: false,
        };
        Ok(kind)
    }

    fn active_kind(&self, owner: OwnerId, session: SessionId) -> CoreResult<SessionKind> {
        match self.lifecycle {
            Lifecycle::Active {
                owner: current_owner,
                session: current_session,
                kind,
                ..
            } if current_owner == owner && current_session == session => Ok(kind),
            Lifecycle::Poisoned { .. } => Err(internal_error("OPFS registry is poisoned")),
            _ => Err(internal_error(
                "owner/session token does not match active registry",
            )),
        }
    }

    fn validate_open(
        &self,
        owner: OwnerId,
        session: SessionId,
        path: &str,
        flags: OpenFlags,
        direct: bool,
    ) -> CoreResult<HandleId> {
        let kind = self.active_kind(owner, session)?;
        let spec = kind
            .paths()
            .iter()
            .find(|spec| spec.path == path)
            .ok_or_else(|| {
                io_error(
                    ErrorKind::PermissionDenied,
                    "path is not allowed by session",
                )
            })?;
        if direct != spec.direct {
            return Err(io_error(
                ErrorKind::InvalidInput,
                "OPFS direct flag does not match allowed path",
            ));
        }
        if flags.contains(OpenFlags::ReadOnly) {
            return Err(io_error(
                ErrorKind::PermissionDenied,
                "read-only OPFS open is not allowed by writable session",
            ));
        }
        if flags.contains(OpenFlags::NoLock) && !spec.allow_no_lock {
            return Err(io_error(
                ErrorKind::PermissionDenied,
                "NoLock is allowed only for the registered WAL path",
            ));
        }
        self.by_path
            .get(spec.path)
            .copied()
            .ok_or_else(|| io_error(ErrorKind::NotFound, "open unregistered OPFS path"))
    }

    fn begin_operation(&mut self, owner: OwnerId, session: SessionId) -> CoreResult<()> {
        match &mut self.lifecycle {
            Lifecycle::Active {
                owner: current_owner,
                session: current_session,
                operation_active,
                ..
            } if *current_owner == owner && *current_session == session => {
                if *operation_active {
                    return Err(internal_error("reentrant OPFS operation rejected"));
                }
                *operation_active = true;
                Ok(())
            }
            Lifecycle::Poisoned { .. } => Err(internal_error("OPFS registry is poisoned")),
            _ => Err(internal_error("operation does not match active session")),
        }
    }

    fn end_operation(&mut self, owner: OwnerId, session: SessionId) {
        if let Lifecycle::Active {
            owner: current_owner,
            session: current_session,
            operation_active,
            ..
        } = &mut self.lifecycle
        {
            if *current_owner == owner && *current_session == session {
                *operation_active = false;
            }
        }
    }

    fn start_closing(
        &mut self,
        owner: OwnerId,
        session: SessionId,
    ) -> CoreResult<(SessionKind, Vec<RegisteredHandle>, FaultPlan)> {
        let kind = match self.lifecycle {
            Lifecycle::Active {
                owner: current_owner,
                session: current_session,
                kind,
                operation_active: false,
            } if current_owner == owner && current_session == session => kind,
            Lifecycle::Active {
                operation_active: true,
                ..
            } => return Err(internal_error("cannot close during an active operation")),
            Lifecycle::Poisoned { .. } => return Err(internal_error("OPFS registry is poisoned")),
            _ => return Err(internal_error("close does not match active session")),
        };
        self.lifecycle = Lifecycle::Closing {
            owner,
            session,
            kind,
        };
        self.by_path.clear();
        let handles = self.by_id.drain().map(|(_, entry)| entry).collect();
        Ok((kind, handles, std::mem::take(&mut self.faults)))
    }

    fn finish_close(
        &mut self,
        owner: OwnerId,
        session: SessionId,
        kind: SessionKind,
        sizes: Vec<(&'static str, u64)>,
    ) -> CoreResult<CloseToken> {
        match self.lifecycle {
            Lifecycle::Closing {
                owner: current_owner,
                session: current_session,
                kind: current_kind,
            } if current_owner == owner && current_session == session && current_kind == kind => {}
            _ => return Err(internal_error("close completion does not match registry")),
        }
        self.next_close_token = checked_next(self.next_close_token, "close token")?;
        let token = CloseToken(self.next_close_token);
        self.lifecycle = Lifecycle::Closed {
            owner,
            token,
            kind,
            sizes,
            inject_recreation_conflict: false,
        };
        Ok(token)
    }

    fn poison(&mut self, owner: OwnerId, reason: String, retained: Vec<RegisteredHandle>) {
        self.by_id.clear();
        self.by_path.clear();
        for entry in retained {
            self.next_handle = self.next_handle.saturating_add(1);
            let id = HandleId(self.next_handle);
            self.by_path.insert(entry.path, id);
            self.by_id.insert(id, entry);
        }
        self.lifecycle = Lifecycle::Poisoned { owner, reason };
    }

    fn inject_recreation_conflict(&mut self, owner: OwnerId, token: CloseToken) -> CoreResult<()> {
        match &mut self.lifecycle {
            Lifecycle::Closed {
                owner: current_owner,
                token: current_token,
                inject_recreation_conflict,
                ..
            } if *current_owner == owner && *current_token == token => {
                *inject_recreation_conflict = true;
                Ok(())
            }
            Lifecycle::Poisoned { .. } => Err(internal_error("OPFS registry is poisoned")),
            _ => Err(internal_error(
                "recreation fault requires the matching close token",
            )),
        }
    }

    fn consume_closed_for_reset(
        &mut self,
        owner: OwnerId,
        token: CloseToken,
    ) -> CoreResult<ClosedResetState> {
        let (kind, sizes, inject_recreation_conflict) = match &self.lifecycle {
            Lifecycle::Closed {
                owner: current_owner,
                token: current_token,
                kind,
                sizes,
                inject_recreation_conflict,
            } if *current_owner == owner && *current_token == token => {
                (*kind, sizes.clone(), *inject_recreation_conflict)
            }
            Lifecycle::Poisoned { .. } => {
                return Err(internal_error("delete rejected after uncertain close"));
            }
            _ => return Err(internal_error("reset requires the matching close token")),
        };
        self.lifecycle = Lifecycle::Resetting { owner, token, kind };
        Ok((kind, sizes, inject_recreation_conflict))
    }

    fn finish_reset(&mut self, owner: OwnerId, token: CloseToken) -> CoreResult<()> {
        match self.lifecycle {
            Lifecycle::Resetting {
                owner: current_owner,
                token: current_token,
                ..
            } if current_owner == owner && current_token == token => {
                self.lifecycle = Lifecycle::Idle { owner };
                Ok(())
            }
            _ => Err(internal_error("reset completion does not match registry")),
        }
    }

    fn release_closed(&mut self, owner: OwnerId, token: CloseToken) -> CoreResult<()> {
        match self.lifecycle {
            Lifecycle::Closed {
                owner: current_owner,
                token: current_token,
                ..
            } if current_owner == owner && current_token == token => {
                self.lifecycle = Lifecycle::Idle { owner };
                Ok(())
            }
            Lifecycle::Poisoned { .. } => {
                Err(internal_error("poisoned close state cannot be released"))
            }
            _ => Err(internal_error("release requires the matching close token")),
        }
    }

    fn lifecycle_label(&self) -> String {
        match &self.lifecycle {
            Lifecycle::Unowned => "unowned".to_string(),
            Lifecycle::Idle { .. } => "idle".to_string(),
            Lifecycle::Opening { kind, .. } => format!("opening:{}", kind.label()),
            Lifecycle::Active {
                kind,
                operation_active,
                ..
            } => format!("active:{}:operation={operation_active}", kind.label()),
            Lifecycle::Closing { kind, .. } => format!("closing:{}", kind.label()),
            Lifecycle::Closed { kind, .. } => format!("closed:{}", kind.label()),
            Lifecycle::Resetting { kind, .. } => format!("resetting:{}", kind.label()),
            Lifecycle::Poisoned { owner, reason } => {
                format!("poisoned:owner={}:reason={reason}", owner.0)
            }
        }
    }
}

thread_local! {
    // The only owner of JavaScript handles. It is unreachable from another
    // worker/thread and is never wrapped in Arc/Mutex or marked Send/Sync.
    static HANDLES: RefCell<HandleRegistry> = RefCell::new(HandleRegistry::default());
}

#[derive(Debug)]
struct OperationGuard {
    owner: OwnerId,
    session: SessionId,
}

impl OperationGuard {
    fn enter(owner: OwnerId, session: SessionId) -> CoreResult<Self> {
        HANDLES.with(|registry| registry.borrow_mut().begin_operation(owner, session))?;
        Ok(Self { owner, session })
    }
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        HANDLES.with(|registry| {
            registry
                .borrow_mut()
                .end_operation(self.owner, self.session)
        });
    }
}

#[derive(Debug)]
struct OpfsFile {
    owner: OwnerId,
    session: SessionId,
    id: HandleId,
}

#[derive(Debug)]
struct OpfsIo {
    owner: OwnerId,
    session: SessionId,
    last_monotonic_nanos: AtomicU64,
}

impl OpfsIo {
    fn new(owner: OwnerId, session: SessionId) -> Self {
        Self {
            owner,
            session,
            last_monotonic_nanos: AtomicU64::new(0),
        }
    }
}

impl Clock for OpfsIo {
    fn current_time_monotonic(&self) -> MonotonicInstant {
        let observed = (performance_now().max(0.0) * 1_000_000.0) as u64;
        let mut previous = self.last_monotonic_nanos.load(Ordering::Relaxed);
        loop {
            let next = observed.max(previous.saturating_add(1));
            match self.last_monotonic_nanos.compare_exchange_weak(
                previous,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return MonotonicInstant::from_nanos(next as u128),
                Err(current) => previous = current,
            }
        }
    }

    fn current_time_wall_clock(&self) -> WallClockInstant {
        let millis = js_sys::Date::now();
        let seconds = (millis / 1_000.0).floor() as i64;
        let micros = ((millis - seconds as f64 * 1_000.0) * 1_000.0) as u32;
        WallClockInstant {
            secs: seconds,
            micros,
        }
    }
}

impl IO for OpfsIo {
    fn open_file(&self, path: &str, flags: OpenFlags, direct: bool) -> CoreResult<Arc<dyn File>> {
        let id = HANDLES.with(|registry| {
            registry
                .borrow()
                .validate_open(self.owner, self.session, path, flags, direct)
        })?;
        Ok(Arc::new(OpfsFile {
            owner: self.owner,
            session: self.session,
            id,
        }))
    }

    fn remove_file(&self, _path: &str) -> CoreResult<()> {
        Err(io_error(
            ErrorKind::Unsupported,
            "OPFS removal requires closed async reset",
        ))
    }

    fn file_id(&self, path: &str) -> CoreResult<FileId> {
        HANDLES.with(|registry| {
            let registry = registry.borrow();
            let kind = registry.active_kind(self.owner, self.session)?;
            if !kind.paths().iter().any(|spec| spec.path == path) {
                return Err(io_error(
                    ErrorKind::PermissionDenied,
                    "file_id path is not allowed by session",
                ));
            }
            Ok(FileId::from_path_hash(path))
        })
    }

    fn supports_shared_wal_coordination(&self) -> bool {
        false
    }
}

impl File for OpfsFile {
    fn lock_file(&self, _exclusive: bool) -> CoreResult<()> {
        validate_file_session(self.owner, self.session)?;
        Ok(())
    }

    fn unlock_file(&self) -> CoreResult<()> {
        validate_file_session(self.owner, self.session)?;
        Ok(())
    }

    fn pread(&self, pos: u64, completion: Completion) -> CoreResult<Completion> {
        let result = read_once(
            self.owner,
            self.session,
            self.id,
            pos,
            completion.as_read().buf(),
        );
        finish_completion(&completion, result);
        Ok(completion)
    }

    fn pwrite(
        &self,
        pos: u64,
        buffer: Arc<Buffer>,
        completion: Completion,
    ) -> CoreResult<Completion> {
        let result = write_all_with(pos, buffer.as_slice(), |offset, bytes| {
            write_once(self.owner, self.session, self.id, offset, bytes)
        })
        .and_then(completion_write_len)
        .map_err(LimboError::CompletionError);
        finish_completion(&completion, result);
        Ok(completion)
    }

    fn pwritev(
        &self,
        mut pos: u64,
        buffers: Vec<Arc<Buffer>>,
        completion: Completion,
    ) -> CoreResult<Completion> {
        let result = (|| {
            let mut total = 0_usize;
            for buffer in buffers {
                let written = write_all_with(pos, buffer.as_slice(), |offset, bytes| {
                    write_once(self.owner, self.session, self.id, offset, bytes)
                })?;
                total = total
                    .checked_add(written)
                    .ok_or(CompletionError::ShortWrite)?;
                pos = pos
                    .checked_add(written as u64)
                    .ok_or(CompletionError::ShortWrite)?;
            }
            completion_write_len(total)
        })()
        .map_err(LimboError::CompletionError);
        finish_completion(&completion, result);
        Ok(completion)
    }

    fn sync(&self, completion: Completion, _sync_type: FileSyncType) -> CoreResult<Completion> {
        let result = file_handle(self.owner, self.session, self.id).and_then(|handle| {
            handle
                .flush()
                .map_err(|_| io_error(ErrorKind::Other, "OPFS flush"))?;
            Ok(0)
        });
        finish_completion(&completion, result);
        Ok(completion)
    }

    fn size(&self) -> CoreResult<u64> {
        file_handle(self.owner, self.session, self.id).and_then(|handle| {
            handle
                .get_size()
                .map_err(|_| io_error(ErrorKind::Other, "OPFS getSize"))
                .and_then(number_to_u64)
        })
    }

    fn truncate(&self, len: u64, completion: Completion) -> CoreResult<Completion> {
        let result = file_handle(self.owner, self.session, self.id).and_then(|handle| {
            validate_position(len)?;
            handle
                .truncate_with_f64(len as f64)
                .map_err(|_| io_error(ErrorKind::Other, "OPFS truncate"))?;
            Ok(0)
        });
        finish_completion(&completion, result);
        Ok(completion)
    }
}

// A zero-length aggregate is a successful no-op reported as `0`; it never
// touches or validates the otherwise-unused offset. Any aggregate that cannot
// fit Turso's signed completion count is `ShortWrite`.
fn completion_write_len(written: usize) -> Result<i32, CompletionError> {
    i32::try_from(written).map_err(|_| CompletionError::ShortWrite)
}

fn write_all_with(
    mut pos: u64,
    mut bytes: &[u8],
    mut write: impl FnMut(u64, &[u8]) -> Result<usize, CompletionError>,
) -> Result<usize, CompletionError> {
    let expected = bytes.len();
    let mut total = 0_usize;
    while !bytes.is_empty() {
        let written = write(pos, bytes)?;
        if written == 0 || written > bytes.len() {
            return Err(CompletionError::ShortWrite);
        }
        total = total
            .checked_add(written)
            .ok_or(CompletionError::ShortWrite)?;
        pos = pos
            .checked_add(written as u64)
            .ok_or(CompletionError::ShortWrite)?;
        bytes = &bytes[written..];
    }
    if total == expected {
        Ok(total)
    } else {
        Err(CompletionError::ShortWrite)
    }
}

fn write_once(
    owner: OwnerId,
    session: SessionId,
    id: HandleId,
    pos: u64,
    bytes: &[u8],
) -> Result<usize, CompletionError> {
    validate_position(pos).map_err(limbo_to_completion)?;
    let (handle, chunk_limit, injected_error) = HANDLES
        .with(|registry| {
            let mut registry = registry.borrow_mut();
            registry.active_kind(owner, session)?;
            let handle = registry
                .by_id
                .get(&id)
                .ok_or_else(|| io_error(ErrorKind::NotFound, "stale OPFS handle ID"))?
                .handle
                .clone();
            Ok::<_, LimboError>((
                handle,
                registry.faults.max_write_chunk,
                registry.faults.next_write_error.take(),
            ))
        })
        .map_err(limbo_to_completion)?;
    if let Some(error) = injected_error {
        return Err(error);
    }
    let chunk_len = chunk_limit.unwrap_or(bytes.len()).min(bytes.len());
    if chunk_len == 0 {
        return Ok(0);
    }
    let options = FileSystemReadWriteOptions::new();
    options.set_at(pos as f64);
    let written = handle
        .write_with_u8_array_and_options(&bytes[..chunk_len], &options)
        .map_err(|_| CompletionError::IOError(ErrorKind::Other, "OPFS write"))?;
    let written = number_to_i32(written)
        .map(|value| value as usize)
        .map_err(limbo_to_completion)?;
    if written <= chunk_len {
        Ok(written)
    } else {
        Err(CompletionError::ShortWrite)
    }
}

fn read_once(
    owner: OwnerId,
    session: SessionId,
    id: HandleId,
    pos: u64,
    buffer: &Buffer,
) -> CoreResult<i32> {
    validate_position(pos)?;
    let handle = file_handle(owner, session, id)?;
    let options = FileSystemReadWriteOptions::new();
    options.set_at(pos as f64);
    handle
        .read_with_u8_array_and_options(buffer.as_mut_slice(), &options)
        .map_err(|_| io_error(ErrorKind::Other, "OPFS read"))
        .and_then(number_to_i32)
}

fn validate_file_session(owner: OwnerId, session: SessionId) -> CoreResult<()> {
    HANDLES.with(|registry| registry.borrow().active_kind(owner, session).map(|_| ()))
}

fn file_handle(
    owner: OwnerId,
    session: SessionId,
    id: HandleId,
) -> CoreResult<FileSystemSyncAccessHandle> {
    HANDLES.with(|registry| {
        let registry = registry.borrow();
        registry.active_kind(owner, session)?;
        registry
            .by_id
            .get(&id)
            .map(|entry| entry.handle.clone())
            .ok_or_else(|| io_error(ErrorKind::NotFound, "stale OPFS handle ID"))
    })
}

fn finish_completion(completion: &Completion, result: CoreResult<i32>) {
    match result {
        Ok(bytes) => completion.complete(bytes),
        Err(error) => completion.error(limbo_to_completion(error)),
    }
}

fn limbo_to_completion(error: LimboError) -> CompletionError {
    match error {
        LimboError::CompletionError(error) => error,
        _ => CompletionError::IOError(ErrorKind::Other, "OPFS adapter"),
    }
}

fn validate_position(value: u64) -> CoreResult<()> {
    if value <= MAX_SAFE_INTEGER {
        Ok(())
    } else {
        Err(io_error(
            ErrorKind::InvalidInput,
            "OPFS offset exceeds JavaScript safe integer",
        ))
    }
}

fn number_to_i32(value: f64) -> CoreResult<i32> {
    if value.is_finite() && value >= 0.0 && value.fract() == 0.0 && value <= i32::MAX as f64 {
        Ok(value as i32)
    } else {
        Err(io_error(ErrorKind::InvalidData, "invalid OPFS byte count"))
    }
}

fn number_to_u64(value: f64) -> CoreResult<u64> {
    if value.is_finite() && value >= 0.0 && value.fract() == 0.0 && value <= MAX_SAFE_INTEGER as f64
    {
        Ok(value as u64)
    } else {
        Err(io_error(ErrorKind::InvalidData, "invalid OPFS file size"))
    }
}

fn close_sync_handle(handle: &FileSystemSyncAccessHandle) -> Result<(), JsValue> {
    let close =
        Reflect::get(handle.as_ref(), &JsValue::from_str("close"))?.dyn_into::<Function>()?;
    close.call0(handle.as_ref()).map(|_| ())
}

fn checked_next(value: u32, label: &str) -> CoreResult<u32> {
    value
        .checked_add(1)
        .ok_or_else(|| internal_error(&format!("{label} space exhausted")))
}

fn internal_error(message: &str) -> LimboError {
    LimboError::InternalError(message.to_string())
}

fn io_error(kind: ErrorKind, operation: &'static str) -> LimboError {
    CompletionError::IOError(kind, operation).into()
}

fn js_error(value: &JsValue) -> String {
    value
        .as_string()
        .or_else(|| {
            value
                .dyn_ref::<js_sys::Error>()
                .map(|error| error.message().into())
        })
        .unwrap_or_else(|| format!("{value:?}"))
}

async fn async_worker_root() -> Result<FileSystemDirectoryHandle, JsValue> {
    let worker = js_sys::global().dyn_into::<DedicatedWorkerGlobalScope>()?;
    JsFuture::from(worker.navigator().storage().get_directory())
        .await?
        .dyn_into::<FileSystemDirectoryHandle>()
}

async fn open_sync_handle(path: &str) -> Result<FileSystemSyncAccessHandle, JsValue> {
    let root = async_worker_root().await?;
    let options = FileSystemGetFileOptions::new();
    options.set_create(true);
    let file = JsFuture::from(root.get_file_handle_with_options(path, &options))
        .await?
        .dyn_into::<FileSystemFileHandle>()?;
    JsFuture::from(file.create_sync_access_handle())
        .await?
        .dyn_into::<FileSystemSyncAccessHandle>()
}

async fn begin_session(owner: OwnerId, kind: SessionKind) -> Result<SessionId, JsValue> {
    let session = HANDLES
        .with(|registry| registry.borrow_mut().start_opening(owner, kind))
        .map_err(core_error_to_js)?;
    for spec in kind.paths() {
        let opened = open_sync_handle(spec.path).await;
        match opened {
            Ok(handle) => {
                let rejected_handle = handle.clone();
                if let Err(error) = HANDLES.with(|registry| {
                    registry
                        .borrow_mut()
                        .register(owner, session, *spec, handle)
                }) {
                    let (rejected_close, retained) = match close_sync_handle(&rejected_handle) {
                        Ok(()) => (String::new(), Vec::new()),
                        Err(close_error) => (
                            format!("; rejected handle close failed: {}", js_error(&close_error)),
                            vec![RegisteredHandle {
                                path: spec.path,
                                handle: rejected_handle,
                            }],
                        ),
                    };
                    abort_opening(owner, &format!("{}{rejected_close}", error), retained);
                    return Err(core_error_to_js(error));
                }
            }
            Err(error) => {
                abort_opening(owner, &js_error(&error), Vec::new());
                return Err(error);
            }
        }
    }
    HANDLES
        .with(|registry| registry.borrow_mut().activate(owner, session))
        .map_err(core_error_to_js)?;
    Ok(session)
}

fn abort_opening(owner: OwnerId, cause: &str, mut failed: Vec<RegisteredHandle>) {
    let handles = HANDLES.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry.by_path.clear();
        registry
            .by_id
            .drain()
            .map(|(_, entry)| entry)
            .collect::<Vec<_>>()
    });
    let mut close_errors = Vec::new();
    for entry in handles {
        match close_sync_handle(&entry.handle) {
            Ok(()) => {}
            Err(error) => {
                close_errors.push(format!("{}: {}", entry.path, js_error(&error)));
                failed.push(entry);
            }
        }
    }
    HANDLES.with(|registry| {
        let mut registry = registry.borrow_mut();
        if failed.is_empty() {
            registry.lifecycle = Lifecycle::Idle { owner };
        } else {
            registry.poison(
                owner,
                format!(
                    "open failed: {cause}; cleanup failed: {}",
                    close_errors.join("; ")
                ),
                failed,
            );
        }
    });
}

fn close_session_internal(owner: OwnerId, session: SessionId) -> Result<String, JsValue> {
    let (kind, handles, mut faults) = HANDLES
        .with(|registry| registry.borrow_mut().start_closing(owner, session))
        .map_err(core_error_to_js)?;
    let mut sizes = Vec::new();
    let mut failures = Vec::new();
    let mut errors = Vec::new();
    for entry in handles {
        match entry
            .handle
            .get_size()
            .map_err(|_| JsValue::from_str("getSize failed during close"))
            .and_then(|value| number_to_u64(value).map_err(core_error_to_js))
        {
            Ok(size) => sizes.push((entry.path, size)),
            Err(error) => errors.push(format!("{} size: {}", entry.path, js_error(&error))),
        }
        let close_result = if faults.fail_next_close {
            faults.fail_next_close = false;
            Err(JsValue::from_str("injected close failure"))
        } else {
            close_sync_handle(&entry.handle)
        };
        if let Err(error) = close_result {
            errors.push(format!("{} close: {}", entry.path, js_error(&error)));
            failures.push(entry);
        }
    }
    if !errors.is_empty() {
        let reason = format!("uncertain close: {}", errors.join("; "));
        HANDLES.with(|registry| {
            registry
                .borrow_mut()
                .poison(owner, reason.clone(), failures)
        });
        return Err(JsValue::from_str(&reason));
    }
    let token = HANDLES
        .with(|registry| {
            registry
                .borrow_mut()
                .finish_close(owner, session, kind, sizes.clone())
        })
        .map_err(core_error_to_js)?;
    Ok(close_summary_json(token, kind, &sizes))
}

async fn reset_closed_session(owner: OwnerId, token: CloseToken) -> Result<String, JsValue> {
    let (kind, before_sizes, inject_recreation_conflict) = HANDLES
        .with(|registry| registry.borrow_mut().consume_closed_for_reset(owner, token))
        .map_err(core_error_to_js)?;
    let reset_result = reset_paths(kind, inject_recreation_conflict).await;
    match reset_result {
        Ok((deleted, recreated)) => {
            HANDLES
                .with(|registry| registry.borrow_mut().finish_reset(owner, token))
                .map_err(core_error_to_js)?;
            Ok(reset_summary_json(
                kind,
                &before_sizes,
                &deleted,
                &recreated,
            ))
        }
        Err(error) => {
            let reason = format!("reset failed after close: {}", js_error(&error));
            HANDLES.with(|registry| {
                registry
                    .borrow_mut()
                    .poison(owner, reason.clone(), Vec::new())
            });
            Err(JsValue::from_str(&reason))
        }
    }
}

async fn reset_paths(
    kind: SessionKind,
    inject_recreation_conflict: bool,
) -> Result<(Vec<(&'static str, bool)>, Vec<(&'static str, u64)>), JsValue> {
    let root = async_worker_root().await?;
    let mut deleted = Vec::new();
    for spec in kind.paths() {
        deleted.push((spec.path, remove_if_present(&root, spec.path).await?));
    }
    if inject_recreation_conflict {
        let path = kind
            .paths()
            .first()
            .ok_or_else(|| JsValue::from_str("reset kind has no paths"))?
            .path;
        let options = FileSystemGetDirectoryOptions::new();
        options.set_create(true);
        // Create an actual browser-side directory conflict after successful
        // deletion. The following real getFileHandle call must reject rather
        // than relying on a synthetic early-return failure.
        JsFuture::from(root.get_directory_handle_with_options(path, &options)).await?;
    }
    let mut recreated = Vec::new();
    for spec in kind.paths() {
        let options = FileSystemGetFileOptions::new();
        options.set_create(true);
        let file_handle = JsFuture::from(root.get_file_handle_with_options(spec.path, &options))
            .await?
            .dyn_into::<FileSystemFileHandle>()?;
        let file = JsFuture::from(file_handle.get_file())
            .await?
            .dyn_into::<WebFile>()?;
        recreated.push((
            spec.path,
            number_to_u64(file.size()).map_err(core_error_to_js)?,
        ));
    }
    Ok((deleted, recreated))
}

async fn remove_if_present(root: &FileSystemDirectoryHandle, path: &str) -> Result<bool, JsValue> {
    match JsFuture::from(root.remove_entry(path)).await {
        Ok(_) => Ok(true),
        Err(error) if is_not_found(&error) => Ok(false),
        Err(error) => Err(error),
    }
}

fn is_not_found(value: &JsValue) -> bool {
    value
        .dyn_ref::<DomException>()
        .is_some_and(|error| error.name() == "NotFoundError")
        || Reflect::get(value, &JsValue::from_str("name"))
            .ok()
            .and_then(|name| name.as_string())
            .is_some_and(|name| name == "NotFoundError")
}

fn close_summary_json(
    token: CloseToken,
    kind: SessionKind,
    sizes: &[(&'static str, u64)],
) -> String {
    format!(
        "{{\"close_token\":{},\"kind\":\"{}\",\"sizes\":{}}}",
        token.0,
        kind.label(),
        path_u64_json(sizes)
    )
}

fn reset_summary_json(
    kind: SessionKind,
    before: &[(&'static str, u64)],
    deleted: &[(&'static str, bool)],
    recreated: &[(&'static str, u64)],
) -> String {
    format!(
        "{{\"kind\":\"{}\",\"before\":{},\"deleted\":{},\"recreated\":{}}}",
        kind.label(),
        path_u64_json(before),
        path_bool_json(deleted),
        path_u64_json(recreated)
    )
}

fn path_u64_json(values: &[(&'static str, u64)]) -> String {
    format!(
        "{{{}}}",
        values
            .iter()
            .map(|(path, value)| format!("\"{path}\":{value}"))
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn path_bool_json(values: &[(&'static str, bool)]) -> String {
    format!(
        "{{{}}}",
        values
            .iter()
            .map(|(path, value)| format!("\"{path}\":{value}"))
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// Claim the worker-local registry after the worker acquires its exclusive Web Lock.
#[wasm_bindgen]
pub fn claim_owner() -> Result<u32, JsValue> {
    HANDLES
        .with(|registry| registry.borrow_mut().claim_owner())
        .map(|owner| owner.0)
        .map_err(core_error_to_js)
}

/// Release an idle worker-local registry before releasing the Web Lock.
#[wasm_bindgen]
pub fn release_owner(owner: u32) -> Result<(), JsValue> {
    HANDLES
        .with(|registry| registry.borrow_mut().release_owner(OwnerId(owner)))
        .map_err(core_error_to_js)
}

/// Asynchronously create an owner-scoped database session and pre-open main/WAL.
#[wasm_bindgen]
pub async fn begin_database_session(owner: u32) -> Result<String, JsValue> {
    let session = begin_session(OwnerId(owner), SessionKind::Database).await?;
    Ok(format!("{{\"session\":{}}}", session.0))
}

/// Asynchronously create an owner-scoped direct-file probe session.
#[wasm_bindgen]
pub async fn begin_direct_probe_session(owner: u32) -> Result<String, JsValue> {
    let session = begin_session(OwnerId(owner), SessionKind::DirectProbe).await?;
    Ok(format!("{{\"session\":{}}}", session.0))
}

/// Asynchronously create an owner-scoped transaction-mode probe session.
#[wasm_bindgen]
pub async fn begin_transaction_probe_session(owner: u32) -> Result<String, JsValue> {
    let session = begin_session(OwnerId(owner), SessionKind::BeginProbe).await?;
    Ok(format!("{{\"session\":{}}}", session.0))
}

/// Close a matching active session and return the consuming close token and pre-close sizes.
#[wasm_bindgen]
pub fn close_session(owner: u32, session: u32) -> Result<String, JsValue> {
    close_session_internal(OwnerId(owner), SessionId(session))
}

/// Consume a successful close token without deleting files, returning the owner to idle.
#[wasm_bindgen]
pub fn release_closed_session(owner: u32, close_token: u32) -> Result<(), JsValue> {
    HANDLES
        .with(|registry| {
            registry
                .borrow_mut()
                .release_closed(OwnerId(owner), CloseToken(close_token))
        })
        .map_err(core_error_to_js)
}

/// Consume a successful close token, then asynchronously delete and recreate its allowed paths.
#[wasm_bindgen]
pub async fn reset_closed_session_paths(owner: u32, close_token: u32) -> Result<String, JsValue> {
    reset_closed_session(OwnerId(owner), CloseToken(close_token)).await
}

/// Inject a directory conflict after deletion so actual recreation must fail.
#[wasm_bindgen]
pub fn inject_next_recreation_conflict(owner: u32, close_token: u32) -> Result<(), JsValue> {
    HANDLES
        .with(|registry| {
            registry
                .borrow_mut()
                .inject_recreation_conflict(OwnerId(owner), CloseToken(close_token))
        })
        .map_err(core_error_to_js)
}

/// Report the owner/session-scoped registry lifecycle for failure assertions.
#[wasm_bindgen]
pub fn registry_lifecycle() -> String {
    HANDLES.with(|registry| registry.borrow().lifecycle_label())
}

/// Inject one close failure into the matching active session.
#[wasm_bindgen]
pub fn inject_next_close_failure(owner: u32, session: u32) -> Result<(), JsValue> {
    HANDLES
        .with(|registry| {
            let mut registry = registry.borrow_mut();
            registry.active_kind(OwnerId(owner), SessionId(session))?;
            registry.faults.fail_next_close = true;
            Ok(())
        })
        .map_err(core_error_to_js)
}

/// Return current registered sizes for a matching active session.
#[wasm_bindgen]
pub fn active_session_sizes(owner: u32, session: u32) -> Result<String, JsValue> {
    let owner = OwnerId(owner);
    let session = SessionId(session);
    let kind = HANDLES
        .with(|registry| registry.borrow().active_kind(owner, session))
        .map_err(core_error_to_js)?;
    let mut sizes = Vec::new();
    for spec in kind.paths() {
        let id = HANDLES
            .with(|registry| registry.borrow().by_path.get(spec.path).copied())
            .ok_or_else(|| JsValue::from_str("registered path has no handle"))?;
        let size = OpfsFile { owner, session, id }
            .size()
            .map_err(core_error_to_js)?;
        sizes.push((spec.path, size));
    }
    Ok(path_u64_json(&sizes))
}

/// Exercise direct file operations, exact callbacks, EOF/short reads, and injected failures.
#[wasm_bindgen]
pub fn run_direct_file_probe(owner: u32, session: u32) -> Result<String, JsValue> {
    let owner = OwnerId(owner);
    let session = SessionId(session);
    let _operation = OperationGuard::enter(owner, session).map_err(core_error_to_js)?;
    let io = OpfsIo::new(owner, session);
    let file = io
        .open_file(DIRECT_PROBE_PATH, OpenFlags::Create, true)
        .map_err(core_error_to_js)?;

    let (empty, empty_count, empty_result) = tracked_write_completion();
    let empty = file
        .pwrite(0, Arc::new(Buffer::new(Vec::new())), empty)
        .map_err(core_error_to_js)?;
    require_completion(&empty, &empty_count, &empty_result, Ok(0))?;

    let (write, write_count, write_result) = tracked_write_completion();
    let write = file
        .pwrite(0, Arc::new(Buffer::new(b"abcdef".to_vec())), write)
        .map_err(core_error_to_js)?;
    require_completion(&write, &write_count, &write_result, Ok(6))?;

    set_write_chunk_fault(owner, session, Some(2)).map_err(core_error_to_js)?;
    let (partial, partial_count, partial_result) = tracked_write_completion();
    let partial = file
        .pwrite(6, Arc::new(Buffer::new(b"1234".to_vec())), partial)
        .map_err(core_error_to_js)?;
    require_completion(&partial, &partial_count, &partial_result, Ok(4))?;
    set_write_chunk_fault(owner, session, None).map_err(core_error_to_js)?;

    let (sync, sync_count, sync_result) = tracked_sync_completion();
    let sync = file
        .sync(sync, FileSyncType::Fsync)
        .map_err(core_error_to_js)?;
    require_completion(&sync, &sync_count, &sync_result, Ok(0))?;

    let read_buffer = Arc::new(Buffer::new(vec![0; 6]));
    let (read, read_count, read_result) = tracked_read_completion(read_buffer.clone(), None);
    let read = file.pread(2, read).map_err(core_error_to_js)?;
    require_completion(&read, &read_count, &read_result, Ok(6))?;
    require(
        read_buffer.as_slice() == b"cdef12",
        "direct pread bytes mismatch",
    )
    .map_err(core_error_to_js)?;

    let (truncate, truncate_count, truncate_result) = tracked_truncate_completion();
    let truncate = file.truncate(5, truncate).map_err(core_error_to_js)?;
    require_completion(&truncate, &truncate_count, &truncate_result, Ok(0))?;

    let short_buffer = Arc::new(Buffer::new(vec![0; 4]));
    let (short, short_count, short_result) = tracked_read_completion(short_buffer.clone(), None);
    let short = file.pread(3, short).map_err(core_error_to_js)?;
    require_completion(&short, &short_count, &short_result, Ok(2))?;
    require(
        &short_buffer.as_slice()[..2] == b"de",
        "short read bytes mismatch",
    )
    .map_err(core_error_to_js)?;

    let eof_buffer = Arc::new(Buffer::new(vec![0; 4]));
    let (eof, eof_count, eof_result) = tracked_read_completion(eof_buffer, None);
    let eof = file.pread(99, eof).map_err(core_error_to_js)?;
    require_completion(&eof, &eof_count, &eof_result, Ok(0))?;

    let detected_buffer = Arc::new(Buffer::new(vec![0; 4]));
    let expected_short = CompletionError::ShortRead {
        page_idx: 0,
        expected: 4,
        actual: 2,
    };
    let (detected, detected_count, _detected_result) =
        tracked_read_completion(detected_buffer, Some(expected_short));
    let detected = file.pread(3, detected).map_err(core_error_to_js)?;
    require(
        detected_count.load(Ordering::SeqCst) == 1,
        "short-read callback count",
    )
    .map_err(core_error_to_js)?;
    require(
        detected.get_error() == Some(expected_short),
        "short-read error lost",
    )
    .map_err(core_error_to_js)?;

    set_write_chunk_fault(owner, session, Some(0)).map_err(core_error_to_js)?;
    let (zero, zero_count, zero_result) = tracked_write_completion();
    let zero = file
        .pwrite(20, Arc::new(Buffer::new(b"x".to_vec())), zero)
        .map_err(core_error_to_js)?;
    require_completion(
        &zero,
        &zero_count,
        &zero_result,
        Err(CompletionError::ShortWrite),
    )?;
    set_write_chunk_fault(owner, session, None).map_err(core_error_to_js)?;

    let injected_error = CompletionError::IOError(ErrorKind::Other, "injected OPFS write failure");
    set_next_write_error(owner, session, injected_error).map_err(core_error_to_js)?;
    let (failed, failed_count, failed_result) = tracked_write_completion();
    let failed = file
        .pwrite(20, Arc::new(Buffer::new(b"x".to_vec())), failed)
        .map_err(core_error_to_js)?;
    require_completion(&failed, &failed_count, &failed_result, Err(injected_error))?;

    let quota_error = CompletionError::IOError(ErrorKind::StorageFull, "injected OPFS quota");
    set_next_write_error(owner, session, quota_error).map_err(core_error_to_js)?;
    let (quota, quota_count, quota_result) = tracked_write_completion();
    let quota = file
        .pwrite(20, Arc::new(Buffer::new(b"x".to_vec())), quota)
        .map_err(core_error_to_js)?;
    require_completion(&quota, &quota_count, &quota_result, Err(quota_error))?;

    Ok(format!(
        "{{\"empty_write_callbacks\":{},\"empty_write_bytes\":0,\"write_callbacks\":{},\"partial_write_callbacks\":{},\"partial_write_retried\":true,\"read_callbacks\":{},\"short_read_callbacks\":{},\"short_read_bytes\":2,\"eof_callbacks\":{},\"eof_bytes\":0,\"detected_short_read_callbacks\":{},\"zero_write_callbacks\":{},\"zero_write_error\":\"ShortWrite\",\"error_write_callbacks\":{},\"error_preserved\":true,\"quota_write_callbacks\":{},\"quota_preserved\":true,\"size_after\":{}}}",
        empty_count.load(Ordering::SeqCst),
        write_count.load(Ordering::SeqCst),
        partial_count.load(Ordering::SeqCst),
        read_count.load(Ordering::SeqCst),
        short_count.load(Ordering::SeqCst),
        eof_count.load(Ordering::SeqCst),
        detected_count.load(Ordering::SeqCst),
        zero_count.load(Ordering::SeqCst),
        failed_count.load(Ordering::SeqCst),
        quota_count.load(Ordering::SeqCst),
        file.size().map_err(core_error_to_js)?,
    ))
}

type TrackedResult = Arc<Mutex<Option<Result<i32, CompletionError>>>>;
type TrackedCompletion = (Completion, Arc<AtomicUsize>, TrackedResult);

fn tracked_write_completion() -> TrackedCompletion {
    let count = Arc::new(AtomicUsize::new(0));
    let result = Arc::new(Mutex::new(None));
    let callback_count = count.clone();
    let callback_result = result.clone();
    let completion = Completion::new_write(move |value| {
        callback_count.fetch_add(1, Ordering::SeqCst);
        *callback_result.lock().expect("write callback result lock") = Some(value);
    });
    (completion, count, result)
}

fn tracked_sync_completion() -> TrackedCompletion {
    let count = Arc::new(AtomicUsize::new(0));
    let result = Arc::new(Mutex::new(None));
    let callback_count = count.clone();
    let callback_result = result.clone();
    let completion = Completion::new_sync(move |value| {
        callback_count.fetch_add(1, Ordering::SeqCst);
        *callback_result.lock().expect("sync callback result lock") = Some(value);
    });
    (completion, count, result)
}

fn tracked_truncate_completion() -> TrackedCompletion {
    let count = Arc::new(AtomicUsize::new(0));
    let result = Arc::new(Mutex::new(None));
    let callback_count = count.clone();
    let callback_result = result.clone();
    let completion = Completion::new_trunc(move |value| {
        callback_count.fetch_add(1, Ordering::SeqCst);
        *callback_result
            .lock()
            .expect("truncate callback result lock") = Some(value);
    });
    (completion, count, result)
}

fn tracked_read_completion(
    buffer: Arc<Buffer>,
    detect_error: Option<CompletionError>,
) -> TrackedCompletion {
    let count = Arc::new(AtomicUsize::new(0));
    let result = Arc::new(Mutex::new(None));
    let callback_count = count.clone();
    let callback_result = result.clone();
    let completion = Completion::new_read(buffer, move |value| {
        callback_count.fetch_add(1, Ordering::SeqCst);
        *callback_result.lock().expect("read callback result lock") = Some(
            value
                .as_ref()
                .map(|(_, bytes)| *bytes)
                .map_err(|error| *error),
        );
        detect_error
    });
    (completion, count, result)
}

fn require_completion(
    completion: &Completion,
    count: &AtomicUsize,
    observed: &Mutex<Option<Result<i32, CompletionError>>>,
    expected: Result<i32, CompletionError>,
) -> Result<(), JsValue> {
    require(
        count.load(Ordering::SeqCst) == 1,
        "completion callback count was not one",
    )
    .map_err(core_error_to_js)?;
    require(
        observed.lock().expect("completion result lock").as_ref() == Some(&expected),
        "completion callback result mismatch",
    )
    .map_err(core_error_to_js)?;
    require(
        completion.get_error() == expected.err(),
        "completion retained the wrong specific error",
    )
    .map_err(core_error_to_js)
}

fn set_write_chunk_fault(
    owner: OwnerId,
    session: SessionId,
    chunk: Option<usize>,
) -> CoreResult<()> {
    HANDLES.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry.active_kind(owner, session)?;
        registry.faults.max_write_chunk = chunk;
        Ok(())
    })
}

fn set_next_write_error(
    owner: OwnerId,
    session: SessionId,
    error: CompletionError,
) -> CoreResult<()> {
    HANDLES.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry.active_kind(owner, session)?;
        registry.faults.next_write_error = Some(error);
        Ok(())
    })
}

fn open_connection_at(
    owner: OwnerId,
    session: SessionId,
    path: &str,
) -> CoreResult<(Arc<Database>, Arc<Connection>)> {
    let io: Arc<dyn IO> = Arc::new(OpfsIo::new(owner, session));
    let database = Database::open(io, path, OpenOptions::new(Arc::new(SqliteDialect)))?;
    let connection = database.connect()?;
    Ok((database, connection))
}

fn with_database<T>(
    owner: OwnerId,
    session: SessionId,
    path: &str,
    operation: impl FnOnce(&Arc<Connection>) -> CoreResult<T>,
) -> CoreResult<T> {
    let _operation_guard = OperationGuard::enter(owner, session)?;
    let (database, connection) = open_connection_at(owner, session, path)?;
    let operation_result = operation(&connection);
    let close_result = connection.close();
    drop(connection);
    drop(database);
    match (operation_result, close_result) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
    }
}

const KILL_WRITE_BOUND: i64 = 10_000;

const PERSISTENCE_SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS persistence_probe (
  id INTEGER PRIMARY KEY,
  value TEXT NOT NULL
);
";

/// Execute real Turso SQL and persist a marker in the matching active session.
#[wasm_bindgen]
pub fn sql_write_marker(owner: u32, session: u32, value: String) -> Result<String, JsValue> {
    with_database(
        OwnerId(owner),
        SessionId(session),
        DATABASE_PATH,
        |connection| {
            connection.execute(PERSISTENCE_SCHEMA)?;
            connection.execute("BEGIN")?;
            execute_bound(
                connection,
                "INSERT INTO persistence_probe (id, value) VALUES (1, ?1) ON CONFLICT(id) DO UPDATE SET value = excluded.value",
                vec![Value::from_text(value)],
            )?;
            connection.execute("COMMIT")?;
            let journal_mode = query_string(connection, "PRAGMA journal_mode")?;
            let persisted = query_string(
                connection,
                "SELECT value FROM persistence_probe WHERE id = 1",
            )?;
            Ok(format!(
                "{{\"journal_mode\":\"{journal_mode}\",\"value\":\"{persisted}\"}}"
            ))
        },
    )
    .map_err(core_error_to_js)
}

/// Reopen Turso and read the persisted marker from the matching active session.
#[wasm_bindgen]
pub fn sql_read_marker(owner: u32, session: u32) -> Result<String, JsValue> {
    with_database(
        OwnerId(owner),
        SessionId(session),
        DATABASE_PATH,
        |connection| {
            connection.execute(PERSISTENCE_SCHEMA)?;
            let value = query_string(
                connection,
                "SELECT value FROM persistence_probe WHERE id = 1",
            )?;
            let count = query_i64(connection, "SELECT COUNT(*) FROM persistence_probe")?;
            Ok(format!("{{\"value\":\"{value}\",\"count\":{count}}}"))
        },
    )
    .map_err(core_error_to_js)
}

/// Verify a reset database is fresh, then persist a recovery marker with real SQL.
#[wasm_bindgen]
pub fn sql_verify_fresh_recovery(owner: u32, session: u32) -> Result<String, JsValue> {
    with_database(
        OwnerId(owner),
        SessionId(session),
        DATABASE_PATH,
        |connection| {
            connection.execute(PERSISTENCE_SCHEMA)?;
            let count_before = query_i64(connection, "SELECT COUNT(*) FROM persistence_probe")?;
            require(count_before == 0, "wiped database retained old SQL rows")?;
            execute_bound(
                connection,
                "INSERT INTO persistence_probe (id, value) VALUES (1, ?1)",
                vec![Value::from_text("recovered-fresh")],
            )?;
            let value = query_string(
                connection,
                "SELECT value FROM persistence_probe WHERE id = 1",
            )?;
            Ok(format!(
                "{{\"count_before\":{count_before},\"value\":\"{value}\"}}"
            ))
        },
    )
    .map_err(core_error_to_js)
}

/// Count committed kill-probe rows before reset proves the finite loop did not finish.
#[wasm_bindgen]
pub fn sql_count_kill_probe(owner: u32, session: u32) -> Result<String, JsValue> {
    with_database(
        OwnerId(owner),
        SessionId(session),
        DATABASE_PATH,
        |connection| {
            let count = query_i64(connection, "SELECT COUNT(*) FROM kill_probe")?;
            Ok(format!(
                "{{\"committed_rows\":{count},\"finite_bound\":{KILL_WRITE_BOUND}}}"
            ))
        },
    )
    .map_err(core_error_to_js)
}

/// Commit repeatedly, reporting only after the first successful commit; the worker must kill it.
#[wasm_bindgen]
pub fn run_worker_kill_write_loop(owner: u32, session: u32) -> Result<String, JsValue> {
    let owner = OwnerId(owner);
    let session = SessionId(session);
    with_database(owner, session, DATABASE_PATH, |connection| {
        connection.execute(PERSISTENCE_SCHEMA)?;
        connection.execute("PRAGMA wal_autocheckpoint = 0")?;
        connection.execute(
            "CREATE TABLE IF NOT EXISTS kill_probe (id INTEGER PRIMARY KEY, payload BLOB NOT NULL)",
        )?;
        for id in 1..=KILL_WRITE_BOUND {
            connection.execute("BEGIN")?;
            execute_bound(
                connection,
                "INSERT INTO kill_probe (id, payload) VALUES (?1, ?2) ON CONFLICT(id) DO UPDATE SET payload = excluded.payload",
                vec![
                    Value::from_i64(id),
                    Value::from_blob(vec![(id & 0xff) as u8; 32 * 1024]),
                ],
            )?;
            connection.execute("COMMIT")?;
            if id == 1 {
                let main = registered_size(owner, session, DATABASE_PATH)?;
                let wal = registered_size(owner, session, DATABASE_WAL_PATH)?;
                report_kill_progress(1, KILL_WRITE_BOUND as u32, main as f64, wal as f64);
            }
        }
        Ok("unexpectedly completed kill loop".to_string())
    })
    .map_err(core_error_to_js)
}

fn registered_size(owner: OwnerId, session: SessionId, path: &str) -> CoreResult<u64> {
    let id = HANDLES
        .with(|registry| registry.borrow().by_path.get(path).copied())
        .ok_or_else(|| io_error(ErrorKind::NotFound, "size of unregistered OPFS path"))?;
    OpfsFile { owner, session, id }.size()
}

/// Execute `BEGIN IMMEDIATE` or `BEGIN EXCLUSIVE` through the real OPFS adapter.
///
/// A WASM environment trap is not converted to success: it rejects the worker
/// RPC and fails the browser run. The parent differential deliberately uses
/// this same export and expects the old revision to trap.
#[wasm_bindgen]
pub fn run_transaction_mode_probe(
    owner: u32,
    session: u32,
    mode: String,
) -> Result<String, JsValue> {
    let (begin_sql, row_id) = match mode.as_str() {
        "immediate" => ("BEGIN IMMEDIATE", 1_i64),
        "exclusive" => ("BEGIN EXCLUSIVE", 2_i64),
        _ => return Err(JsValue::from_str("unsupported transaction mode")),
    };
    with_database(
        OwnerId(owner),
        SessionId(session),
        TRANSACTION_PROBE_PATH,
        |connection| {
            connection.execute(
                "CREATE TABLE IF NOT EXISTS transaction_probe (id INTEGER PRIMARY KEY, mode TEXT NOT NULL)",
            )?;
            execute_bound(
                connection,
                "DELETE FROM transaction_probe WHERE id = ?1",
                vec![Value::from_i64(row_id)],
            )?;
            run_transaction(connection, begin_sql, || {
                execute_bound(
                    connection,
                    "INSERT INTO transaction_probe (id, mode) VALUES (?1, ?2)",
                    vec![Value::from_i64(row_id), Value::from_text(mode.clone())],
                )?;
                Ok(())
            })?;

            connection.execute(begin_sql)?;
            execute_bound(
                connection,
                "DELETE FROM transaction_probe WHERE id = ?1",
                vec![Value::from_i64(row_id)],
            )?;
            connection.execute("ROLLBACK")?;
            let count = query_i64_bound(
                connection,
                "SELECT COUNT(*) FROM transaction_probe WHERE id = ?1 AND mode = ?2",
                vec![Value::from_i64(row_id), Value::from_text(mode.clone())],
            )?;
            require(count == 1, "transaction commit/rollback probe mismatch")?;
            Ok(format!(
                "{{\"mode\":\"{mode}\",\"committed_rows\":{count},\"rollback_preserved\":true}}"
            ))
        },
    )
    .map_err(core_error_to_js)
}

const CACHE_SCHEMA: &str = "
CREATE TABLE meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
CREATE TABLE records (
  __typename TEXT NOT NULL,
  id TEXT NOT NULL,
  value BLOB NOT NULL,
  PRIMARY KEY (__typename, id)
);
CREATE TABLE mutation_queue (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  query TEXT NOT NULL,
  operation_name TEXT,
  variables_json TEXT NOT NULL,
  identity TEXT,
  attempt_count INTEGER NOT NULL DEFAULT 0,
  next_attempt_at_ms INTEGER,
  lease_owner TEXT,
  lease_generation INTEGER NOT NULL DEFAULT 0,
  lease_expires_at_ms INTEGER,
  last_error TEXT,
  created_at_ms INTEGER NOT NULL
);
CREATE TABLE optimistic_layers (
  mutation_id INTEGER PRIMARY KEY,
  optimistic_data_json TEXT NOT NULL,
  normalized_updates BLOB NOT NULL,
  FOREIGN KEY (mutation_id) REFERENCES mutation_queue(id) ON DELETE CASCADE
);
";

const RECORD_UPSERT_SQL: &str = "
INSERT INTO records (__typename, id, value)
VALUES (?1, ?2, ?3)
ON CONFLICT (__typename, id) DO UPDATE SET value = excluded.value
";

#[derive(Debug, PartialEq, Eq)]
struct ForeignKeyViolation {
    table: String,
    rowid: i64,
    parent: String,
    fkid: i64,
}

#[derive(Debug, PartialEq, Eq)]
struct ForeignKeyCheckObservation {
    column_count: usize,
    rows: Vec<ForeignKeyViolation>,
}

fn expected_foreign_key_violation() -> ForeignKeyViolation {
    ForeignKeyViolation {
        table: "optimistic_layers".to_string(),
        rowid: 9_999_999,
        parent: "mutation_queue".to_string(),
        fkid: 0,
    }
}

/// Deliberately reach Turso's retained built-in temp `MemoryIO` WASM trap.
///
/// This is a negative-only route. The page runs it in an isolated worker after
/// all enumerated production-route probes, records the expected environment
/// trap, and terminates that worker without reusing its poisoned runtime.
#[wasm_bindgen]
pub fn run_explicit_temp_negative_probe(owner: u32, session: u32) -> Result<String, JsValue> {
    with_database(
        OwnerId(owner),
        SessionId(session),
        TRANSACTION_PROBE_PATH,
        |connection| {
            connection.execute(
                "CREATE TEMP TABLE explicit_temp_negative_probe (id INTEGER PRIMARY KEY)",
            )?;
            Ok("{\"unexpectedly_succeeded\":true}".to_string())
        },
    )
    .map_err(core_error_to_js)
}

/// Execute the selected WP-04 cache SQL/pragma/transaction shape on real OPFS.
#[wasm_bindgen]
pub fn run_full_cache_sql_probe(owner: u32, session: u32) -> Result<String, JsValue> {
    with_database(
        OwnerId(owner),
        SessionId(session),
        DATABASE_PATH,
        run_full_cache_sql_contract,
    )
    .map_err(core_error_to_js)
}

/// Reopen after a clean worker shutdown and verify the cache-contract rows.
#[wasm_bindgen]
pub fn verify_full_cache_sql_persistence(owner: u32, session: u32) -> Result<String, JsValue> {
    with_database(
        OwnerId(owner),
        SessionId(session),
        DATABASE_PATH,
        |connection| {
            connection.execute("PRAGMA foreign_keys = ON")?;
            let foreign_keys = query_i64(connection, "PRAGMA foreign_keys")?;
            let quick_check = query_string(connection, "PRAGMA quick_check")?;
            let foreign_key_rows = query_row_count(connection, "PRAGMA foreign_key_check", vec![])?;
            let metadata = query_i64(connection, "SELECT COUNT(*) FROM meta")?;
            let records = query_i64(connection, "SELECT COUNT(*) FROM records")?;
            let queue = query_i64(connection, "SELECT COUNT(*) FROM mutation_queue")?;
            let scan = canonical_scan(connection, None)?;
            require(foreign_keys == 1, "foreign_keys was not retained on reopen")?;
            require(quick_check == "ok", "quick_check failed on clean reopen")?;
            require(foreign_key_rows == 0, "foreign_key_check failed on clean reopen")?;
            require(metadata == 3, "metadata rows were not preserved")?;
            require(records == 3, "record rows were not preserved")?;
            require(queue == 0, "queue was not empty after clean reopen")?;
            require(
                scan == ["Type0:1", "Type:9", "Type:tenant:1"],
                "canonical rows changed after clean reopen",
            )?;
            Ok(format!(
                "{{\"metadata_rows\":{metadata},\"record_rows\":{records},\"queue_rows\":{queue},\"quick_check\":\"{quick_check}\",\"foreign_key_check_rows\":{foreign_key_rows},\"canonical_scan\":[\"{}\"]}}",
                scan.join("\",\"")
            ))
        },
    )
    .map_err(core_error_to_js)
}

fn run_full_cache_sql_contract(connection: &Arc<Connection>) -> CoreResult<String> {
    connection.execute("PRAGMA foreign_keys = ON")?;
    require(
        query_i64(connection, "PRAGMA foreign_keys")? == 1,
        "foreign key enforcement did not read back enabled",
    )?;

    run_transaction(connection, "BEGIN IMMEDIATE", || {
        connection.execute(CACHE_SCHEMA)?;
        for (key, value) in [
            ("scope", "opaque-scope"),
            ("namespace", "cache-v1"),
            ("storage_schema_version", "1"),
        ] {
            execute_bound(
                connection,
                "INSERT INTO meta (key, value) VALUES (?1, ?2)",
                vec![Value::from_text(key), Value::from_text(value)],
            )?;
        }
        Ok(())
    })?;

    connection.execute("BEGIN IMMEDIATE")?;
    connection.execute("CREATE TABLE rolled_back_ddl (id INTEGER)")?;
    connection.execute("ROLLBACK")?;
    require(
        query_i64(
            connection,
            "SELECT COUNT(*) FROM sqlite_schema WHERE name = 'rolled_back_ddl'",
        )? == 0,
        "DDL rollback left a table",
    )?;

    run_transaction(connection, "BEGIN IMMEDIATE", || {
        for (typename, id, bytes) in [
            ("ROOT_QUERY", "", vec![0_u8]),
            ("Type", "9", vec![9_u8]),
            ("Type0", "1", vec![1_u8]),
            ("Type", "tenant:1", vec![2_u8]),
            ("__meta", "identity", vec![3_u8]),
        ] {
            require(
                execute_bound(
                    connection,
                    RECORD_UPSERT_SQL,
                    vec![
                        Value::from_text(typename),
                        Value::from_text(id),
                        Value::from_blob(bytes),
                    ],
                )? == 1,
                "record upsert affected-row count mismatch",
            )?;
        }
        require(
            execute_bound(
                connection,
                "DELETE FROM records WHERE __typename = ?1 AND id = ?2",
                vec![Value::from_text("Missing"), Value::from_text("none")],
            )? == 0,
            "missing record delete affected rows",
        )?;
        Ok(())
    })?;

    let record_blob = query_blob_bound(
        connection,
        "SELECT value FROM records WHERE __typename = ?1 AND id = ?2",
        vec![Value::from_text("Type"), Value::from_text("tenant:1")],
    )?;
    require(record_blob == [2_u8], "bound BLOB record changed")?;
    let scan = canonical_scan(connection, None)?;
    require(
        scan == ["Type0:1", "Type:9", "Type:tenant:1"],
        "canonical-key scan did not match Rust string order",
    )?;
    let cursor_scan = canonical_scan(connection, Some("Type:9"))?;
    require(
        cursor_scan == ["Type:tenant:1"],
        "exclusive canonical cursor scan mismatch",
    )?;

    connection.execute("BEGIN IMMEDIATE")?;
    execute_bound(
        connection,
        "INSERT INTO records (__typename, id, value) VALUES (?1, ?2, ?3)",
        vec![
            Value::from_text("Rollback"),
            Value::from_text("one"),
            Value::from_blob(vec![1]),
        ],
    )?;
    let duplicate_rejected = execute_bound(
        connection,
        "INSERT INTO records (__typename, id, value) VALUES (?1, ?2, ?3)",
        vec![
            Value::from_text("Rollback"),
            Value::from_text("one"),
            Value::from_blob(vec![2]),
        ],
    )
    .is_err();
    connection.execute("ROLLBACK")?;
    require(
        duplicate_rejected,
        "duplicate statement failure was not reported",
    )?;
    require(
        query_i64(
            connection,
            "SELECT COUNT(*) FROM records WHERE __typename = 'Rollback'",
        )? == 0,
        "statement failure rollback leaked a row",
    )?;

    let first_queue_id = run_transaction(connection, "BEGIN IMMEDIATE", || {
        insert_queue_with_layer(connection, "first", 100)
    })?;
    let second_queue_id = run_transaction(connection, "BEGIN IMMEDIATE", || {
        insert_queue_with_layer(connection, "second", 200)
    })?;
    require(
        second_queue_id > first_queue_id,
        "AUTOINCREMENT queue IDs were not increasing",
    )?;
    let loaded = query_row_count(
        connection,
        "SELECT m.id, m.query, m.operation_name, m.variables_json, m.identity, m.attempt_count, m.next_attempt_at_ms, m.lease_owner, m.lease_generation, m.lease_expires_at_ms, m.last_error, m.created_at_ms, o.optimistic_data_json, o.normalized_updates FROM mutation_queue AS m LEFT JOIN optimistic_layers AS o ON o.mutation_id = m.id ORDER BY m.id ASC",
        vec![],
    )?;
    require(loaded == 2, "LEFT JOIN queue load shape mismatch")?;
    require(
        query_row_count(
            connection,
            "SELECT o.mutation_id FROM optimistic_layers AS o LEFT JOIN mutation_queue AS m ON m.id = o.mutation_id WHERE m.id IS NULL LIMIT 1",
            vec![],
        )? == 0,
        "orphan optimistic row unexpectedly present",
    )?;

    let selected_head = query_i64(
        connection,
        "SELECT m.id FROM mutation_queue AS m LEFT JOIN optimistic_layers AS o ON o.mutation_id = m.id ORDER BY m.id ASC LIMIT 1",
    )?;
    require(
        selected_head == first_queue_id,
        "strict queue head selection mismatch",
    )?;
    run_transaction(connection, "BEGIN IMMEDIATE", || {
        require(
            execute_bound(
                connection,
                "UPDATE mutation_queue SET attempt_count = ?2, next_attempt_at_ms = NULL, lease_owner = ?3, lease_generation = ?4, lease_expires_at_ms = ?5 WHERE id = ?1",
                vec![
                    Value::from_i64(first_queue_id),
                    Value::from_i64(1),
                    Value::from_text("owner-a"),
                    Value::from_i64(1),
                    Value::from_i64(500),
                ],
            )? == 1,
            "claim update affected-row mismatch",
        )?;
        Ok(())
    })?;
    run_transaction(connection, "BEGIN IMMEDIATE", || {
        require(
            execute_bound(
                connection,
                "UPDATE mutation_queue SET next_attempt_at_ms = ?4, lease_owner = NULL, lease_expires_at_ms = NULL, last_error = ?5 WHERE id = ?1 AND lease_owner = ?2 AND lease_generation = ?3",
                vec![
                    Value::from_i64(first_queue_id),
                    Value::from_text("owner-a"),
                    Value::from_i64(1),
                    Value::from_i64(600),
                    Value::from_text("retry"),
                ],
            )? == 1,
            "defer fenced update mismatch",
        )?;
        Ok(())
    })?;
    require(
        query_i64(
            connection,
            "SELECT id FROM mutation_queue ORDER BY id ASC LIMIT 1",
        )? == first_queue_id,
        "deferred head did not continue blocking later rows",
    )?;
    run_transaction(connection, "BEGIN IMMEDIATE", || {
        execute_bound(
            connection,
            "UPDATE mutation_queue SET attempt_count = ?2, next_attempt_at_ms = NULL, lease_owner = ?3, lease_generation = ?4, lease_expires_at_ms = ?5 WHERE id = ?1",
            vec![
                Value::from_i64(first_queue_id),
                Value::from_i64(2),
                Value::from_text("owner-b"),
                Value::from_i64(2),
                Value::from_i64(700),
            ],
        )?;
        require(
            query_i64_bound(
                connection,
                "SELECT COUNT(*) FROM optimistic_layers WHERE mutation_id = ?1",
                vec![Value::from_i64(first_queue_id)],
            )? == 1,
            "complete mutation lost optimistic layer before settlement",
        )?;
        execute_bound(
            connection,
            RECORD_UPSERT_SQL,
            vec![
                Value::from_text("Complete"),
                Value::from_text("one"),
                Value::from_blob(vec![4]),
            ],
        )?;
        require(
            execute_bound(
                connection,
                "DELETE FROM mutation_queue WHERE id = ?1 AND lease_owner = ?2 AND lease_generation = ?3",
                vec![
                    Value::from_i64(first_queue_id),
                    Value::from_text("owner-b"),
                    Value::from_i64(2),
                ],
            )? == 1,
            "complete mutation fenced delete mismatch",
        )?;
        Ok(())
    })?;
    require(
        query_i64_bound(
            connection,
            "SELECT COUNT(*) FROM optimistic_layers WHERE mutation_id = ?1",
            vec![Value::from_i64(first_queue_id)],
        )? == 0,
        "complete mutation did not cascade optimistic delete",
    )?;

    run_transaction(connection, "BEGIN IMMEDIATE", || {
        execute_bound(
            connection,
            "UPDATE mutation_queue SET attempt_count = ?2, lease_owner = ?3, lease_generation = ?4, lease_expires_at_ms = ?5 WHERE id = ?1",
            vec![
                Value::from_i64(second_queue_id),
                Value::from_i64(1),
                Value::from_text("owner-c"),
                Value::from_i64(1),
                Value::from_i64(800),
            ],
        )?;
        require(
            execute_bound(
                connection,
                "DELETE FROM mutation_queue WHERE id = ?1 AND lease_owner = ?2 AND lease_generation = ?3",
                vec![
                    Value::from_i64(second_queue_id),
                    Value::from_text("owner-c"),
                    Value::from_i64(1),
                ],
            )? == 1,
            "discard mutation fenced delete mismatch",
        )?;
        Ok(())
    })?;

    connection.execute("BEGIN IMMEDIATE")?;
    let foreign_key_rejected = execute_bound(
        connection,
        "INSERT INTO optimistic_layers (mutation_id, optimistic_data_json, normalized_updates) VALUES (?1, ?2, ?3)",
        vec![
            Value::from_i64(9_999_999),
            Value::from_text("{}"),
            Value::from_blob(vec![0]),
        ],
    )
    .is_err();
    connection.execute("ROLLBACK")?;
    require(
        foreign_key_rejected,
        "foreign-key violation was not rejected",
    )?;

    connection.execute("PRAGMA foreign_keys = OFF")?;
    require(
        query_i64(connection, "PRAGMA foreign_keys")? == 0,
        "foreign_keys did not disable for violation-shape probe",
    )?;
    run_transaction(connection, "BEGIN IMMEDIATE", || {
        execute_bound(
            connection,
            "INSERT INTO optimistic_layers (mutation_id, optimistic_data_json, normalized_updates) VALUES (?1, ?2, ?3)",
            vec![
                Value::from_i64(9_999_999),
                Value::from_text("{}"),
                Value::from_blob(vec![0]),
            ],
        )?;
        Ok(())
    })?;
    let foreign_key_violation = query_foreign_key_check(connection)?;
    let expected_foreign_key_violation = expected_foreign_key_violation();
    let foreign_key_check_violation_shape = foreign_key_violation.column_count == 4
        && foreign_key_violation.rows.as_slice() == [expected_foreign_key_violation];
    let foreign_key_violation_rows = foreign_key_violation.rows.len();
    let foreign_key_violation_json = foreign_key_check_json(&foreign_key_violation);
    run_transaction(connection, "BEGIN IMMEDIATE", || {
        execute_bound(
            connection,
            "DELETE FROM optimistic_layers WHERE mutation_id = ?1",
            vec![Value::from_i64(9_999_999)],
        )?;
        Ok(())
    })?;
    connection.execute("PRAGMA foreign_keys = ON")?;
    require(
        query_i64(connection, "PRAGMA foreign_keys")? == 1,
        "foreign_keys did not re-enable after violation-shape probe",
    )?;

    let pre_clear_queue_id = run_transaction(connection, "BEGIN IMMEDIATE", || {
        insert_queue_with_layer(connection, "clear", 300)
    })?;
    run_transaction(connection, "BEGIN EXCLUSIVE", || {
        connection.execute("DELETE FROM optimistic_layers")?;
        connection.execute("DELETE FROM mutation_queue")?;
        connection.execute("DELETE FROM records")?;
        Ok(())
    })?;
    require(
        query_i64(connection, "SELECT COUNT(*) FROM records")? == 0
            && query_i64(connection, "SELECT COUNT(*) FROM mutation_queue")? == 0
            && query_i64(connection, "SELECT COUNT(*) FROM optimistic_layers")? == 0,
        "clear transaction retained cache data",
    )?;
    require(
        query_i64(connection, "SELECT COUNT(*) FROM meta")? == 3,
        "clear transaction removed metadata",
    )?;

    let post_clear_queue_id = run_transaction(connection, "BEGIN IMMEDIATE", || {
        insert_queue_with_layer(connection, "post-clear", 400)
    })?;
    require(
        post_clear_queue_id > pre_clear_queue_id,
        "clear reused an AUTOINCREMENT queue ID",
    )?;
    run_transaction(connection, "BEGIN IMMEDIATE", || {
        execute_bound(
            connection,
            "DELETE FROM mutation_queue WHERE id = ?1",
            vec![Value::from_i64(post_clear_queue_id)],
        )?;
        for (typename, id, bytes) in [
            ("Type", "9", vec![9_u8]),
            ("Type0", "1", vec![1_u8]),
            ("Type", "tenant:1", vec![2_u8]),
        ] {
            execute_bound(
                connection,
                RECORD_UPSERT_SQL,
                vec![
                    Value::from_text(typename),
                    Value::from_text(id),
                    Value::from_blob(bytes),
                ],
            )?;
        }
        Ok(())
    })?;

    let quick_check = query_string(connection, "PRAGMA quick_check")?;
    let foreign_key_rows = query_row_count(connection, "PRAGMA foreign_key_check", vec![])?;
    require(quick_check == "ok", "PRAGMA quick_check did not return ok")?;
    require(
        foreign_key_rows == 0,
        "PRAGMA foreign_key_check returned violation rows",
    )?;
    require(
        canonical_scan(connection, None)? == ["Type0:1", "Type:9", "Type:tenant:1"],
        "final persisted canonical scan mismatch",
    )?;

    Ok(format!(
        "{{\"begin_immediate\":true,\"begin_exclusive\":true,\"ddl_rollback\":true,\"bound_text_blob_integer_null\":true,\"upsert_delete_affected_rows\":true,\"canonical_scan\":[\"{}\"],\"exclusive_cursor\":[\"{}\"],\"left_join_queue_rows\":{loaded},\"strict_head_fencing\":true,\"complete_discard_cascade\":true,\"foreign_key_violation_rejected\":true,\"foreign_key_check_expected_violation\":{{\"column_count\":4,\"rows\":[{{\"table\":\"optimistic_layers\",\"rowid\":9999999,\"parent\":\"mutation_queue\",\"fkid\":0}}]}},\"foreign_key_check_actual_violation\":{foreign_key_violation_json},\"foreign_key_check_violation_shape\":{foreign_key_check_violation_shape},\"foreign_key_check_deliberate_violation_rows\":{foreign_key_violation_rows},\"autoincrement_nonreuse\":true,\"clear_atomic\":true,\"quick_check\":\"{quick_check}\",\"foreign_key_check_rows\":{foreign_key_rows},\"persisted_record_rows\":3}}",
        scan.join("\",\""),
        cursor_scan.join("\",\"")
    ))
}

fn run_transaction<T>(
    connection: &Arc<Connection>,
    begin_sql: &str,
    operation: impl FnOnce() -> CoreResult<T>,
) -> CoreResult<T> {
    connection.execute(begin_sql)?;
    match operation() {
        Ok(value) => match connection.execute("COMMIT") {
            Ok(()) => Ok(value),
            Err(error) => {
                let _ = connection.execute("ROLLBACK");
                Err(error)
            }
        },
        Err(error) => {
            let _ = connection.execute("ROLLBACK");
            Err(error)
        }
    }
}

fn insert_queue_with_layer(
    connection: &Arc<Connection>,
    label: &'static str,
    created_at_ms: i64,
) -> CoreResult<i64> {
    require(
        execute_bound(
            connection,
            "INSERT INTO mutation_queue (query, operation_name, variables_json, identity, attempt_count, next_attempt_at_ms, lease_owner, lease_generation, lease_expires_at_ms, last_error, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            vec![
                Value::from_text(label),
                Value::Null,
                Value::from_text("{}"),
                Value::Null,
                Value::from_i64(0),
                Value::Null,
                Value::Null,
                Value::from_i64(0),
                Value::Null,
                Value::Null,
                Value::from_i64(created_at_ms),
            ],
        )? == 1,
        "queue insert affected-row mismatch",
    )?;
    let id = query_i64(connection, "SELECT last_insert_rowid()")?;
    require(id > 0, "last_insert_rowid was not positive")?;
    require(
        execute_bound(
            connection,
            "INSERT INTO optimistic_layers (mutation_id, optimistic_data_json, normalized_updates) VALUES (?1, ?2, ?3)",
            vec![
                Value::from_i64(id),
                Value::from_text("{}"),
                Value::from_blob(vec![id as u8]),
            ],
        )? == 1,
        "optimistic layer insert affected-row mismatch",
    )?;
    Ok(id)
}

fn canonical_scan(
    connection: &Arc<Connection>,
    cursor: Option<&'static str>,
) -> CoreResult<Vec<String>> {
    match cursor {
        None => query_strings_bound(
            connection,
            "SELECT (__typename || ':' || id) COLLATE BINARY FROM records WHERE __typename IN (?1, ?2) AND NOT (__typename = 'ROOT_QUERY' AND id = '') ORDER BY (__typename || ':' || id) COLLATE BINARY ASC LIMIT ?3",
            vec![
                Value::from_text("Type"),
                Value::from_text("Type0"),
                Value::from_i64(10),
            ],
        ),
        Some(cursor) => query_strings_bound(
            connection,
            "SELECT (__typename || ':' || id) COLLATE BINARY FROM records WHERE __typename IN (?1, ?2) AND NOT (__typename = 'ROOT_QUERY' AND id = '') AND ((__typename || ':' || id) COLLATE BINARY) > ?3 ORDER BY (__typename || ':' || id) COLLATE BINARY ASC LIMIT ?4",
            vec![
                Value::from_text("Type"),
                Value::from_text("Type0"),
                Value::from_text(cursor),
                Value::from_i64(10),
            ],
        ),
    }
}

fn execute_bound(connection: &Arc<Connection>, sql: &str, values: Vec<Value>) -> CoreResult<i64> {
    let mut statement = connection.prepare(sql)?;
    require(
        statement.parameters_count() == values.len(),
        "SQL parameter count mismatch",
    )?;
    for (offset, value) in values.into_iter().enumerate() {
        statement.bind_at(
            NonZeroUsize::new(offset + 1).expect("one-based SQL parameter"),
            value,
        )?;
    }
    drive_statement(&mut statement, |_| Ok(()))?;
    Ok(statement.n_change())
}

fn query_i64(connection: &Arc<Connection>, sql: &str) -> CoreResult<i64> {
    query_one(connection, sql, |row| row.get::<i64>(0))
}

fn query_i64_bound(connection: &Arc<Connection>, sql: &str, values: Vec<Value>) -> CoreResult<i64> {
    query_one_bound(connection, sql, values, |row| row.get::<i64>(0))
}

fn query_string(connection: &Arc<Connection>, sql: &str) -> CoreResult<String> {
    query_one(connection, sql, |row| row.get::<String>(0))
}

fn query_blob_bound(
    connection: &Arc<Connection>,
    sql: &str,
    values: Vec<Value>,
) -> CoreResult<Vec<u8>> {
    query_one_bound(connection, sql, values, |row| {
        match row.get::<&Value>(0)? {
            Value::Blob(bytes) => Ok(bytes.to_vec()),
            _ => Err(internal_error("SQL query did not return a BLOB")),
        }
    })
}

fn query_strings_bound(
    connection: &Arc<Connection>,
    sql: &str,
    values: Vec<Value>,
) -> CoreResult<Vec<String>> {
    let mut statement = prepare_bound(connection, sql, values)?;
    let mut rows = Vec::new();
    drive_statement(&mut statement, |row| {
        rows.push(row.get::<String>(0)?);
        Ok(())
    })?;
    Ok(rows)
}

fn query_row_count(connection: &Arc<Connection>, sql: &str, values: Vec<Value>) -> CoreResult<i64> {
    let mut statement = prepare_bound(connection, sql, values)?;
    let mut rows = 0_i64;
    drive_statement(&mut statement, |_| {
        rows = rows
            .checked_add(1)
            .ok_or_else(|| internal_error("row count overflow"))?;
        Ok(())
    })?;
    Ok(rows)
}

fn query_foreign_key_check(connection: &Arc<Connection>) -> CoreResult<ForeignKeyCheckObservation> {
    let mut statement = connection.prepare("PRAGMA foreign_key_check")?;
    let column_count = statement.num_columns();
    let mut rows = Vec::new();
    drive_statement(&mut statement, |row| {
        require(
            column_count == 4,
            "PRAGMA foreign_key_check row did not have exactly four columns",
        )?;
        rows.push(ForeignKeyViolation {
            table: row.get::<String>(0)?,
            rowid: row.get::<i64>(1)?,
            parent: row.get::<String>(2)?,
            fkid: row.get::<i64>(3)?,
        });
        Ok(())
    })?;
    Ok(ForeignKeyCheckObservation { column_count, rows })
}

fn foreign_key_check_json(observation: &ForeignKeyCheckObservation) -> String {
    let rows = observation
        .rows
        .iter()
        .map(|row| {
            format!(
                "{{\"table\":{},\"rowid\":{},\"parent\":{},\"fkid\":{}}}",
                json_string(&row.table),
                row.rowid,
                json_string(&row.parent),
                row.fkid
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"column_count\":{},\"rows\":[{rows}]}}",
        observation.column_count
    )
}

fn json_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\u{08}' => escaped.push_str("\\b"),
            '\u{0c}' => escaped.push_str("\\f"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character.is_control() => {
                write!(&mut escaped, "\\u{:04x}", character as u32)
                    .expect("writing JSON escape to String");
            }
            character => escaped.push(character),
        }
    }
    escaped.push('"');
    escaped
}

fn prepare_bound(
    connection: &Arc<Connection>,
    sql: &str,
    values: Vec<Value>,
) -> CoreResult<Statement> {
    let mut statement = connection.prepare(sql)?;
    require(
        statement.parameters_count() == values.len(),
        "SQL parameter count mismatch",
    )?;
    for (offset, value) in values.into_iter().enumerate() {
        statement.bind_at(
            NonZeroUsize::new(offset + 1).expect("one-based SQL parameter"),
            value,
        )?;
    }
    Ok(statement)
}

fn query_one_bound<T>(
    connection: &Arc<Connection>,
    sql: &str,
    values: Vec<Value>,
    map: impl FnOnce(&Row) -> CoreResult<T>,
) -> CoreResult<T> {
    let mut statement = prepare_bound(connection, sql, values)?;
    let mut result = None;
    let mut map = Some(map);
    drive_statement(&mut statement, |row| {
        if let Some(map) = map.take() {
            result = Some(map(row)?);
        }
        Ok(())
    })?;
    result.ok_or_else(|| internal_error("SQL query returned no rows"))
}

fn query_one<T>(
    connection: &Arc<Connection>,
    sql: &str,
    map: impl FnOnce(&Row) -> CoreResult<T>,
) -> CoreResult<T> {
    let mut statement = connection.prepare(sql)?;
    let mut result = None;
    let mut map = Some(map);
    drive_statement(&mut statement, |row| {
        if let Some(map) = map.take() {
            result = Some(map(row)?);
        }
        Ok(())
    })?;
    result.ok_or_else(|| internal_error("SQL query returned no rows"))
}

fn drive_statement(
    statement: &mut Statement,
    mut on_row: impl FnMut(&Row) -> CoreResult<()>,
) -> CoreResult<()> {
    loop {
        match statement.step()? {
            StepResult::Done => return Ok(()),
            StepResult::Row => on_row(
                statement
                    .row()
                    .ok_or_else(|| internal_error("row step had no row"))?,
            )?,
            StepResult::IO | StepResult::Yield => statement._io().step()?,
            StepResult::Busy => return Err(LimboError::Busy),
            StepResult::Interrupt => return Err(LimboError::Interrupt),
        }
    }
}

fn require(condition: bool, message: &str) -> CoreResult<()> {
    if condition {
        Ok(())
    } else {
        Err(internal_error(message))
    }
}

fn core_error_to_js(error: LimboError) -> JsValue {
    JsValue::from_str(&error.to_string())
}

#[allow(dead_code)]
fn assert_send_sync_contract() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<OwnerId>();
    assert_send_sync::<SessionId>();
    assert_send_sync::<HandleId>();
    assert_send_sync::<OpfsFile>();
    assert_send_sync::<OpfsIo>();
}

#[cfg(test)]
mod test;
