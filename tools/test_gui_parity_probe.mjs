import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

const modulePath =
  process.env.PLAYWRIGHT_MODULE ??
  "/home/w0w/.local/share/playwright/node_modules/playwright/index.mjs";
const executablePath =
  process.env.PLAYWRIGHT_CHROMIUM ??
  "/home/w0w/.cache/ms-playwright/chromium-1228/chrome-linux64/chrome";
const probePath = path.resolve("tools/playwright/gui_parity_probe.mjs");

function browserAvailable() {
  return fs.existsSync(modulePath) && fs.existsSync(executablePath);
}

test("gui_parity_probe extracts bounded menu labels from a local page", (t) => {
  if (!browserAvailable()) {
    t.skip("local Playwright Chromium is not installed");
    return;
  }

  const fixture = path.join(
    os.tmpdir(),
    `gui-parity-fixture-${process.pid}.html`,
  );
  fs.writeFileSync(
    fixture,
    `<!doctype html><html><head><title>Fixture</title></head><body>
      <nav><a href="#/image">Images</a><button>Power</button></nav>
      <main><h1>Fixture page</h1></main>
    </body></html>\n`,
  );

  const output = path.join(
    "evidence",
    "private",
    `gui-parity-probe-selftest-${process.pid}.json`,
  );
  const outputPath = path.resolve(output);
  try {
    const result = spawnSync(
      process.execPath,
      [
        probePath,
        modulePath,
        executablePath,
        pathToFileURL(fixture).href,
        outputPath,
      ],
      { encoding: "utf8", timeout: 60_000 },
    );
    assert.equal(result.status, 0, result.stderr || result.stdout);

    const receipt = JSON.parse(fs.readFileSync(outputPath, "utf8"));
    assert.equal(receipt.probe, "hardened-blikvm-gui-parity");
    assert.equal(receipt.title, "Fixture");
    assert.ok(
      receipt.menuLabels.includes("Images"),
      `labels: ${receipt.menuLabels}`,
    );
    assert.ok(
      receipt.menuLabels.includes("Power"),
      `labels: ${receipt.menuLabels}`,
    );
    assert.equal(receipt.consoleErrors, 0);
    assert.equal(receipt.cookiesRead, false);
    assert.equal(receipt.storageRead, false);
    assert.equal(receipt.mutationsPerformed, false);
    assert.ok(Object.keys(receipt.tagCounts).length > 0);
  } finally {
    fs.rmSync(fixture, { force: true });
    fs.rmSync(outputPath, { force: true });
  }
});
