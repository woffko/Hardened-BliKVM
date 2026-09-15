import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { startApp } from "./playwright/app_harness.mjs";

const modulePath =
  process.env.PLAYWRIGHT_MODULE ??
  "/home/w0w/.local/share/playwright/node_modules/playwright/index.mjs";
const executablePath =
  process.env.PLAYWRIGHT_CHROMIUM ??
  "/home/w0w/.cache/ms-playwright/chromium-1228/chrome-linux64/chrome";
const probePath = path.resolve("tools/playwright/restricted_power_probe.mjs");
const dist = path.resolve("web/dist");

function available() {
  return (
    fs.existsSync(modulePath) &&
    fs.existsSync(executablePath) &&
    fs.existsSync(path.join(dist, "index.html"))
  );
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

test("the restricted power path confirms focus and applies the action", async (t) => {
  if (!available()) {
    t.skip("local Playwright Chromium or web/dist is not available");
    return;
  }
  const { server, baseUrl, requests } = await startApp();
  const usernameFile = path.join(os.tmpdir(), `power-user-${process.pid}.txt`);
  fs.writeFileSync(usernameFile, "operator\n");
  const output = path.resolve(
    "evidence",
    "private",
    `restricted-power-selftest-${process.pid}.json`,
  );
  try {
    const result = await runProbe(
      [probePath, modulePath, executablePath, baseUrl, usernameFile, output],
      "testpassword\n",
    );
    const receipt = JSON.parse(fs.readFileSync(output, "utf8"));
    assert.equal(
      result.status,
      0,
      `${result.stderr || result.stdout} receipt=${JSON.stringify(receipt)} requests=${JSON.stringify(requests)}`,
    );
    assert.equal(receipt.probe, "hardened-blikvm-restricted-power");
    assert.equal(receipt.result, "passed");
    assert.ok(receipt.focusResponses >= 1, JSON.stringify(receipt.events));
    assert.ok(receipt.powerResponses >= 1, JSON.stringify(receipt.events));
    assert.ok(requests.includes("POST /api/application-power"));
  } finally {
    fs.rmSync(output, { force: true });
    fs.rmSync(usernameFile, { force: true });
    await new Promise((resolve) => server.close(resolve));
  }
});
