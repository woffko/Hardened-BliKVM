// Verify the reference Tailscale actions on the device: stop, start,
// restart, the login URL (the login itself is left to the operator),
// uninstall and install from Tailscale's apt repository. The board ends
// installed and running as before. The GUI password is read from stdin; a
// receipt is written under evidence/private.
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

const { request } = await import(
  pathToFileURL(
    process.env.PLAYWRIGHT_MODULE ??
      "/home/w0w/.local/share/playwright/node_modules/playwright/index.mjs",
  ).href
);
const context = await request.newContext({ ignoreHTTPSErrors: true });
const receipt = { schemaVersion: 1, probe: "tailscale", steps: [] };
const step = (name, value) => receipt.steps.push({ step: name, ...value });
try {
  const login = await context.post(`${url.origin}/api/auth/login`, {
    data: { username, password },
    headers: { Origin: url.origin },
  });
  password = "";
  const csrf = (await login.json()).data.csrfToken;
  const headers = { Origin: url.origin, "X-CSRF-Token": csrf };
  const api = async (method, route) => {
    const started = Date.now();
    const response = await context.fetch(
      `${url.origin}/api/extensions/tailscale/${route}`,
      { method, headers, timeout: 660000 },
    );
    let body = null;
    try {
      body = await response.json();
    } catch {
      body = null;
    }
    return {
      http: response.status(),
      code: body?.code,
      msg: body?.msg,
      data: body?.data,
      seconds: Math.round((Date.now() - started) / 1000),
    };
  };
  const status = async () => (await api("GET", "status")).data?.state;
  for (const [name, method, route] of [
    ["stop", "POST", "stop"],
    ["start", "POST", "start"],
    ["restart", "POST", "restart"],
    ["login", "POST", "login"],
    ["uninstall", "POST", "uninstall"],
    ["install", "POST", "install"],
  ]) {
    const result = await api(method, route);
    if (result.data?.url)
      result.data.url = result.data.url.replace(/\/a\/.*/, "/a/…");
    step(name, { ...result, state: await status() });
  }
  const by = Object.fromEntries(
    receipt.steps.map((entry) => [entry.step, entry]),
  );
  receipt.result =
    by.stop.code === 0 &&
    by.stop.state === "notRunning" &&
    by.start.code === 0 &&
    by.start.state === "notLogin" &&
    by.restart.code === 0 &&
    by.login.code === 0 &&
    /^https:\/\/login\.tailscale\.com\//.test(by.login.data?.url ?? "") &&
    by.uninstall.code === 0 &&
    by.uninstall.state === "notInstall" &&
    by.install.code === 0 &&
    by.install.state === "notLogin"
      ? "passed"
      : "failed";
} finally {
  await context.dispose();
  fs.writeFileSync(output, JSON.stringify(receipt, null, 1), { mode: 0o600 });
  console.log(JSON.stringify(receipt));
}
