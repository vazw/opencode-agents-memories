#!/usr/bin/env node
// A static file server for the built `web-ui/dist` bundle -- the Step 9 smoke
// harness (D1). It is deliberately tiny: no rebuild watcher, no websocket, no
// token injection, no directory listing any an extensionless path falls back
// to `index.html` (the shape `mem-web` provides with its SPA fallback).
//
// The MIME map is pinned by the blueprint. A module script served with a
// non-JS MIME type leaves the body empty, which would make every browser
// assertion in Step 9 fail, so `.js -> text/javascript` is load-bearing.

import { createServer } from "node:http";
import { createReadStream, statSync } from "node:fs";
import { extname, resolve, sep } from "node:path";

const MIME = new Map([
  [".html", "text/html; charset=utf-8"],
  [".js", "text/javascript"],
  [".mjs", "text/javascript"],
  [".css", "text/css"],
  [".wasm", "application/wasm"],
  [".json", "application/json"],
  [".svg", "image/svg+xml"],
  [".png", "image/png"],
  [".ico", "image/x-icon"],
]);

function parseArgs(argv) {
  const options = { port: 8080, root: "web-ui/dist", host: "127.0.0.1" };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--port" && argv[i + 1]) {
      options.port = Number.parseInt(argv[i + 1], 10);
      i += 1;
    } else if (arg === "--root" && argv[i + 1]) {
      options.root = argv[i + 1];
      i += 1;
    } else if (arg === "--host" && argv[i + 1]) {
      options.host = argv[i + 1];
      i += 1;
    }
  }
  return options;
}

function contentTypeFor(filePath) {
  return MIME.get(extname(filePath).toLowerCase()) ?? "application/octet-stream";
}

const { port, root, host } = parseArgs(process.argv.slice(2));
const rootDir = resolve(root);

function fileForPathname(rawPathname) {
  let pathname;
  try {
    pathname = decodeURIComponent(rawPathname);
  } catch {
    return null;
  }

  const candidate = resolve(rootDir, `.${pathname}`);
  if (candidate !== rootDir && !candidate.startsWith(rootDir + sep)) {
    return null;
  }

  // Extensionless paths (including "/") serve the shell document.
  if (pathname === "/" || extname(pathname) === "") {
    return resolve(rootDir, "index.html");
  }
  return candidate;
}

const server = createServer((request, response) => {
  const url = new URL(request.url ?? "/", `http://${request.headers.host ?? "localhost"}`);
  const filePath = fileForPathname(url.pathname);

  if (filePath === null) {
    response.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
    response.end("Not found\n");
    return;
  }

  let stat;
  try {
    stat = statSync(filePath);
  } catch {
    response.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
    response.end("Not found\n");
    return;
  }
  if (!stat.isFile()) {
    response.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
    response.end("Not found\n");
    return;
  }

  response.writeHead(200, {
    "content-type": contentTypeFor(filePath),
    "content-length": stat.size,
    "cache-control": "no-cache",
  });

  if (request.method === "HEAD") {
    response.end();
    return;
  }
  createReadStream(filePath).pipe(response);
});

server.listen(port, host, () => {
  console.log(`serving ${rootDir} on http://${host}:${port}`);
});
