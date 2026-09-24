import assert from "node:assert/strict";
import { spawn } from "node:child_process";
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
const probePath = path.resolve("tools/playwright/gui_parity_probe.mjs");
const dist = path.resolve("web/dist");

function browserAvailable() {
  return (
    fs.existsSync(modulePath) &&
    fs.existsSync(executablePath) &&
    fs.existsSync(path.join(dist, "index.html"))
  );
}

function manifest() {
  const status = (availability) => ({
    availability,
    evidence: "source_verified",
  });
  return {
    schemaVersion: 1,
    product: {
      name: "Hardened BliKVM",
      shortName: "BLi-KVM",
      tagline: "fixture",
    },
    platform: {
      id: "bli-h616-pikvm",
      board: "BliKVM v4 H616",
      architecture: "aarch64",
      operatingSystem: "Armbian",
    },
    video: {
      status: status("read_only"),
      transports: ["mjpeg"],
      preferredTransport: "mjpeg",
      maxWidth: null,
      maxHeight: null,
      audio: status("unavailable"),
    },
    hid: {
      status: status("read_write"),
      modes: ["keyboard", "absolute_mouse", "relative_mouse"],
    },
    power: {
      status: status("read_write"),
      actions: ["power_short", "power_long", "reset"],
      ledState: status("read_only"),
    },
    virtualMedia: {
      status: status("read_write"),
      modes: ["cd_rom", "removable_disk"],
      resumableUpload: status("unavailable"),
      remoteDownload: status("unavailable"),
    },
    board: { features: {} },
    system: {
      status: status("read_only"),
      readOnlyRootfs: true,
      maintenanceWriteMode: false,
      signedApplicationUpdates: status("unavailable"),
      signedSystemUpdates: status("unavailable"),
      automaticRollback: status("unavailable"),
    },
  };
}

const CONTENT_TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".woff2": "font/woff2",
};

async function startApp() {
  const requests = [];
  const server = http.createServer((request, response) => {
    const route = new URL(request.url, "http://127.0.0.1").pathname;
    if (route.startsWith("/api/")) {
      requests.push(`${request.method} ${route}`);
      const json = (value, status = 200) => {
        response.writeHead(status, { "Content-Type": "application/json" });
        response.end(JSON.stringify(value));
      };
      if (route === "/api/auth/setup" && request.method === "GET") {
        json({ code: 0, data: { required: false } });
        return;
      }
      if (route === "/api/auth/login" && request.method === "POST") {
        response.writeHead(200, {
          "Content-Type": "application/json",
          "Set-Cookie": "session=fixture; Path=/; HttpOnly",
        });
        response.end(
          JSON.stringify({ code: 0, data: { csrfToken: "fixture-csrf" } }),
        );
        return;
      }
      if (route === "/api/capabilities") {
        json({ code: 0, data: manifest() });
        return;
      }
      if (route === "/api/inventory") {
        json({ code: 0, data: { schemaVersion: 3 } });
        return;
      }
      if (route === "/api/application/current-version") {
        json({ code: 0, data: { current: "1.0.0" } });
        return;
      }
      json({ code: 1, msg: "not found" }, 404);
      return;
    }
    const relative = route === "/" ? "index.html" : route.replace(/^\/+/, "");
    const candidate = path.join(dist, relative);
    if (
      candidate.startsWith(dist) &&
      fs.existsSync(candidate) &&
      fs.statSync(candidate).isFile()
    ) {
      response.writeHead(200, {
        "Content-Type":
          CONTENT_TYPES[path.extname(candidate)] ?? "application/octet-stream",
      });
      response.end(fs.readFileSync(candidate));
      return;
    }
    response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" });
    response.end(fs.readFileSync(path.join(dist, "index.html")));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  return {
    server,
    baseUrl: `http://127.0.0.1:${server.address().port}`,
    requests,
  };
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

test("the real app renders the desktop without calling the removed endpoints", async (t) => {
  if (!browserAvailable()) {
    t.skip("local Playwright Chromium or web/dist is not available");
    return;
  }
  const { server, baseUrl, requests } = await startApp();
  const usernameFile = path.join(
    os.tmpdir(),
    `parity-app-user-${process.pid}.txt`,
  );
  fs.writeFileSync(usernameFile, "operator\n");
  const output = path.resolve(
    "evidence",
    "private",
    `gui-parity-app-selftest-${process.pid}.json`,
  );
  const contextPath = `${output}.context.json`;
  fs.writeFileSync(
    contextPath,
    JSON.stringify({
      acceptanceRunId: "gui-parity-app-selftest",
      reviewedSourceCommit: "a".repeat(40),
    }),
    { mode: 0o600 },
  );
  try {
    const result = await runProbe(
      [
        probePath,
        modulePath,
        executablePath,
        baseUrl,
        output,
        usernameFile,
        "ours",
        contextPath,
        "fixture-source-revision",
      ],
      "testpassword\n",
    );
    assert.equal(result.status, 0, result.stderr || result.stdout);
    const receipt = JSON.parse(fs.readFileSync(output, "utf8"));
    assert.equal(receipt.schemaVersion, 2);
    assert.equal(receipt.result, "passed");
    assert.equal(receipt.targetRole, "ours");
    assert.equal(receipt.title, "Remote Desktop - Hardened BliKVM");
    // This fixture does not advertise the leader-key capability.
    assert.ok(!requests.includes("GET /api/hid/shortcut/leader-key"));
    for (const removed of ["/api/vm/gpio", "/api/hid/mode", "/api/hid/reset"]) {
      assert.ok(
        !requests.includes(`GET ${removed}`) &&
          !requests.includes(`POST ${removed}`),
        `${removed} was requested: ${JSON.stringify(requests)}`,
      );
    }
    assert.ok(requests.includes("GET /api/capabilities"));
    assert.equal(receipt.coverage.settingsTabs, "complete");
    assert.ok(
      receipt.settingsTabs.includes("About BliKVM"),
      `settings tabs: ${JSON.stringify(receipt.settingsTabs)}`,
    );
  } finally {
    fs.rmSync(output, { force: true });
    fs.rmSync(contextPath, { force: true });
    fs.rmSync(usernameFile, { force: true });
    await new Promise((resolve) => server.close(resolve));
  }
});
