// Run one shell script as root on the board through the default-off web
// terminal: switch the terminal on, unlock it with the GUI password, send the
// script, print its output, and switch the terminal off again (it is left off
// even when it was on before). The GUI password is read from stdin and never
// printed. The script must be small (it is sent as one base64 line).
//
// Usage: node terminal_run.mjs BASE_URL USERNAME SCRIPT_FILE
import assert from "node:assert/strict";
import fs from "node:fs";
import { pathToFileURL } from "node:url";

const [baseUrl, username, scriptPath] = process.argv.slice(2);
const url = new URL(baseUrl);
assert.equal(url.protocol, "https:");
const script = fs.readFileSync(scriptPath);
const encoded = script.toString("base64");
assert.ok(encoded.length < 3000, "the script is too large for one terminal line");
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
let exitCode = 1;
let api = null;
try {
  const context = await browser.newContext({ ignoreHTTPSErrors: true });
  const page = await context.newPage();
  const login = await context.request.post(`${url.origin}/api/auth/login`, {
    data: { username, password },
    headers: { Origin: url.origin },
  });
  const csrf = (await login.json()).data.csrfToken;
  await page.goto(`${url.origin}/`);
  api = async (method, route, data) => {
    const response = await context.request.fetch(`${url.origin}${route}`, {
      method,
      data,
      headers: { Origin: url.origin, "X-CSRF-Token": csrf },
    });
    return (await response.json().catch(() => ({}))) ?? {};
  };
  const switched = await api("POST", "/api/vm/terminal/enabled", { enabled: true });
  assert.equal(switched.data?.enabled, true, "the terminal could not be switched on");
  const unlocked = await api("POST", "/api/vm/terminal/unlock", { username, password });
  password = "";
  const ticket = unlocked.data?.ticket;
  assert.ok(ticket, "the terminal did not unlock");
  // The markers are computed by the shell, so the echoed command line never
  // matches them.
  const result = await page.evaluate(
    async ({ ticket, encoded }) =>
      await new Promise((resolve) => {
        const socket = new WebSocket(
          `wss://${location.host}/api/vm/terminal?ticket=${encodeURIComponent(ticket)}`,
        );
        socket.binaryType = "arraybuffer";
        let text = "";
        const finish = (timedOut) => {
          socket.close();
          resolve({ text, timedOut });
        };
        socket.onopen = () => {
          socket.send(new Blob([JSON.stringify({ rows: 50, cols: 200 })]));
          socket.send(
            `stty -echo; printf 'BEGIN-%s\\n' $((6*7)); echo ${encoded} | base64 -d > /tmp/.agent-run.sh; sh /tmp/.agent-run.sh; printf 'END-%s-%s\\n' $((6*7)) $?; rm -f /tmp/.agent-run.sh\r`,
          );
        };
        socket.onmessage = (event) => {
          text += new TextDecoder().decode(event.data);
          if (/END-42-\d+/.test(text)) finish(false);
        };
        socket.onclose = () => resolve({ text, timedOut: false });
        setTimeout(() => finish(true), 600000);
      }),
    { ticket, encoded },
  );
  const begin = result.text.indexOf("BEGIN-42");
  const end = result.text.search(/END-42-\d+/);
  const body = begin >= 0 && end > begin ? result.text.slice(begin + 9, end) : result.text;
  process.stdout.write(body.replace(/\r/g, ""));
  const status = result.text.match(/END-42-(\d+)/);
  exitCode = status ? Number(status[1]) : 124;
  if (result.timedOut) console.error("terminal_run: timed out");
} finally {
  password = "";
  if (api) {
    const off = await api("POST", "/api/vm/terminal/enabled", { enabled: false }).catch(() => null);
    if (off?.data?.enabled !== false) console.error("terminal_run: the terminal may still be on");
  }
  await browser.close();
}
process.exit(exitCode);
