use serde::Serialize;
use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
};
use wasmparser::{ExternalKind, Parser, Payload, TypeRef, VisitOperator, VisitSimdOperator};

#[derive(Debug, Serialize)]
struct Import {
    module: String,
    name: String,
    kind: String,
}

impl Import {
    fn allowlist_entry(&self) -> String {
        format!("{}\t{}\t{}", self.module, self.name, self.kind)
    }

    fn qualified_name(&self) -> String {
        format!("{}::{}", self.module, self.name)
    }
}

#[derive(Debug, Serialize)]
struct Memory {
    source: String,
    initial_pages: u64,
    maximum_pages: Option<u64>,
    shared: bool,
    memory64: bool,
}

#[derive(Debug, Serialize)]
struct Inspection {
    path: PathBuf,
    bytes: u64,
    memories: Vec<Memory>,
    imports: Vec<Import>,
    exports: Vec<String>,
    custom_sections: Vec<String>,
    atomic_operator_count: u64,
    thread_related_imports: Vec<String>,
    worker_related_imports: Vec<String>,
    expected_import_allowlist: Option<PathBuf>,
    unexpected_imports: Vec<String>,
    missing_imports: Vec<String>,
    duplicate_imports: Vec<String>,
    imports_allowed: Option<bool>,
    environment_trap_markers: Vec<String>,
    contract_violations: Vec<String>,
    browser_contract_compliant: bool,
}

struct Options {
    path: PathBuf,
    assert_browser_contract: bool,
    expected_import_allowlist: Option<PathBuf>,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let options = parse_options()?;
    let bytes = fs::read(&options.path)
        .map_err(|error| format!("read {}: {error}", options.path.display()))?;
    let expected_imports = options
        .expected_import_allowlist
        .as_deref()
        .map(load_expected_imports)
        .transpose()?;
    let inspection = inspect_bytes(
        options.path,
        &bytes,
        options.expected_import_allowlist,
        expected_imports.as_ref(),
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(&inspection)
            .map_err(|error| format!("serialize inspection: {error}"))?
    );
    if options.assert_browser_contract && !inspection.browser_contract_compliant {
        return Err(format!(
            "WASM violates the browser contract: {}",
            inspection.contract_violations.join("; ")
        ));
    }
    Ok(())
}

fn parse_options() -> Result<Options, String> {
    let mut path = None;
    let mut assert_browser_contract = false;
    let mut expected_import_allowlist = None;
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            // Preserve the original flag as an alias, but enforce the stronger
            // exact-memory browser contract rather than merely checking shared.
            "--assert-single-threaded" | "--assert-browser-contract" => {
                assert_browser_contract = true;
            }
            "--expected-imports" => {
                let value = arguments.next().ok_or_else(usage)?;
                if expected_import_allowlist
                    .replace(PathBuf::from(value))
                    .is_some()
                {
                    return Err(usage());
                }
            }
            _ if argument.starts_with('-') => return Err(usage()),
            _ => {
                if path.replace(PathBuf::from(argument)).is_some() {
                    return Err(usage());
                }
            }
        }
    }
    Ok(Options {
        path: path.ok_or_else(usage)?,
        assert_browser_contract,
        expected_import_allowlist,
    })
}

fn usage() -> String {
    "usage: inspect-turso-wasm [--assert-browser-contract] [--expected-imports <allowlist.tsv>] <module.wasm>".to_string()
}

fn load_expected_imports(path: &Path) -> Result<BTreeSet<String>, String> {
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("read import allowlist {}: {error}", path.display()))?;
    let mut entries = BTreeSet::new();
    for (offset, line) in contents.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() != 3
            || !matches!(
                fields[2],
                "function" | "table" | "memory" | "global" | "tag"
            )
        {
            return Err(format!(
                "invalid import allowlist entry at {}:{}; expected module<TAB>name<TAB>kind",
                path.display(),
                offset + 1
            ));
        }
        if !entries.insert(line.to_string()) {
            return Err(format!(
                "duplicate import allowlist entry at {}:{}",
                path.display(),
                offset + 1
            ));
        }
    }
    if entries.is_empty() {
        return Err(format!("import allowlist {} is empty", path.display()));
    }
    Ok(entries)
}

fn inspect_bytes(
    path: PathBuf,
    bytes: &[u8],
    expected_import_allowlist: Option<PathBuf>,
    expected_imports: Option<&BTreeSet<String>>,
) -> Result<Inspection, String> {
    let mut inspection = Inspection {
        path,
        bytes: bytes.len() as u64,
        memories: Vec::new(),
        imports: Vec::new(),
        exports: Vec::new(),
        custom_sections: Vec::new(),
        atomic_operator_count: 0,
        thread_related_imports: Vec::new(),
        worker_related_imports: Vec::new(),
        expected_import_allowlist,
        unexpected_imports: Vec::new(),
        missing_imports: Vec::new(),
        duplicate_imports: Vec::new(),
        imports_allowed: expected_imports.map(|_| true),
        environment_trap_markers: find_environment_trap_markers(bytes),
        contract_violations: Vec::new(),
        browser_contract_compliant: false,
    };

    for payload in Parser::new(0).parse_all(bytes) {
        match payload.map_err(|error| format!("parse WASM: {error}"))? {
            Payload::ImportSection(section) => {
                for import in section {
                    let import = import.map_err(|error| format!("parse import: {error}"))?;
                    let kind = match import.ty {
                        TypeRef::Func(_) => "function",
                        TypeRef::Table(_) => "table",
                        TypeRef::Memory(memory) => {
                            inspection.memories.push(Memory {
                                source: format!("import {}::{}", import.module, import.name),
                                initial_pages: memory.initial,
                                maximum_pages: memory.maximum,
                                shared: memory.shared,
                                memory64: memory.memory64,
                            });
                            "memory"
                        }
                        TypeRef::Global(_) => "global",
                        TypeRef::Tag(_) => "tag",
                    };
                    let parsed_import = Import {
                        module: import.module.to_string(),
                        name: import.name.to_string(),
                        kind: kind.to_string(),
                    };
                    classify_suspicious_import(&parsed_import, &mut inspection);
                    inspection.imports.push(parsed_import);
                }
            }
            Payload::MemorySection(section) => {
                for (index, memory) in section.into_iter().enumerate() {
                    let memory = memory.map_err(|error| format!("parse memory: {error}"))?;
                    inspection.memories.push(Memory {
                        source: format!("defined memory {index}"),
                        initial_pages: memory.initial,
                        maximum_pages: memory.maximum,
                        shared: memory.shared,
                        memory64: memory.memory64,
                    });
                }
            }
            Payload::ExportSection(section) => {
                for export in section {
                    let export = export.map_err(|error| format!("parse export: {error}"))?;
                    inspection.exports.push(format!(
                        "{} ({})",
                        export.name,
                        match export.kind {
                            ExternalKind::Func => "function",
                            ExternalKind::Table => "table",
                            ExternalKind::Memory => "memory",
                            ExternalKind::Global => "global",
                            ExternalKind::Tag => "tag",
                        }
                    ));
                }
            }
            Payload::CodeSectionEntry(body) => {
                let mut operators = body
                    .get_operators_reader()
                    .map_err(|error| format!("read operators: {error}"))?;
                let mut atomic_visitor = AtomicOperatorVisitor;
                while !operators.eof() {
                    let operator = operators
                        .read()
                        .map_err(|error| format!("parse operator: {error}"))?;
                    if atomic_visitor.visit_operator(&operator) {
                        inspection.atomic_operator_count += 1;
                    }
                }
            }
            Payload::CustomSection(section) => {
                inspection.custom_sections.push(section.name().to_string());
            }
            _ => {}
        }
    }

    if let Some(expected_imports) = expected_imports {
        let mut actual_imports = BTreeSet::new();
        for entry in inspection.imports.iter().map(Import::allowlist_entry) {
            if !actual_imports.insert(entry.clone()) {
                inspection.duplicate_imports.push(entry);
            }
        }
        inspection.unexpected_imports = actual_imports
            .difference(expected_imports)
            .cloned()
            .collect();
        inspection.missing_imports = expected_imports
            .difference(&actual_imports)
            .cloned()
            .collect();
        inspection.imports_allowed = Some(
            inspection.unexpected_imports.is_empty()
                && inspection.missing_imports.is_empty()
                && inspection.duplicate_imports.is_empty(),
        );
    }
    inspection.contract_violations = contract_violations(&inspection);
    inspection.browser_contract_compliant = inspection.contract_violations.is_empty();
    Ok(inspection)
}

fn find_environment_trap_markers(bytes: &[u8]) -> Vec<String> {
    [
        "time not implemented on this platform",
        "not implemented on this platform",
        "RuntimeError: unreachable",
        "wasm trap",
    ]
    .into_iter()
    .filter(|marker| {
        bytes
            .windows(marker.len())
            .any(|candidate| candidate == marker.as_bytes())
    })
    .map(str::to_string)
    .collect()
}

fn classify_suspicious_import(import: &Import, inspection: &mut Inspection) {
    let qualified = import.qualified_name();
    let lower = qualified.to_ascii_lowercase();
    if lower.contains("thread")
        || lower.contains("atomic")
        || lower.contains("shared_memory")
        || lower.contains("pthread")
        || lower.contains("emscripten")
        || lower.contains("wasi")
    {
        inspection.thread_related_imports.push(qualified.clone());
    }
    // Worker-global type checks are required by the direct Rust OPFS adapter;
    // they do not construct a nested worker. Every such import is still pinned
    // by the exact allowlist. Other worker imports remain contract violations.
    if lower.contains("worker")
        && !lower.contains("dedicatedworkerglobalscope")
        && !lower.contains("workernavigator")
    {
        inspection.worker_related_imports.push(qualified);
    }
}

fn contract_violations(inspection: &Inspection) -> Vec<String> {
    let mut violations = Vec::new();
    if inspection.memories.len() != 1 {
        violations.push(format!(
            "expected exactly one memory, found {}",
            inspection.memories.len()
        ));
    } else {
        let memory = &inspection.memories[0];
        if memory.shared {
            violations.push("memory is shared".to_string());
        }
        if memory.memory64 {
            violations.push("memory is 64-bit".to_string());
        }
    }
    if inspection.atomic_operator_count != 0 {
        violations.push(format!(
            "found {} atomic operators",
            inspection.atomic_operator_count
        ));
    }
    if !inspection.thread_related_imports.is_empty() {
        violations.push("found thread/shared/WASI-related imports".to_string());
    }
    if !inspection.worker_related_imports.is_empty() {
        violations.push("found worker-related imports".to_string());
    }
    if inspection.imports_allowed == Some(false) {
        violations.push(format!(
            "import set differs from the explicit allowlist ({} unexpected, {} missing, {} duplicate)",
            inspection.unexpected_imports.len(),
            inspection.missing_imports.len(),
            inspection.duplicate_imports.len()
        ));
    }
    violations
}

struct AtomicOperatorVisitor;

macro_rules! define_atomic_operator_visitor {
    (@one @threads $op:ident $({ $($arg:ident: $argty:ty),* })? => $visit:ident) => {
        fn $visit(&mut self $($(,$arg: $argty)*)?) -> bool {
            $($(let _ = $arg;)*)?
            true
        }
    };
    (@one @shared_everything_threads $op:ident $({ $($arg:ident: $argty:ty),* })? => $visit:ident) => {
        fn $visit(&mut self $($(,$arg: $argty)*)?) -> bool {
            $($(let _ = $arg;)*)?
            true
        }
    };
    (@one @$proposal:ident $op:ident $({ $($arg:ident: $argty:ty),* })? => $visit:ident) => {
        fn $visit(&mut self $($(,$arg: $argty)*)?) -> bool {
            $($(let _ = $arg;)*)?
            false
        }
    };
    ($( @$proposal:ident $op:ident $({ $($arg:ident: $argty:ty),* })? => $visit:ident ($($ann:tt)*))*) => {
        $(define_atomic_operator_visitor!(@one @$proposal $op $({ $($arg: $argty),* })? => $visit);)*
    };
}

impl<'a> VisitOperator<'a> for AtomicOperatorVisitor {
    type Output = bool;

    fn simd_visitor(&mut self) -> Option<&mut dyn VisitSimdOperator<'a, Output = Self::Output>> {
        Some(self)
    }

    wasmparser::for_each_visit_operator!(define_atomic_operator_visitor);
}

impl<'a> VisitSimdOperator<'a> for AtomicOperatorVisitor {
    wasmparser::for_each_visit_simd_operator!(define_atomic_operator_visitor);
}

#[cfg(test)]
mod test;
