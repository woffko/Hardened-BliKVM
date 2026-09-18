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
const probePath = path.resolve("tools/playwright/video_frame_timing_probe.mjs");

function browserAvailable() {
  return fs.existsSync(modulePath) && fs.existsSync(executablePath);
}

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

function fixtureHtml() {
  return `<!doctype html><html><head><meta charset="utf-8"><title>Login - Hardened BliKVM</title></head>
<body>
<button id="login">Login</button><input id="user"><input id="password">
<script>
document.getElementById("login").addEventListener("click", async () => {
  const rsp = await fetch("/api/auth/login", { method: "POST" });
  if (!rsp.ok) return;
  document.title = "Remote Desktop - Hardened BliKVM";
  location.hash = "#/";
});
</script>
</body></html>`;
}

function frame(index) {
  const body = Buffer.alloc(64, index % 256);
  return Buffer.concat([
    Buffer.from([0xff, 0xd8]),
    body,
    Buffer.from([0xff, 0xd9]),
  ]);
}

async function startMock() {
  const server = http.createServer((request, response) => {
    if (request.url === "/api/auth/login") {
      response.writeHead(200, { "Content-Type": "application/json" });
      response.end("{}");
      return;
    }
    if (request.url === "/api/stream/mjpeg") {
      response.writeHead(200, { "Content-Type": "multipart/x-mixed-replace" });
      let index = 0;
      const timer = setInterval(() => {
        response.write(frame(index));
        index += 1;
      }, 40);
      request.on("close", () => clearInterval(timer));
      return;
    }
    response.writeHead(200, { "Content-Type": "text/html" });
    response.end(fixtureHtml());
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address();
  return { server, baseUrl: `http://127.0.0.1:${port}` };
}

test("frame timing probe measures a local MJPEG fixture", async (t) => {
  if (!browserAvailable()) {
    t.skip("local Playwright Chromium is not installed");
    return;
  }
  const { server, baseUrl } = await startMock();
  const usernameFile = path.join(os.tmpdir(), `vt-user-${process.pid}.txt`);
  fs.writeFileSync(usernameFile, "operator\n");
  const output = path.resolve(
    "evidence",
    "private",
    `video-frame-timing-selftest-${process.pid}.json`,
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
        "3",
      ],
      "testpassword\n",
    );
    const receipt = JSON.parse(fs.readFileSync(output, "utf8"));
    assert.equal(
      result.status,
      0,
      `${result.stderr || result.stdout} receipt=${JSON.stringify(receipt)}`,
    );
    assert.equal(receipt.probe, "hardened-blikvm-video-frame-timing");
    assert.equal(receipt.result, "passed");
    assert.ok(receipt.frameCount >= 30, JSON.stringify(receipt));
    assert.ok(receipt.averageFrameIntervalMs > 0);
    assert.ok(receipt.averageFrameBytes > 0);
    assert.ok(receipt.averageFps > 0);
  } finally {
    fs.rmSync(output, { force: true });
    fs.rmSync(usernameFile, { force: true });
    await new Promise((resolve) => server.close(resolve));
  }
});

test("frame timing probe rejects a remote plain-http stand", () => {
  const usernameFile = path.join(os.tmpdir(), `vt-user-${process.pid}-b.txt`);
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
        path.resolve("evidence", "private", `vt-bad-${process.pid}.json`),
        "3",
      ],
      { encoding: "utf8", input: "testpassword\n", timeout: 60_000 },
    );
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /https/);
  } finally {
    fs.rmSync(usernameFile, { force: true });
  }
});
