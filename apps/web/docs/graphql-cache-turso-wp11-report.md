# GraphQL cache Turso WP-11 packaging and performance report

Status: **candidate gates pass; production exposure remains blocked**

Measured 2026-08-17 on Jujutsu change `ukyrkmnlolqutzpuyxywqplnxnxmqywr`.
The machine-readable evidence is
[`../measurements/cache-wasm-wp11.json`](../measurements/cache-wasm-wp11.json),
with per-browser samples under `../measurements/generated/`. Reproduce the
build, development/production browser runs, Node samples, and report with:

```sh
cd apps/web
just report-cache-wasm
```

The browser executables must be available through the existing
`PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH` and
`PLAYWRIGHT_FIREFOX_EXECUTABLE_PATH` environment variables. The recorded paths
and versions are in the JSON evidence.

## Result

The production artifact remains one wasm-pack module containing `cache-core`,
`cache-turso`, `turso-opfs`, Turso core, and the wasm-bindgen shell. The
inspector found exactly one defined 32-bit memory, externally exported as
`memory`; it is unshared and not imported. Every import is a wasm-bindgen
function import from exactly `./cache_wasm_bg.js`. Binaryen reports no threads,
atomics, memory64, or shared-everything feature. TypeScript-AST analysis of the
exact final `cache.engine-worker` chunk conservatively rejects every reference
derived from global `Worker`/`SharedWorker`, including aliases, `Proxy`,
`.bind`, sequence expressions, computed global properties, and
`Reflect.construct`; static marker checks also reject
SharedArrayBuffer, Atomics, WASI, and pthread paths. No Turso/libSQL npm package
or separate Turso WASM is present. The inspector also rejects the destructive
`browserTestMakeNamespaceIncompatible` and `browserTestCorruptQueuePayload`
exports. WP-12 builds those exports only into a separately named
`browser-test-hooks` artifact for recovery tests; that test artifact is absent
from the application dist and excluded from every WP-11 size/timing/hash gate.

The app also emits exactly one known hashed Loro WASM. The allowlist is not
filename-only: its binary must contain only wasm-bindgen function imports from
exactly `./loro_wasm_bg.js`, and its SHA-256 must differ from the cache module.
It is explicitly reported as unrelated and remains raw/unmodified; an opaque
WASM or cache binary renamed to the Loro pattern fails inspection. Only
`cache_wasm_bg-*.wasm` receives the WP-11 sidecar/upload treatment.

### Candidate hard gates

| Metric | Measured | Candidate gate | Result |
|---|---:|---:|---|
| cache WASM build, recorded warm workspace target/sccache | 15.324 s | 180 s | pass |
| first target-fill observation in an empty temporary target | 87.599 s | 180 s | pass |
| raw combined WASM | 11,407,381 B | 12,582,912 B (12 MiB) | pass |
| GNU gzip 1.14 `-9` from stdin | 3,910,054 B | 4,718,592 B (4.5 MiB) | pass |
| deployed Node-zlib Brotli quality 11 sidecar | 2,658,816 B | 3,145,728 B (3 MiB) | pass |
| generated wasm-bindgen glue | 53,595 B | 65,536 B (64 KiB) | pass |
| Node cold compile + instantiate p95, 5 fresh processes | 39.571 ms | 1,000 ms | pass |
| Chromium production DB-ready p95, 5 fresh scopes | 414.900 ms | 3,000 ms | pass |
| Firefox production DB-ready p95, 5 fresh scopes | 487 ms | 3,000 ms | pass |
| Chromium production host first-ready p95 | 427.200 ms | 5,000 ms | pass |
| Firefox production host first-ready p95 | 493 ms | 5,000 ms | pass |
| active WASM linear memory, both production engines | 10,027,008 B | 33,554,432 B (32 MiB) | pass |

The build number is cache-sensitive, so the report records the cache state
rather than presenting it as hardware-independent. The conservative first
observed target-fill time is committed as a machine-readable sample with its
exact command, cache state, measured stable change ID, rustc/cargo/wasm-pack/
wasm-opt identities, and artifact hash. The report captures the measured
revision once before browser evidence is rewritten, then requires every
identity and the inspected package hash to match before gating it at 180
seconds.

Node samples use five new Node processes. Each synchronously compiles the raw
production module and instantiates it through generated `initSync`; the initial
linear memory is 3,211,264 B. Browser samples use five fresh anonymous scopes
per engine and mode. `activation` is trigger-to-instrumented-dedicated-worker
activation, `DB-ready` is trigger-to-Turso/OPFS engine readiness, and `host
first-ready` is trigger-to-first successful `createWorkerCacheHost` read. The
measurement worker installs its Worker interception before dynamically loading
the production runtime. Its URL is recorded separately from the exact
production engine URL; a separate smoke test runs `createWorkerCacheHost`
through the exact, unwrapped production engine. The active browser memory
figure is taken from the exported combined-module memory after DB readiness.

## Packaging and loading evidence

- Web builds retain raw cache WASM plus its adjacent, round-trip-verified
  deterministic Brotli sidecar. Tauri build output, the local OTA archive, and
  the production native app archive retain raw WASM but exclude the web-only
  sidecar; the existing bundle manifest has no asset inventory to invalidate.
- Production and preview generic S3 sync exclude both cache artifacts. The
  targeted uploader first uploads sidecar bytes at the current hashed `.wasm`
  key with `Content-Type: application/wasm`, `Content-Encoding: br`, and
  immutable cache control. Upload and prune are separate operations: preview
  publishes generic assets and then index content before pruning; production
  publishes generic assets/index content and index metadata before pruning.
  Any failed publication step retains all prior cache-WASM keys. The final
  prune excludes the current key, and unrelated WASM is untouched.
- Development Vite serves raw bytes as `application/wasm` with no content
  encoding. The local production-origin emulator serves under the actual
  `/app/` base and returns Brotli sidecar bytes at the original `.wasm` URL with
  `Content-Encoding: br`. Chromium and Firefox fetch, transparently decode,
  compile, and SHA-256 hash the decoded bytes in every fresh scope. Structural
  report validation requires one identical hash across all five samples; report
  generation additionally binds development evidence to the package hash and
  production evidence to the inspected dist hash and exact hashed URL basename.
- The production page entry, its static import graph, and module-preload list
  do not contain the engine worker, glue, or WASM. Browser smoke observes no
  executable engine/WASM request before first use, then exactly one owner
  epoch, dedicated engine, and combined WASM per fresh scope, with zero nested
  workers and no cross-origin isolation or SharedArrayBuffer. Vite development
  may load its inert `?worker&url` URL-export module before use; the actual
  `?worker_file` execution request remains lazy.
- Development URLs resolve to Vite worker/`@fs` URLs. Production URLs resolve
  under `/app/assets/` to distinct exact-production and instrumented engine
  chunks plus a hashed external cache WASM.
- Production smoke fetches source maps through the emitted coordinator, exact
  production-engine, and instrumented-engine URLs. The app inspector separately
  proves the page-entry map and exact final production engine chunk. Generated
  external wasm-bindgen glue is inspected directly and is not a Rollup chunk.

## Exact environment

The committed JSON records the full Rust compiler identity, CPU/OS, executable
paths, user agents, and every raw timing sample. Summary:

- Linux 7.1.8, x86_64, AMD Ryzen 9 7940HS, 16 logical CPUs;
- rustc/cargo 1.94.0, wasm-pack 0.15.0;
- wasm-bindgen crate/build glue 0.2.126; the independently installed CLI on
  `PATH` is 0.2.121 and is not the CLI wasm-pack selected for this build;
- wasm-opt 129, Node 24.18.0, Bun 1.3.13, Vite 6.4.3;
- Playwright 1.62.0;
- Google Chrome for Testing 145.0.7632.6 and Firefox 151.0 executables. Device
  descriptors override their reported user agents; both exact executable
  versions and user agents are retained in the JSON;
- Brotli CLI 1.2.0 for upload round-trip validation and GNU gzip 1.14. Sidecar
  generation uses Node 24's zlib Brotli quality-11 implementation.

## Remaining risks and exposure decision

Passing these candidate budgets does **not** authorize exposure. Product-owner
acceptance of the numeric budgets remains pending. WP-12 records a local
Chromium/Firefox navigation/resource subset, privacy-safe telemetry, a
session-latched kill-switch policy, and provider-neutral rollback thresholds.
Required Section 10 real-browser/native entries, exact stable Safari, live
delivery, owner acceptance, and deployed rollback automation remain pending.
See [`graphql-cache-turso-wp12-report.md`](./graphql-cache-turso-wp12-report.md).

Headroom is finite: raw WASM is about 90.5% of the candidate cap and Brotli is
about 84.5% of its cap. Toolchain or Turso upgrades can change wasm-opt and
compression output, so the build hard-fails instead of silently increasing a
budget. Browser measurements are local fresh-scope startup evidence, not WAN,
low-end-device, Safari, navigation, or fleet-tail evidence. The `/app/`
precompressed-origin smoke is a local deployment emulation, not live
S3/CloudFront verification. Live delivery behavior remains WP-12 work after
owner acceptance; WP-11 tests prove local bytes, URL/base behavior, S3 command
ordering, keys, and metadata construction without performing a production
upload.
