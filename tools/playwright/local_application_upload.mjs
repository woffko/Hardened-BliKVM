// Local browser/component acceptance only. No device, real account, signing
// key, or privileged worker is used. HTTP receipts below are synthetic.
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { createServer } from "../../web/node_modules/vite/dist/node/index.js";
import { chromium } from "/home/w0w/.local/share/playwright/node_modules/playwright/index.mjs";

const webRoot = fileURLToPath(new URL("../../web", import.meta.url));
// Tailwind/PostCSS resolve project-relative sources from the process cwd.
process.chdir(webRoot);
const archive = Buffer.from("synthetic archive browser fixture");
const receipt = {
  slot: {
    slotId: `app-18446744073709551615-${"a".repeat(64)}`,
    version: "1.0.0",
    sequence: "18446744073709551615",
    minimumSystemSequence: "1",
  },
  disposition: "published",
  manifestSha256: "a".repeat(64),
  payloadBytes: String(archive.length),
};
let mode = "success";
let activationMode = "accepted";
const activations = [];
let uploads = 0;
let aborts = 0;
const uploadModes = [];
const abortedModes = [];
const violations = [];
const server = await createServer({
  root: webRoot,
  configFile: `${webRoot}/vite.config.ts`,
  server: { host: "127.0.0.1", port: 0, hmr: false, open: false },
  logLevel: "error",
  plugins: [
    {
      name: "local-upload-fixture",
      configureServer(server) {
        server.middlewares.use(async (request, response, next) => {
          const route = request.url?.split("?")[0];
          if (route === "/__upload-test") {
            response.setHeader("Content-Type", "text/html");
            response.end(
              await server.transformIndexHtml(
                route,
                `<!doctype html><html><body><div id="root"></div><script type="module">
            import React from 'react';
            import ReactDOM from 'react-dom/client';
            import { BliApplicationUpload } from '/src/pages/desktop/menu/settings/update/bli-upload.tsx';
            import '/src/i18n/index.ts';
            import '/src/assets/styles/index.css';
            const root = ReactDOM.createRoot(document.getElementById('root'));
            const current = { slotId: 'app-00000000000000000002-' + 'b'.repeat(64), version: '0.1.0', sequence: '2', minimumSystemSequence: '1' };
            const initialStatus = { schemaVersion: 1, generation: '10', floorSequence: '2', systemSequence: '1', selected: current, confirmed: current, previous: null, pending: null, lastRollback: null };
            let status = initialStatus;
            window.uploadHarness = (enabled = true, ready = true, visible = true) => root.render(visible ? React.createElement(BliApplicationUpload, {enabled, ready, status, refresh: () => {}}) : null);
            window.activationStatus = (patch) => { status = { ...initialStatus, ...patch }; window.uploadHarness(); };
            document.cookie = 'hardened-blikvm-csrf=synthetic-browser-fixture; SameSite=Strict; Path=/';
            window.uploadHarness();
          </script></body></html>`,
              ),
            );
            return;
          }
          if (route?.startsWith("/api/")) {
            if (
              route === "/api/application-update/activate" &&
              request.method === "POST"
            ) {
              const chunks = [];
              for await (const chunk of request) chunks.push(chunk);
              const intent = JSON.parse(Buffer.concat(chunks).toString());
              activations.push(intent);
              if (
                request.headers["x-csrf-token"] !== "synthetic-browser-fixture"
              )
                violations.push("activation CSRF");
              if (request.headers["content-type"] !== "application/json")
                violations.push("activation type");
              if (
                JSON.stringify(Object.keys(intent).sort()) !==
                JSON.stringify(["expectedGeneration", "slotId"])
              )
                violations.push("activation fields");
              response.setHeader("Content-Type", "application/json");
              if (activationMode === "uncertain")
                response
                  .writeHead(503)
                  .end(
                    JSON.stringify({ code: -503, msg: "service unavailable" }),
                  );
              else
                response.writeHead(202).end(
                  JSON.stringify({
                    code: 0,
                    data: {
                      phase: "awaiting_health",
                      receipt: {
                        operationId: "c".repeat(32),
                        targetSlotId: receipt.slot.slotId,
                      },
                    },
                  }),
                );
              return;
            }
            if (
              route !== "/api/application-update/upload" ||
              request.method !== "POST"
            ) {
              violations.push("unexpected API route");
              response.writeHead(404).end();
              return;
            }
            uploads += 1;
            const selectedMode = mode;
            uploadModes.push(selectedMode);
            response.on("close", () => {
              if (!response.writableEnded) {
                aborts += 1;
                abortedModes.push(selectedMode);
              }
            });
            const chunks = [];
            for await (const chunk of request) chunks.push(chunk);
            if (request.headers["content-type"] !== "application/octet-stream")
              violations.push("content type");
            if (request.headers["content-length"] !== String(archive.length))
              violations.push("content length");
            if (request.headers["x-csrf-token"] !== "synthetic-browser-fixture")
              violations.push("CSRF header");
            if (!Buffer.concat(chunks).equals(archive))
              violations.push("archive bytes");
            if (request.headers["transfer-encoding"])
              violations.push("transfer encoding");
            // Keep the response separate from transport completion so the UI's
            // 100%-but-not-verified state is observable in a real browser.
            setTimeout(
              () => {
                if (response.destroyed) return;
                if (selectedMode === "lost") {
                  response.destroy();
                  return;
                }
                const body =
                  selectedMode === "rejected"
                    ? { code: -1, msg: "fixture rejection" }
                    : {
                        code: 0,
                        data: {
                          ...receipt,
                          disposition:
                            selectedMode === "retry"
                              ? "already_present"
                              : "published",
                        },
                      };
                response.setHeader("Content-Type", "application/json");
                response.end(JSON.stringify(body));
              },
              selectedMode === "cancel" ? 1500 : 500,
            );
            return;
          }
          next();
        });
      },
    },
  ],
});
let browser;
let phase = "start";
try {
  await server.listen();
  const address = server.httpServer.address();
  browser = await chromium.launch({
    executablePath:
      "/home/w0w/.cache/ms-playwright/chromium-1228/chrome-linux64/chrome",
    headless: true,
  });
  const page = await browser.newPage({
    locale: "en-US",
    viewport: { width: 390, height: 844 },
  });
  // The dev server compiles modules on the first request; a loaded build host
  // needs room for that.
  page.setDefaultTimeout(20000);
  page.setDefaultNavigationTimeout(60000);
  const errors = [];
  let logicalUploads = 0;
  page.on("request", (request) => {
    if (new URL(request.url()).pathname === "/api/application-update/upload")
      logicalUploads += 1;
  });
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto(`http://127.0.0.1:${address.port}/__upload-test`);
  const panel = page.getByTestId("bli-application-upload");
  const picker = page.getByTestId("application-archive");
  const choose = () =>
    picker.setInputFiles({
      name: "fixture.tar",
      mimeType: "application/octet-stream",
      buffer: archive,
    });
  phase = "invalid-size";
  await picker.setInputFiles({
    name: "empty.tar",
    mimeType: "application/octet-stream",
    buffer: Buffer.alloc(0),
  });
  await panel
    .getByText("Choose a nonempty archive no larger than 128 MiB.", {
      exact: true,
    })
    .waitFor();
  assert.equal(
    await panel
      .getByRole("button", { name: "Upload and verify", exact: true })
      .isEnabled(),
    false,
  );
  assert.equal(uploads, 0);
  phase = "verified-receipt";
  await choose();
  await panel
    .getByRole("button", { name: "Upload and verify", exact: true })
    .click();
  await panel
    .getByText(
      "Archive sent. Waiting for signature verification and staging…",
      { exact: true },
    )
    .waitFor();
  assert.equal(await picker.isEnabled(), false);
  await panel
    .getByText("Archive verified and staged", { exact: true })
    .waitFor();
  await panel
    .getByText("1.0.0 · #18446744073709551615", { exact: true })
    .waitFor();
  assert.equal(
    await panel.getByRole("button", { name: /activate|install/i }).count(),
    0,
  );
  assert.ok(
    await panel.evaluate(
      (element) => element.scrollWidth <= element.clientWidth,
    ),
  );
  phase = "activation-confirmation-fence";
  const review = page.getByTestId("application-activation-review");
  await review.click();
  const dialog = page.getByRole("dialog");
  await dialog.waitFor();
  assert.equal(activations.length, 0);
  await page.evaluate(() => window.activationStatus({ generation: "11" }));
  assert.equal(
    await page.getByTestId("application-activation-confirm").isEnabled(),
    false,
  );
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await review.click();
  // The following same-tick DOM clicks intentionally bypass Playwright's
  // click actionability wait. Wait for React to enable the newly reviewed
  // intent first; a still-disabled button would silently ignore both clicks.
  await page.waitForFunction(
    () =>
      document.querySelector('[data-testid="application-activation-confirm"]')
        ?.disabled === false,
  );
  await page
    .getByTestId("application-activation-confirm")
    .evaluate((button) => {
      button.click();
      button.click();
    });
  await panel.getByText(/Activation is awaiting health confirmation/).waitFor();
  assert.deepEqual(activations, [
    { slotId: receipt.slot.slotId, expectedGeneration: "11" },
  ]);
  assert.equal(await picker.isEnabled(), false);
  await page.evaluate(
    (target) =>
      window.activationStatus({
        generation: "12",
        selected: target,
        pending: {
          operationId: "c".repeat(32),
          target,
          phase: "awaiting_health",
        },
      }),
    receipt.slot,
  );
  await panel.getByText(/Activation is awaiting health confirmation/).waitFor();
  await page.evaluate(
    (target) =>
      window.activationStatus({
        generation: "13",
        selected: target,
        confirmed: target,
        floorSequence: target.sequence,
      }),
    receipt.slot,
  );
  await panel
    .getByText("The requested target is now the confirmed application.", {
      exact: true,
    })
    .waitFor();
  assert.equal(await review.isEnabled(), false);
  phase = "activation-uncertain-reconciliation";
  await page.evaluate(() => window.activationStatus({ generation: "20" }));
  activationMode = "uncertain";
  await review.click();
  await page.getByTestId("application-activation-confirm").click();
  await panel.getByText(/The outcome is unknown/).waitFor();
  await panel
    .getByRole("button", { name: "Check current status", exact: true })
    .click();
  assert.equal(activations.length, 2);
  await page.evaluate(
    (target) =>
      window.activationStatus({
        generation: "22",
        selected: target,
        confirmed: target,
        floorSequence: target.sequence,
      }),
    receipt.slot,
  );
  await panel
    .getByText("The requested target is now the confirmed application.", {
      exact: true,
    })
    .waitFor();
  assert.deepEqual(activations, [
    { slotId: receipt.slot.slotId, expectedGeneration: "11" },
    { slotId: receipt.slot.slotId, expectedGeneration: "20" },
  ]);
  await page.evaluate(() => window.activationStatus({ generation: "30" }));
  phase = "lost-receipt-and-retry";
  mode = "lost";
  await choose();
  await panel
    .getByRole("button", { name: "Upload and verify", exact: true })
    .click();
  await panel.getByText(/The upload result is unknown/).waitFor();
  mode = "retry";
  await panel
    .getByRole("button", { name: "Retry the same archive", exact: true })
    .click();
  await panel
    .getByText("This verified archive is already staged", { exact: true })
    .waitFor();
  phase = "cancel-and-retry";
  mode = "cancel";
  await choose();
  await panel
    .getByRole("button", { name: "Upload and verify", exact: true })
    .click();
  await panel
    .getByText(
      "Archive sent. Waiting for signature verification and staging…",
      { exact: true },
    )
    .waitFor();
  await panel
    .getByRole("button", { name: "Cancel upload", exact: true })
    .click();
  await panel.getByText(/Transfer cancelled/).waitFor();
  mode = "retry";
  await panel
    .getByRole("button", { name: "Retry the same archive", exact: true })
    .click();
  await panel
    .getByText("This verified archive is already staged", { exact: true })
    .waitFor();
  phase = "rejection";
  mode = "rejected";
  await choose();
  await panel
    .getByRole("button", { name: "Upload and verify", exact: true })
    .click();
  await panel
    .getByText("The archive was rejected. Check its format and signature.", {
      exact: true,
    })
    .waitFor();
  phase = "unmount-cancellation";
  mode = "cancel";
  await panel
    .getByRole("button", { name: "Upload and verify", exact: true })
    .click();
  await panel
    .getByText(
      "Archive sent. Waiting for signature verification and staging…",
      { exact: true },
    )
    .waitFor();
  await page.evaluate(() => window.uploadHarness(true, true, false));
  await panel.waitFor({ state: "detached" });
  phase = "capability-gate";
  await page.evaluate(() => window.uploadHarness(false));
  assert.equal(await panel.count(), 0);
  await page.evaluate(() => window.uploadHarness(true, false));
  await panel
    .getByText("Wait for a current update status with no pending operation.", {
      exact: true,
    })
    .waitFor();
  assert.equal(
    await panel
      .getByRole("button", { name: "Upload and verify", exact: true })
      .isEnabled(),
    false,
  );
  // Chromium may replay a POST internally after ERR_EMPTY_RESPONSE. Count
  // browser-visible sends independently; only the deliberately lost response
  // may have transport retries. This must not mask double UI submission.
  assert.equal(logicalUploads, 7);
  assert.equal(
    activations.length,
    2,
    "only the two explicitly confirmed activations may be sent",
  );
  assert.deepEqual(
    uploadModes.filter((value) => value !== "lost"),
    ["success", "retry", "cancel", "retry", "rejected", "cancel"],
  );
  assert.ok(uploadModes.filter((value) => value === "lost").length >= 1);
  for (
    let attempt = 0;
    attempt < 50 &&
    abortedModes.filter((value) => value === "cancel").length < 2;
    attempt += 1
  ) {
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  assert.equal(
    abortedModes.filter((value) => value === "cancel").length,
    2,
    "both explicit cancellation and unmount must close HTTP",
  );
  assert.deepEqual(violations, []);
  assert.deepEqual(errors, []);
  console.log(
    JSON.stringify({
      ok: true,
      scope: "local component plus synthetic HTTP",
      uploads,
      logicalUploads,
      activations: activations.length,
      aborts,
      mobile: true,
    }),
  );
} catch (error) {
  console.error(
    `local application upload failed at ${phase}: ${error.message}`,
  );
  console.error(
    JSON.stringify({
      uploads,
      aborts,
      uploadModes,
      activations: activations.length,
      violations,
    }),
  );
  process.exitCode = 1;
} finally {
  await browser?.close();
  await server.close();
}
