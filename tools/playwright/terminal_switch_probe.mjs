// Verify the default-off web terminal on the device: refused while switched
// off, a working root shell after the switch and password unlock, refused
// again once switched off. Leaves the terminal switched off. The GUI password
// is read from stdin; a receipt is written under evidence/private.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const [baseUrl, username, outputValue] = process.argv.slice(2);
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
const receipt = { schemaVersion: 1, probe: "terminal-switch", steps: [] };
const step = (name, value) => receipt.steps.push({ step: name, ...value });
try {
  const context = await browser.newContext({ ignoreHTTPSErrors: true });
  const page = await context.newPage();
  const login = await context.request.post(`${url.origin}/api/auth/login`, {
    data: { username, password },
    headers: { Origin: url.origin },
  });
  const csrf = (await login.json()).data.csrfToken;
  await page.goto(`${url.origin}/`);
  const api = async (method, route, data) => {
    const response = await context.request.fetch(`${url.origin}${route}`, {
      method,
      data,
      headers: { Origin: url.origin, "X-CSRF-Token": csrf },
    });
    let body = null;
    try {
      body = await response.json();
    } catch {
      body = null;
    }
    return { http: response.status(), code: body?.code, data: body?.data };
  };
  const unlock = () =>
    api("POST", "/api/vm/terminal/unlock", { username, password });

  step("initial", await api("GET", "/api/admin/features"));
  step("unlock_while_off", await unlock());
  step(
    "switch_on",
    await api("POST", "/api/vm/terminal/enabled", { enabled: true }),
  );
  const unlocked = await unlock();
  step("unlock_while_on", { http: unlocked.http, code: unlocked.code });
  const ticket = unlocked.data?.ticket;
  const shell = await page.evaluate(async (ticket) => {
    return await new Promise((resolve) => {
      const socket = new WebSocket(
        `wss://${location.host}/api/vm/terminal?ticket=${encodeURIComponent(ticket)}`,
      );
      socket.binaryType = "arraybuffer";
      let text = "";
      const done = (result) => {
        socket.close();
        resolve(result);
      };
      socket.onopen = () => {
        socket.send(new Blob([JSON.stringify({ rows: 30, cols: 100 })]));
        socket.send("stty size; echo term-$((6*7)); id -u\r");
      };
      socket.onmessage = (event) => {
        text += new TextDecoder().decode(event.data);
        if (/term-42/.test(text) && /\n0\r?\n/.test(text)) {
          done({
            opened: true,
            marker: true,
            root: true,
            size: /30 100/.test(text),
          });
        }
      };
      socket.onclose = () => resolve({ opened: false, text: text.slice(-200) });
      setTimeout(
        () =>
          done({
            opened: true,
            marker: /term-42/.test(text),
            text: text.slice(-300),
          }),
        15000,
      );
    });
  }, ticket);
  step("shell", shell);
  const reuse = await page.evaluate(async (ticket) => {
    return await new Promise((resolve) => {
      const socket = new WebSocket(
        `wss://${location.host}/api/vm/terminal?ticket=${encodeURIComponent(ticket)}`,
      );
      socket.onopen = () => resolve("opened");
      socket.onerror = () => resolve("refused");
    });
  }, ticket);
  step("ticket_reuse", { result: reuse });
  // A shell left open across the switch-off must be closed by the server.
  const second = await unlock();
  const held = await page.evaluate(async (ticket) => {
    return await new Promise((resolve) => {
      const socket = new WebSocket(
        `wss://${location.host}/api/vm/terminal?ticket=${encodeURIComponent(ticket)}`,
      );
      window.__heldTerminal = { closedAt: null, code: null };
      socket.onclose = (event) => {
        window.__heldTerminal.closedAt = Date.now();
        window.__heldTerminal.code = event.code;
      };
      socket.onopen = () => resolve("open");
      socket.onerror = () => resolve("refused");
    });
  }, second.data?.ticket);
  step("held_terminal", { result: held });
  const offAt = Date.now();
  step(
    "switch_off",
    await api("POST", "/api/vm/terminal/enabled", { enabled: false }),
  );
  await page.waitForTimeout(3000);
  const closed = await page.evaluate(() => window.__heldTerminal);
  step("held_terminal_closed", {
    closedWithinMs: closed.closedAt ? closed.closedAt - offAt : null,
    code: closed.code,
  });
  step("unlock_after_off", await unlock());
  step("final", await api("GET", "/api/admin/features"));
  password = "";
  const byName = Object.fromEntries(
    receipt.steps.map((entry) => [entry.step, entry]),
  );
  receipt.result =
    byName.initial.data?.terminal === false &&
    byName.unlock_while_off.http === 403 &&
    byName.switch_on.data?.enabled === true &&
    byName.unlock_while_on.code === 0 &&
    byName.shell.marker === true &&
    byName.shell.root === true &&
    byName.ticket_reuse.result === "refused" &&
    byName.held_terminal.result === "open" &&
    byName.held_terminal_closed.closedWithinMs !== null &&
    byName.held_terminal_closed.closedWithinMs <= 3000 &&
    byName.switch_off.data?.enabled === false &&
    byName.unlock_after_off.http === 403 &&
    byName.final.data?.terminal === false
      ? "passed"
      : "failed";
} finally {
  await browser.close();
  fs.writeFileSync(output, JSON.stringify(receipt, null, 1), { mode: 0o600 });
  console.log(JSON.stringify(receipt));
}
