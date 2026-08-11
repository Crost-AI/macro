import { createServer } from "node:http";
import { readFile, stat } from "node:fs/promises";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";

const root = normalize(join(fileURLToPath(new URL("..", import.meta.url))));
const mime = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".wasm": "application/wasm",
};
const port = Number(process.env.PORT ?? 4179);

createServer(async (request, response) => {
  try {
    const url = new URL(request.url, `http://${request.headers.host}`);
    if (url.pathname === "/") {
      response.writeHead(302, { location: "/harness/index.html" }).end();
      return;
    }
    const relative = url.pathname.slice(1);
    const path = normalize(join(root, relative));
    if (!path.startsWith(root) || !(await stat(path)).isFile()) throw new Error("not found");
    const body = await readFile(path);
    response.writeHead(200, {
      "content-type": mime[extname(path)] ?? "application/octet-stream",
      "cache-control": "no-store",
      // Deliberately no COOP or COEP headers.
    });
    response.end(body);
  } catch {
    response.writeHead(404).end("not found");
  }
}).listen(port, "127.0.0.1", () => {
  console.log(`http://127.0.0.1:${port}`);
});
