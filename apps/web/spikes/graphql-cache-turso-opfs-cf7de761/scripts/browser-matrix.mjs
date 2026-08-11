import { existsSync } from "node:fs";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const { chromium, firefox, webkit } = require("playwright");
const playwrightVersion = require("playwright/package.json").version;
const root = normalize(join(fileURLToPath(new URL("..", import.meta.url))));
const outputDir = join(root, "measurements/generated");
const outputName = process.env.MATRIX_OUTPUT ?? "browser-matrix.actual.json";
const transactionExpectation = process.env.TRANSACTION_EXPECTATION ?? "head-success";
const coldRuns = Number.parseInt(process.env.COLD_RUNS ?? "2", 10);
const warmRunsPerCold = Number.parseInt(process.env.WARM_RUNS_PER_COLD ?? "1", 10);
if (!Number.isInteger(coldRuns) || coldRuns < 1) throw new Error("COLD_RUNS must be positive");
if (!Number.isInteger(warmRunsPerCold) || warmRunsPerCold < 0) {
  throw new Error("WARM_RUNS_PER_COLD must be non-negative");
}
const expectedToolchain = JSON.parse(
  await readFile(join(root, "measurements/expected-toolchain.json"), "utf8"),
);
if (playwrightVersion !== expectedToolchain.playwright.version) {
  throw new Error(
    `Playwright version mismatch: ${playwrightVersion} != ${expectedToolchain.playwright.version}`,
  );
}
const mime = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".wasm": "application/wasm",
};

const specs = [
  {
    name: "chromium",
    label: "Playwright Chromium",
    type: chromium,
    executablePath: process.env.CHROMIUM_PATH,
    expectedVersion: expectedToolchain.browsers.chromium.version,
  },
  {
    name: "firefox",
    label: "Playwright Firefox",
    type: firefox,
    executablePath: process.env.FIREFOX_PATH,
    expectedVersion: expectedToolchain.browsers.firefox.version,
  },
  {
    name: "webkit-wpe",
    label: "Playwright WebKit WPE (not Safari/WKWebView)",
    type: webkit,
    executablePath: process.env.WEBKIT_PATH,
    expectedVersion: expectedToolchain.browsers["webkit-wpe"].version,
  },
];
const requestedBrowsers = new Set(
  (process.env.BROWSERS ?? specs.map((spec) => spec.name).join(","))
    .split(",")
    .filter(Boolean),
);

const server = createServer(async (request, response) => {
  try {
    const url = new URL(request.url, `http://${request.headers.host}`);
    const relative = url.pathname === "/" ? "harness/index.html" : url.pathname.slice(1);
    const path = normalize(join(root, relative));
    if (!path.startsWith(root)) throw new Error("outside root");
    const body = await readFile(path);
    response.writeHead(200, {
      "content-type": mime[extname(path)] ?? "application/octet-stream",
      "cache-control": "no-store",
      // The spike intentionally serves neither COOP nor COEP.
    });
    response.end(body);
  } catch {
    response.writeHead(404).end("not found");
  }
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const { port } = server.address();
const baseUrl = `http://127.0.0.1:${port}/harness/index.html`;

const runs = [];
const notRun = [];
for (const spec of specs.filter((candidate) => requestedBrowsers.has(candidate.name))) {
  console.error(`starting ${spec.name} with ${spec.executablePath ?? "no executable"}`);
  if (!spec.executablePath || !existsSync(spec.executablePath)) {
    notRun.push({ name: spec.name, reason: "existing browser executable not found" });
    continue;
  }

  let browser;
  try {
    browser = await spec.type.launch({ headless: true, executablePath: spec.executablePath });
    const actualBrowserVersion = browser.version();
    if (actualBrowserVersion !== spec.expectedVersion) {
      throw new Error(
        `${spec.name} version mismatch: ${actualBrowserVersion} != ${spec.expectedVersion}`,
      );
    }

    const specColdRuns = spec.name === "webkit-wpe" ? 1 : coldRuns;
    const specWarmRuns = spec.name === "webkit-wpe" ? 0 : warmRunsPerCold;
    for (let iteration = 1; iteration <= specColdRuns; iteration += 1) {
      const context = await browser.newContext();
      try {
        for (const phase of ["cold", ...Array(specWarmRuns).fill("warm")]) {
          const warmOrdinal =
            phase === "warm"
              ? runs.filter(
                  (run) =>
                    run.name === spec.name && run.iteration === iteration && run.phase === "warm",
                ).length + 1
              : 0;
          const runId =
            phase === "cold"
              ? `${spec.name}-cold-${iteration}`
              : `${spec.name}-warm-${iteration}-${warmOrdinal}`;
          const page = await context.newPage();
          const consoleMessages = [];
          page.on("console", (message) => {
            const text = `${message.type()}: ${message.text()}`;
            consoleMessages.push(text);
            if (process.env.VERBOSE_BROWSER === "1") console.error(`${runId}: ${text}`);
          });
          const runUrl = new URL(baseUrl);
          runUrl.searchParams.set("autorun", "1");
          runUrl.searchParams.set("transactionExpectation", transactionExpectation);
          runUrl.searchParams.set("runPhase", phase);
          runUrl.searchParams.set("iteration", String(iteration));
          try {
            const response = await page.goto(runUrl.href, {
              waitUntil: "domcontentloaded",
              timeout: 30_000,
            });
            const headers = await response.allHeaders();
            await page.waitForSelector('#result[data-state="done"]', { timeout: 180_000 });
            const report = await page.evaluate(() => globalThis.__tursoOpfsReport);
            runs.push({
              name: spec.name,
              runId,
              phase,
              iteration,
              warmOrdinal,
              freshBrowserContext: phase === "cold",
              label: spec.label,
              executablePath: spec.executablePath,
              browserVersion: actualBrowserVersion,
              userAgent: report?.page?.userAgent ?? null,
              responseIsolationHeaders: {
                crossOriginOpenerPolicy: headers["cross-origin-opener-policy"] ?? null,
                crossOriginEmbedderPolicy: headers["cross-origin-embedder-policy"] ?? null,
              },
              report,
              consoleMessages,
            });
            console.error(`finished ${runId}: pass=${report?.pass}`);
          } catch (error) {
            console.error(`failed ${runId}: ${error.stack ?? error}`);
            runs.push({
              name: spec.name,
              runId,
              phase,
              iteration,
              warmOrdinal,
              freshBrowserContext: phase === "cold",
              label: spec.label,
              executablePath: spec.executablePath,
              browserVersion: actualBrowserVersion,
              harnessError: {
                name: error.name,
                message: error.message,
                stack: error.stack ?? null,
              },
              consoleMessages,
            });
          } finally {
            await page.close().catch(() => {});
          }
        }
      } finally {
        await context.close();
      }
    }
  } catch (error) {
    console.error(`failed ${spec.name}: ${error.stack ?? error}`);
    notRun.push({
      name: spec.name,
      executablePath: spec.executablePath,
      reason: `launch/version failed: ${error.message}`,
    });
  } finally {
    await browser?.close().catch(() => {});
  }
}
server.close();

const matrix = {
  generatedAt: new Date().toISOString(),
  playwrightVersion,
  transactionExpectation,
  repetitionPlan: { coldRuns, warmRunsPerCold, webkitWpeRuns: "one honest cold run" },
  exactToolchain: expectedToolchain,
  runs,
  notRun,
};
await mkdir(outputDir, { recursive: true });
await writeFile(join(outputDir, outputName), `${JSON.stringify(matrix, null, 2)}\n`);
console.log(JSON.stringify(matrix, null, 2));
if (requestedBrowsers.has("chromium") && !runs.some((run) => run.name === "chromium")) {
  process.exitCode = 1;
}
