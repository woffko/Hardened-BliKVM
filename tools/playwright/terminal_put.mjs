// Copy one file to the board through the default-off web terminal: the file
// goes as base64 in bounded chunks, each acknowledged before the next, and is
// moved into place only when its SHA-256 matches. The terminal is switched on
// for the copy and off afterwards. The GUI password is read from stdin.
//
// Usage: node terminal_put.mjs BASE_URL USERNAME SOURCE DESTINATION
import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import { pathToFileURL } from "node:url";

const [baseUrl, username, source, destination] = process.argv.slice(2);
const url = new URL(baseUrl);
assert.equal(url.protocol, "https:");
assert.ok(
  /^\/[A-Za-z0-9._/-]+$/.test(destination),
  "the destination must be a plain absolute path",
);
const content = fs.readFileSync(source);
const digest = crypto.createHash("sha256").update(content).digest("hex");
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
let api = null;
let exitCode = 1;
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
  assert.equal(
    (await api("POST", "/api/vm/terminal/enabled", { enabled: true })).data
      ?.enabled,
    true,
  );
  const ticket = (
    await api("POST", "/api/vm/terminal/unlock", { username, password })
  ).data?.ticket;
  password = "";
  assert.ok(ticket, "the terminal did not unlock");
  await page.evaluate(async (ticket) => {
    await new Promise((resolve, reject) => {
      const socket = new WebSocket(
        `wss://${location.host}/api/vm/terminal?ticket=${encodeURIComponent(ticket)}`,
      );
      socket.binaryType = "arraybuffer";
      window.__text = "";
      socket.onmessage = (event) => {
        window.__text += new TextDecoder().decode(event.data);
      };
      socket.onopen = () => {
        socket.send(new Blob([JSON.stringify({ rows: 50, cols: 250 })]));
        socket.send("stty -echo; export TERM=dumb\r");
        window.__socket = socket;
        setTimeout(resolve, 1500);
      };
      socket.onerror = () => reject(new Error("terminal socket failed"));
    });
  }, ticket);
  // Sends one shell line and waits for a marker the shell computes itself.
  const run = (line, tag, timeoutMs = 120000) =>
    page.evaluate(
      async ({ line, tag, timeoutMs }) => {
        const start = window.__text.length;
        window.__socket.send(`${line}; printf 'A%s-%s\\n' ${tag} $((6*7))\r`);
        const marker = `A${tag}-42`;
        const deadline = Date.now() + timeoutMs;
        while (Date.now() < deadline) {
          const text = window.__text.slice(start);
          const at = text.indexOf(marker);
          if (at >= 0) return text.slice(0, at).replace(/\r/g, "").trim();
          await new Promise((resolve) => setTimeout(resolve, 100));
        }
        return "TIMEOUT";
      },
      { line, tag, timeoutMs },
    );
  const part = `${destination}.part`;
  await run(`rm -f '${part}'`, "start");
  const encoded = content.toString("base64");
  const chunk = 3000;
  for (
    let offset = 0, index = 0;
    offset < encoded.length;
    offset += chunk, index += 1
  ) {
    const piece = encoded.slice(offset, offset + chunk);
    const reply = await run(
      `printf '%s' '${piece}' | base64 -d >> '${part}'`,
      `c${index}`,
    );
    if (reply === "TIMEOUT") throw new Error(`chunk ${index} timed out`);
    if (index % 200 === 0)
      process.stderr.write(`sent ${offset + piece.length}/${encoded.length}\n`);
  }
  const check = await run(`sha256sum '${part}' | cut -c1-64`, "sum");
  if (!check.includes(digest)) throw new Error("the copy does not match");
  await run(`chmod 0600 '${part}' && mv -f '${part}' '${destination}'`, "move");
  console.log(`${digest}  ${destination}`);
  exitCode = 0;
} finally {
  password = "";
  if (api) {
    const off = await api("POST", "/api/vm/terminal/enabled", {
      enabled: false,
    }).catch(() => null);
    if (off?.data?.enabled !== false)
      console.error("terminal_put: the terminal may still be on");
  }
  await browser.close();
}
process.exit(exitCode);
