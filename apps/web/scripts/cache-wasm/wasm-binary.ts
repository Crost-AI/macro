export interface WasmMemoryInspection {
  source: 'defined' | 'imported';
  initialPages: bigint;
  maximumPages?: bigint;
  shared: boolean;
  memory64: boolean;
}

export interface WasmBinaryInspection {
  memories: WasmMemoryInspection[];
  memoryExports: Array<{ name: string; index: number }>;
  imports: Array<{ module: string; name: string; kind: string }>;
}

class Reader {
  private offset = 0;

  constructor(private readonly bytes: Uint8Array) {}

  get done(): boolean {
    return this.offset === this.bytes.length;
  }

  byte(): number {
    if (this.offset >= this.bytes.length)
      throw new Error('unexpected end of WASM');
    return this.bytes[this.offset++];
  }

  bytesValue(length: number): Uint8Array {
    const end = this.offset + length;
    if (
      !Number.isSafeInteger(length) ||
      length < 0 ||
      end > this.bytes.length
    ) {
      throw new Error('invalid WASM byte range');
    }
    const value = this.bytes.subarray(this.offset, end);
    this.offset = end;
    return value;
  }

  unsigned(): bigint {
    let value = 0n;
    let shift = 0n;
    for (let count = 0; count < 10; count++) {
      const byte = this.byte();
      value |= BigInt(byte & 0x7f) << shift;
      if ((byte & 0x80) === 0) return value;
      shift += 7n;
    }
    throw new Error('invalid WASM unsigned LEB128');
  }

  unsignedNumber(): number {
    const value = this.unsigned();
    if (value > BigInt(Number.MAX_SAFE_INTEGER)) {
      throw new Error('WASM integer exceeds JavaScript safe range');
    }
    return Number(value);
  }

  signed(): bigint {
    let value = 0n;
    let shift = 0n;
    let byte = 0;
    for (let count = 0; count < 10; count++) {
      byte = this.byte();
      value |= BigInt(byte & 0x7f) << shift;
      shift += 7n;
      if ((byte & 0x80) === 0) {
        if ((byte & 0x40) !== 0) value |= -1n << shift;
        return value;
      }
    }
    throw new Error('invalid WASM signed LEB128');
  }

  string(): string {
    return new TextDecoder('utf-8', { fatal: true }).decode(
      this.bytesValue(this.unsignedNumber())
    );
  }
}

function readLimits(reader: Reader): Omit<WasmMemoryInspection, 'source'> {
  const flags = reader.unsignedNumber();
  if ((flags & ~0x0f) !== 0)
    throw new Error(`unsupported WASM limits flags ${flags}`);
  const initialPages = reader.unsigned();
  const maximumPages = (flags & 1) !== 0 ? reader.unsigned() : undefined;
  if ((flags & 8) !== 0) reader.unsigned();
  return {
    initialPages,
    maximumPages,
    shared: (flags & 2) !== 0,
    memory64: (flags & 4) !== 0,
  };
}

function readReferenceType(reader: Reader): void {
  const type = reader.byte();
  if (type === 0x63 || type === 0x64) reader.signed();
}

function readImportSection(
  reader: Reader,
  inspection: WasmBinaryInspection
): void {
  const count = reader.unsignedNumber();
  for (let index = 0; index < count; index++) {
    const module = reader.string();
    const name = reader.string();
    const kind = reader.byte();
    switch (kind) {
      case 0:
        reader.unsigned();
        inspection.imports.push({ module, name, kind: 'function' });
        break;
      case 1:
        readReferenceType(reader);
        readLimits(reader);
        inspection.imports.push({ module, name, kind: 'table' });
        break;
      case 2:
        inspection.memories.push({ source: 'imported', ...readLimits(reader) });
        inspection.imports.push({ module, name, kind: 'memory' });
        break;
      case 3:
        readReferenceType(reader);
        reader.byte();
        inspection.imports.push({ module, name, kind: 'global' });
        break;
      case 4:
        reader.byte();
        reader.unsigned();
        inspection.imports.push({ module, name, kind: 'tag' });
        break;
      default:
        throw new Error(`unknown WASM import kind ${kind}`);
    }
  }
}

function readMemorySection(
  reader: Reader,
  inspection: WasmBinaryInspection
): void {
  const count = reader.unsignedNumber();
  for (let index = 0; index < count; index++) {
    inspection.memories.push({ source: 'defined', ...readLimits(reader) });
  }
}

function readExportSection(
  reader: Reader,
  inspection: WasmBinaryInspection
): void {
  const count = reader.unsignedNumber();
  for (let offset = 0; offset < count; offset++) {
    const name = reader.string();
    const kind = reader.byte();
    const index = reader.unsignedNumber();
    if (kind === 2) inspection.memoryExports.push({ name, index });
  }
}

export function inspectWasmBinary(bytes: Uint8Array): WasmBinaryInspection {
  const reader = new Reader(bytes);
  const magicAndVersion = [...reader.bytesValue(8)];
  const expected = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
  if (!magicAndVersion.every((byte, index) => byte === expected[index])) {
    throw new Error('expected a version-1 core WebAssembly module');
  }

  const inspection: WasmBinaryInspection = {
    memories: [],
    memoryExports: [],
    imports: [],
  };
  while (!reader.done) {
    const sectionId = reader.byte();
    const section = new Reader(reader.bytesValue(reader.unsignedNumber()));
    if (sectionId === 2) readImportSection(section, inspection);
    if (sectionId === 5) readMemorySection(section, inspection);
    if (sectionId === 7) readExportSection(section, inspection);
    if (!section.done && [2, 5, 7].includes(sectionId)) {
      throw new Error(`WASM section ${sectionId} has trailing bytes`);
    }
  }
  return inspection;
}

export function wasmBindgenImportViolations(
  inspection: WasmBinaryInspection,
  expectedModule: string,
  glueImportNames?: ReadonlySet<string>
): string[] {
  const unexpectedImports = inspection.imports.filter(
    ({ module, name, kind }) =>
      module !== expectedModule ||
      kind !== 'function' ||
      !/^__(?:wbg|wbindgen)_[A-Za-z0-9_]+$/.test(name) ||
      (glueImportNames !== undefined && !glueImportNames.has(name))
  );
  if (inspection.imports.length === 0) {
    return [`expected wasm-bindgen function imports from ${expectedModule}`];
  }
  return unexpectedImports.length > 0
    ? [
        `unexpected imports for ${expectedModule}: ${unexpectedImports
          .map(({ module, name, kind }) => `${module}::${name} (${kind})`)
          .join(', ')}`,
      ]
    : [];
}

export function wasmContractViolations(
  inspection: WasmBinaryInspection,
  enabledFeatures: readonly string[],
  glueImportNames?: ReadonlySet<string>
): string[] {
  const violations: string[] = [];
  const importedMemories = inspection.memories.filter(
    (memory) => memory.source === 'imported'
  );
  const definedMemories = inspection.memories.filter(
    (memory) => memory.source === 'defined'
  );
  if (importedMemories.length > 0) {
    violations.push(`memory is imported (${importedMemories.length})`);
  }
  if (inspection.memories.length !== 1 || definedMemories.length !== 1) {
    violations.push(
      `expected exactly one defined memory, found ${definedMemories.length} defined / ${inspection.memories.length} total`
    );
  }
  for (const memory of inspection.memories) {
    if (memory.shared) violations.push('memory is shared');
    if (memory.memory64) violations.push('memory is memory64');
  }
  if (
    inspection.memoryExports.length !== 1 ||
    inspection.memoryExports[0]?.name !== 'memory' ||
    inspection.memoryExports[0]?.index !== 0
  ) {
    violations.push(
      'memory must be externally exported exactly once as memory index 0'
    );
  }

  violations.push(
    ...wasmBindgenImportViolations(
      inspection,
      './cache_wasm_bg.js',
      glueImportNames
    )
  );
  if (enabledFeatures.includes('--enable-threads')) {
    violations.push('WASM atomics/threads feature is enabled');
  }
  if (enabledFeatures.includes('--enable-memory64')) {
    violations.push('WASM memory64 feature is enabled');
  }
  if (enabledFeatures.includes('--enable-shared-everything')) {
    violations.push('WASM shared-everything threads feature is enabled');
  }
  return violations;
}
