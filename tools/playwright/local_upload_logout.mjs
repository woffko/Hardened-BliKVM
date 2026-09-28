// Actual upload/logout components and XHR; synthetic local auth and staging.
// No device, credential, signed payload, or privileged worker is used.
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { createServer } from "../../web/node_modules/vite/dist/node/index.js";
import { chromium } from "/home/w0w/.local/share/playwright/node_modules/playwright/index.mjs";

const webRoot = fileURLToPath(new URL("../../web", import.meta.url));
process.chdir(webRoot);
const archive = Buffer.alloc(256 * 1024, 42);
let sessionActive = true;
let uploadCount = 0;
let logoutCount = 0;
let incompleteUploads = 0;
let abortedResponses = 0;
let completedBodies = 0;
const violations = [];
const held = new Set();
const server = await createServer({
  root: webRoot,
  configFile: `${webRoot}/vite.config.ts`,
  server: { host: "127.0.0.1", port: 0, hmr: false, open: false },
  logLevel: "error",
  plugins: [
    {
      name: "upload-logout-fixture",
      configureServer(server) {
        server.middlewares.use(async (request, response, next) => {
          const route = request.url?.split("?")[0];
          if (route === "/__upload-logout") {
            sessionActive = true;
            response.setHeader("Content-Type", "text/html");
            response.end(
              await server.transformIndexHtml(
                route,
                `<!doctype html><html><body><div id="root"></div><script type="module">
          import React from 'react';
          import ReactDOM from 'react-dom/client';
          import { MemoryRouter, Routes, Route } from 'react-router-dom';
          import { BliApplicationUpload } from '/src/pages/desktop/menu/settings/update/bli-upload.tsx';
          import { Logout } from '/src/pages/desktop/menu/settings/account/logout.tsx';
          import '/src/i18n/index.ts';
          import '/src/assets/styles/index.css';
          const slot = {slotId:'app-00000000000000000008-'+'a'.repeat(64),version:'fixture',sequence:'8',minimumSystemSequence:'1'};
          const status = {schemaVersion:1,generation:'40',floorSequence:'8',systemSequence:'1',selected:slot,confirmed:slot,previous:null,pending:null,lastRollback:null};
          document.cookie='hardened-blikvm-csrf=synthetic-browser-fixture; SameSite=Strict; Path=/';
          const active = React.createElement(React.Fragment, null,
            React.createElement(BliApplicationUpload,{enabled:true,ready:true,status,refresh:()=>{}}), React.createElement(Logout));
          ReactDOM.createRoot(document.getElementById('root')).render(React.createElement(MemoryRouter,null,
            React.createElement(Routes,null,
              React.createElement(Route,{path:'/',element:active}),
              React.createElement(Route,{path:'/auth/login',element:React.createElement('div',{'data-testid':'logged-out'},'Logged out')}))));
        </script></body></html>`,
              ),
            );
            return;
          }
          if (
            route === "/api/application-update/upload" &&
            request.method === "POST"
          ) {
            uploadCount++;
            if (
              !sessionActive ||
              request.headers["x-csrf-token"] !== "synthetic-browser-fixture"
            )
              violations.push("upload auth");
            if (request.headers["content-length"] !== String(archive.length))
              violations.push("length");
            let received = 0;
            const chunks = [];
            held.add(response);
            request.on("data", (chunk) => {
              received += chunk.length;
              chunks.push(chunk);
            });
            request.on("aborted", () => {
              if (received < archive.length) incompleteUploads++;
            });
            request.on("error", () => {});
            request.on("end", () => {
              completedBodies++;
              if (!Buffer.concat(chunks).equals(archive))
                violations.push("bytes");
              // Deliberately withhold verification: logout must unmount/cancel the
              // real upload hook, not receive a fixture-forced connection close.
            });
            response.on("close", () => {
              if (!response.writableEnded) abortedResponses++;
              held.delete(response);
            });
            return;
          }
          if (route === "/api/auth/logout" && request.method === "POST") {
            logoutCount++;
            if (
              !sessionActive ||
              request.headers["x-csrf-token"] !== "synthetic-browser-fixture"
            )
              violations.push("logout auth");
            sessionActive = false;
            response.setHeader("Content-Type", "application/json");
            response.end(JSON.stringify({ code: 0, msg: "success" }));
            return;
          }
          if (
            route === "/api/application-update/status" &&
            request.method === "GET"
          ) {
            response.setHeader("Content-Type", "application/json");
            response
              .writeHead(sessionActive ? 200 : 401)
              .end(JSON.stringify({ code: sessionActive ? 0 : -401 }));
            return;
          }
          if (route?.startsWith("/api/")) {
            violations.push("unexpected API");
            response.writeHead(404).end();
            return;
          }
          next();
        });
      },
    },
  ],
});

let browser;
let phase = "startup";
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
    viewport: { width: 1280, height: 900 },
  });
  page.setDefaultTimeout(15000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.name));
  await page.goto(`http://127.0.0.1:${address.port}/__upload-logout`);
  const panel = page.getByTestId("bli-application-upload");
  await panel.waitFor();
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("Network.enable");
  await cdp.send("Network.emulateNetworkConditions", {
    offline: false,
    latency: 0,
    downloadThroughput: -1,
    uploadThroughput: 65536,
  });
  await panel.getByTestId("application-archive").setInputFiles({
    name: "fixture.tar",
    mimeType: "application/octet-stream",
    buffer: archive,
  });
  phase = "partial-progress";
  await panel
    .getByRole("button", { name: "Upload and verify", exact: true })
    .click();
  await page.waitForFunction(() => {
    const value = Number(
      document
        .querySelector('[role="progressbar"]')
        ?.getAttribute("aria-valuenow"),
    );
    return value > 0 && value < 100;
  });
  const partial = Number(
    await panel.getByRole("progressbar").getAttribute("aria-valuenow"),
  );
  assert.ok(partial > 0 && partial < 100);
  phase = "cancel-partial";
  await panel
    .getByRole("button", { name: "Cancel upload", exact: true })
    .click();
  await panel.getByText(/Transfer cancelled/).waitFor();
  phase = "retry-and-logout";
  await panel
    .getByRole("button", { name: "Retry the same archive", exact: true })
    .click();
  await panel
    .getByText(
      "Archive sent. Waiting for signature verification and staging…",
      { exact: true },
    )
    .waitFor();
  assert.equal(
    await panel.getByRole("progressbar").getAttribute("aria-valuenow"),
    "100",
  );
  // The browser counts a throttled body as sent before the fixture has read
  // all of it; log out only once verification is really pending.
  for (let count = 0; count < 250 && completedBodies < 1; count++)
    await new Promise((resolve) => setTimeout(resolve, 20));
  // Ant Design's labelled logout icon contributes to the accessible name.
  await page.getByRole("button", { name: /Logout$/ }).click();
  await page.getByRole("button", { name: "Yes", exact: true }).click();
  await page.getByTestId("logged-out").waitFor();
  assert.equal(await panel.count(), 0);
  const status = await page.evaluate(
    async () =>
      (
        await fetch("/api/application-update/status", {
          credentials: "include",
        })
      ).status,
  );
  assert.equal(status, 401);
  for (let count = 0; count < 50 && abortedResponses < 2; count++)
    await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(uploadCount, 2, "uploads");
  assert.equal(logoutCount, 1, "logouts");
  assert.equal(incompleteUploads, 1, "incomplete uploads");
  assert.equal(completedBodies, 1, "completed bodies");
  assert.equal(abortedResponses, 2, "aborted responses");
  assert.equal(held.size, 0, "held responses");
  assert.deepEqual(violations, []);
  assert.deepEqual(errors, []);
  console.log(
    JSON.stringify({
      result: "passed",
      scope: "local components and synthetic auth/staging",
      partialProgress: partial,
      cancelledIncompleteUpload: true,
      logoutDuringVerification: true,
      activationRequests: 0,
    }),
  );
} catch (error) {
  console.error(
    JSON.stringify({ result: "failed", phase, error: error.message }),
  );
  process.exitCode = 1;
} finally {
  for (const response of held) response.destroy();
  await browser?.close();
  await server.close();
}
