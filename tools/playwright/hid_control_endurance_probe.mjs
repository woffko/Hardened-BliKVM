// Hold remote control through the real GUI in headless Chromium and move the
// mouse over the screen for a while, recording every HID/focus API response
// and whether the page turns control off. The GUI password is read from stdin.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const [baseUrl, username, outputValue, seconds = "120"] = process.argv.slice(2);
const url = new URL(baseUrl);
assert.equal(url.protocol, "https:");
const output = path.resolve(outputValue);
assert.equal(path.dirname(output), path.resolve("evidence/private"));
assert.ok(!fs.existsSync(output));
let password = "";
for await (const chunk of process.stdin) password += chunk.toString("utf8");
password = password.replace(/[\r\n]+$/, "");
assert.ok(password.length >= 8);

const { chromium } = await import(
  pathToFileURL(
    process.env.PLAYWRIGHT_MODULE ??
      "/home/w0w/.local/share/playwright/node_modules/playwright/index.mjs",
  ).href
);
const browser = await chromium.launch({
  headless: true,
  executablePath:
    process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH ??
    "/home/w0w/.cache/ms-playwright/chromium-1228/chrome-linux64/chrome",
});
const started = Date.now();
const at = () => Math.round((Date.now() - started) / 100) / 10;
const receipt = {
  schemaVersion: 1,
  probe: "hid-control-endurance",
  events: [],
};
const event = (value) => receipt.events.push({ t: at(), ...value });
try {
  const context = await browser.newContext({
    ignoreHTTPSErrors: true,
    viewport: { width: 1280, height: 800 },
  });
  const login = await context.request.post(`${url.origin}/api/auth/login`, {
    data: { username, password },
    headers: { Origin: url.origin },
  });
  password = "";
  receipt.login = login.status();
  const page = await context.newPage();
  page.on("response", async (response) => {
    const route = new URL(response.url()).pathname;
    if (!route.startsWith("/api/hid")) return;
    let body = null;
    try {
      body = await response.json();
    } catch {
      body = null;
    }
    const data = body?.data ?? {};
    event({
      route,
      method: response.request().method(),
      http: response.status(),
      code: body?.code,
      applied: data.applied,
      attested: data.attested,
      rejection: data.rejection,
    });
  });
  page.on("console", (message) => {
    if (message.type() === "error")
      event({ console: message.text().slice(0, 200) });
  });
  page.on("websocket", (socket) => {
    event({ websocket: new URL(socket.url()).pathname });
    socket.on("close", () =>
      event({ websocketClosed: new URL(socket.url()).pathname }),
    );
  });
  await page.goto(`${url.origin}/`);
  await page.waitForSelector("#screen", { timeout: 20000 });
  const toggle = page.locator('button[aria-label$="remote control"]');
  await toggle.first().click();
  await page.getByRole("button", { name: "Confirm and control" }).click();
  event({ control: await toggle.first().getAttribute("aria-pressed") });
  const box = await page.locator("#screen").boundingBox();
  let lastState = "true";
  const deadline = Date.now() + Number(seconds) * 1000;
  let step = 0;
  while (Date.now() < deadline) {
    step += 1;
    const x = box.x + box.width * (0.3 + 0.2 * Math.sin(step / 7));
    const y = box.y + box.height * (0.4 + 0.2 * Math.cos(step / 5));
    await page.mouse.move(x, y, { steps: 3 });
    await page.waitForTimeout(250);
    const state = await toggle.first().getAttribute("aria-pressed");
    if (state !== lastState) {
      event({ control: state });
      lastState = state;
    }
  }
  receipt.result = lastState === "true" ? "held" : "dropped";
} finally {
  await browser.close();
  fs.writeFileSync(output, JSON.stringify(receipt, null, 1), { mode: 0o600 });
  console.log(
    JSON.stringify({ result: receipt.result, events: receipt.events.length }),
  );
}
