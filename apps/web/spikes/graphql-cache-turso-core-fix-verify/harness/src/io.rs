use serde::Serialize;
use std::sync::{
    atomic::{AtomicU64, AtomicU8, Ordering},
    Arc,
};
use turso_core::{
    io::{FileId, FileSyncType},
    Buffer, Clock, Completion, CompletionError, File, LimboError, MemoryIO, MonotonicInstant,
    OpenFlags, Result, WallClockInstant, IO,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Fault {
    None = 0,
    WriteStorageFull = 1,
    SyncOther = 2,
    CorruptMainHeaderRead = 3,
}

/// Operation counts proving that Turso used the supplied I/O and clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IoEvidence {
    /// Calls to the custom monotonic clock.
    pub monotonic_clock_calls: u64,
    /// Calls to the custom wall clock.
    pub wall_clock_calls: u64,
    /// Main/WAL open calls accepted by the fixed path policy.
    pub open_calls: u64,
    /// File read calls.
    pub read_calls: u64,
    /// File write calls, including vectored writes.
    pub write_calls: u64,
    /// File sync calls.
    pub sync_calls: u64,
    /// File truncate calls.
    pub truncate_calls: u64,
}

/// Production-shaped deterministic I/O for the spike.
///
/// The Turso-facing value owns a fixed database path, accepts only that path
/// and its WAL companion, returns stable path-derived IDs, wraps every file to
/// observe/fault synchronous operations, and supplies a WASM-safe clock. The
/// byte store is Turso's memory `File` implementation; its built-in `Clock`
/// implementation is deliberately bypassed.
pub(crate) struct ProductionLikeIo {
    files: MemoryIO,
    state: Arc<IoState>,
}

struct IoState {
    database_path: String,
    fault: AtomicU8,
    monotonic_tick: AtomicU64,
    monotonic_clock_calls: AtomicU64,
    wall_clock_calls: AtomicU64,
    open_calls: AtomicU64,
    read_calls: AtomicU64,
    write_calls: AtomicU64,
    sync_calls: AtomicU64,
    truncate_calls: AtomicU64,
}

impl ProductionLikeIo {
    pub(crate) fn new(database_path: &str) -> Arc<Self> {
        Arc::new(Self {
            files: MemoryIO::new(),
            state: Arc::new(IoState {
                database_path: database_path.to_owned(),
                fault: AtomicU8::new(Fault::None as u8),
                monotonic_tick: AtomicU64::new(1),
                monotonic_clock_calls: AtomicU64::new(0),
                wall_clock_calls: AtomicU64::new(0),
                open_calls: AtomicU64::new(0),
                read_calls: AtomicU64::new(0),
                write_calls: AtomicU64::new(0),
                sync_calls: AtomicU64::new(0),
                truncate_calls: AtomicU64::new(0),
            }),
        })
    }

    pub(crate) fn database_path(&self) -> &str {
        &self.state.database_path
    }

    pub(crate) fn arm(&self, fault: Fault) {
        self.state.fault.store(fault as u8, Ordering::SeqCst);
    }

    fn validate_path(&self, path: &str) -> Result<()> {
        self.state.validate_path(path)
    }

    pub(crate) fn evidence(&self) -> IoEvidence {
        IoEvidence {
            monotonic_clock_calls: self.state.monotonic_clock_calls.load(Ordering::SeqCst),
            wall_clock_calls: self.state.wall_clock_calls.load(Ordering::SeqCst),
            open_calls: self.state.open_calls.load(Ordering::SeqCst),
            read_calls: self.state.read_calls.load(Ordering::SeqCst),
            write_calls: self.state.write_calls.load(Ordering::SeqCst),
            sync_calls: self.state.sync_calls.load(Ordering::SeqCst),
            truncate_calls: self.state.truncate_calls.load(Ordering::SeqCst),
        }
    }
}

impl IoState {
    fn take_fault(&self, expected: Fault) -> bool {
        self.fault
            .compare_exchange(
                expected as u8,
                Fault::None as u8,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
    }

    fn validate_path(&self, path: &str) -> Result<()> {
        if path == self.database_path
            || path == format!("{}-wal", self.database_path)
            || path == "tursodb_temp_file"
            || path.ends_with("/tursodb_temp_file")
        {
            Ok(())
        } else {
            Err(LimboError::InternalError(format!(
                "custom I/O rejected unregistered path: {path}"
            )))
        }
    }
}

impl Clock for ProductionLikeIo {
    fn current_time_monotonic(&self) -> MonotonicInstant {
        self.state
            .monotonic_clock_calls
            .fetch_add(1, Ordering::SeqCst);
        MonotonicInstant::from_nanos(
            self.state.monotonic_tick.fetch_add(1, Ordering::SeqCst) as u128
        )
    }

    fn current_time_wall_clock(&self) -> WallClockInstant {
        self.state.wall_clock_calls.fetch_add(1, Ordering::SeqCst);
        WallClockInstant {
            secs: 1_700_000_000,
            micros: 123_456,
        }
    }
}

impl IO for ProductionLikeIo {
    fn open_file(&self, path: &str, flags: OpenFlags, direct: bool) -> Result<Arc<dyn File>> {
        self.validate_path(path)?;
        self.state.open_calls.fetch_add(1, Ordering::SeqCst);
        let inner = self.files.open_file(path, flags, direct)?;
        Ok(Arc::new(ObservedOwnedFile {
            state: self.state.clone(),
            inner,
            path: path.to_owned(),
        }))
    }

    fn remove_file(&self, path: &str) -> Result<()> {
        self.validate_path(path)?;
        self.files.remove_file(path)
    }

    fn file_id(&self, path: &str) -> Result<FileId> {
        self.validate_path(path)?;
        Ok(FileId::from_path_hash(path))
    }

    fn supports_shared_wal_coordination(&self) -> bool {
        false
    }
}

struct ObservedOwnedFile {
    state: Arc<IoState>,
    inner: Arc<dyn File>,
    path: String,
}

impl File for ObservedOwnedFile {
    fn lock_file(&self, exclusive: bool) -> Result<()> {
        self.inner.lock_file(exclusive)
    }

    fn unlock_file(&self) -> Result<()> {
        self.inner.unlock_file()
    }

    fn pread(&self, pos: u64, completion: Completion) -> Result<Completion> {
        self.state.read_calls.fetch_add(1, Ordering::SeqCst);
        if self.path == self.state.database_path
            && self.state.take_fault(Fault::CorruptMainHeaderRead)
        {
            return Err(LimboError::Corrupt(format!(
                "injected corrupt main-database read at offset {pos}"
            )));
        }
        self.inner.pread(pos, completion)
    }

    fn pwrite(&self, pos: u64, buffer: Arc<Buffer>, completion: Completion) -> Result<Completion> {
        self.state.write_calls.fetch_add(1, Ordering::SeqCst);
        if self.state.take_fault(Fault::WriteStorageFull) {
            return Err(CompletionError::IOError(
                std::io::ErrorKind::StorageFull,
                "injected pwrite",
            )
            .into());
        }
        self.inner.pwrite(pos, buffer, completion)
    }

    fn pwritev(
        &self,
        pos: u64,
        buffers: Vec<Arc<Buffer>>,
        completion: Completion,
    ) -> Result<Completion> {
        self.state.write_calls.fetch_add(1, Ordering::SeqCst);
        if self.state.take_fault(Fault::WriteStorageFull) {
            return Err(CompletionError::IOError(
                std::io::ErrorKind::StorageFull,
                "injected pwritev",
            )
            .into());
        }
        self.inner.pwritev(pos, buffers, completion)
    }

    fn sync(&self, completion: Completion, sync_type: FileSyncType) -> Result<Completion> {
        self.state.sync_calls.fetch_add(1, Ordering::SeqCst);
        if self.state.take_fault(Fault::SyncOther) {
            return Err(
                CompletionError::IOError(std::io::ErrorKind::Other, "injected sync").into(),
            );
        }
        self.inner.sync(completion, sync_type)
    }

    fn size(&self) -> Result<u64> {
        self.inner.size()
    }

    fn truncate(&self, len: u64, completion: Completion) -> Result<Completion> {
        self.state.truncate_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.truncate(len, completion)
    }

    fn has_hole(&self, pos: usize, len: usize) -> Result<bool> {
        self.inner.has_hole(pos, len)
    }

    fn punch_hole(&self, pos: usize, len: usize) -> Result<()> {
        self.inner.punch_hole(pos, len)
    }
}
