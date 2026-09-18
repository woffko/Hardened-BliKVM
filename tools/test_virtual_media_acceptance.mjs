import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import test from "node:test";

const modulePath =
  process.env.PLAYWRIGHT_MODULE ??
  "/home/w0w/.local/share/playwright/node_modules/playwright/index.mjs";
const executablePath =
  process.env.PLAYWRIGHT_CHROMIUM ??
  "/home/w0w/.cache/ms-playwright/chromium-1228/chrome-linux64/chrome";
const probePath = path.resolve("tools/playwright/virtual_media_acceptance.mjs");

const IMAGE = "kubuntu.img";

function runProbe(args, input) {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, args, {
      stdio: ["pipe", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (data) => {
      stdout += data.toString("utf8");
    });
    child.stderr.on("data", (data) => {
      stderr += data.toString("utf8");
    });
    const timer = setTimeout(() => child.kill("SIGKILL"), 120_000);
    child.on("close", (status) => {
      clearTimeout(timer);
      resolve({ status, stdout, stderr });
    });
    child.stdin.end(input);
  });
}

function browserAvailable() {
  return fs.existsSync(modulePath) && fs.existsSync(executablePath);
}

function fixtureHtml() {
  return `<!doctype html><html><head><meta charset="utf-8"><title>Login - Hardened BliKVM</title></head>
<body>
<button id="login">Login</button><input id="user"><input id="password">
<div id="app" hidden>
  <svg class="lucide lucide-disc" width="18" height="18"></svg>
  <div id="modal" hidden><div id="row">${IMAGE}</div></div>
</div>
<script>
const loginButton = document.getElementById("login");
const app = document.getElementById("app");
let mounted = false;
loginButton.addEventListener("click", async () => {
  const rsp = await fetch("/api/auth/login", { method: "POST" });
  if (!rsp.ok) return;
  loginButton.hidden = true;
  app.hidden = false;
  document.title = "Remote Desktop - Hardened BliKVM";
  location.hash = "#/";
});
document.querySelector("svg.lucide-disc").addEventListener("click", () => {
  document.getElementById("modal").hidden = false;
});
async function post(command) {
  await fetch("/api/virtual-media", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ operationId: "a".repeat(32), command }),
  });
}
document.getElementById("row").addEventListener("click", async () => {
  if (!mounted) {
    await post({ action: "set_connected", connected: false });
    await post({ action: "select", imageId: 1, mode: "removable_disk", writable: false });
    await post({ action: "set_connected", connected: true });
    mounted = true;
  } else {
    await post({ action: "set_connected", connected: false });
    mounted = false;
  }
});
</script>
</body></html>`;
}

async function startMock() {
  const server = http.createServer((request, response) => {
    if (request.url === "/api/auth/login") {
      response.writeHead(200, { "Content-Type": "application/json" });
      response.end("{}");
      return;
    }
    if (request.url === "/api/virtual-media") {
      let body = "";
      request.on("data", (chunk) => {
        body += chunk.toString("utf8");
      });
      request.on("end", () => {
        response.writeHead(200, { "Content-Type": "application/json" });
        response.end(JSON.stringify({ applied: true, rejection: "" }));
      });
      return;
    }
    response.writeHead(200, { "Content-Type": "text/html" });
    response.end(fixtureHtml());
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address();
  return { server, baseUrl: `http://127.0.0.1:${port}` };
}

test("virtual media acceptance drives attach and eject against a local mock", async (t) => {
  if (!browserAvailable()) {
    t.skip("local Playwright Chromium is not installed");
    return;
  }
  const { server, baseUrl } = await startMock();
  const usernameFile = path.join(os.tmpdir(), `vm-user-${process.pid}.txt`);
  fs.writeFileSync(usernameFile, "operator\n");
  const output = path.resolve(
    "evidence",
    "private",
    `virtual-media-acceptance-selftest-${process.pid}.json`,
  );
  try {
    const result = await runProbe(
      [
        probePath,
        modulePath,
        executablePath,
        baseUrl,
        usernameFile,
        output,
        IMAGE,
      ],
      "testpassword\n",
    );
    const receipt = JSON.parse(fs.readFileSync(output, "utf8"));
    assert.equal(
      result.status,
      0,
      `${result.stderr || result.stdout} receipt=${JSON.stringify(receipt)}`,
    );
    assert.equal(receipt.probe, "hardened-blikvm-virtual-media-acceptance");
    assert.equal(receipt.result, "passed", JSON.stringify(receipt));
    assert.ok(receipt.selectApplied >= 1);
    assert.ok(receipt.attachApplied >= 1);
    assert.ok(receipt.ejectApplied >= 1);
    const posts = receipt.events.filter(
      (event) =>
        event.route === "/api/virtual-media" && event.method === "POST",
    );
    assert.ok(posts.length >= 3);
    assert.ok(posts.every((event) => event.applied === true));
  } finally {
    fs.rmSync(output, { force: true });
    fs.rmSync(usernameFile, { force: true });
    await new Promise((resolve) => server.close(resolve));
  }
});

test("virtual media acceptance rejects a remote plain-http stand", () => {
  const usernameFile = path.join(os.tmpdir(), `vm-user-${process.pid}-b.txt`);
  fs.writeFileSync(usernameFile, "operator\n");
  try {
    const result = spawnSync(
      process.execPath,
      [
        probePath,
        modulePath,
        executablePath,
        "http://192.0.2.10",
        usernameFile,
        path.resolve("evidence", "private", `vm-bad-${process.pid}.json`),
        IMAGE,
      ],
      { encoding: "utf8", input: "testpassword\n", timeout: 60_000 },
    );
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /https/);
  } finally {
    fs.rmSync(usernameFile, { force: true });
  }
});
