// Verify the KVM self-reboot from Device settings: the API refuses without
// the operator's confirmation, reboots with it, and the board serves the
// GUI again with a fresh uptime. The GUI password is read from stdin; a
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
const receipt = { schemaVersion: 1, probe: "kvm-reboot", steps: [] };
const step = (name, value) => receipt.steps.push({ step: name, ...value });
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const session = async () => {
  const context = await request.newContext({ ignoreHTTPSErrors: true });
  const login = await context.post(`${url.origin}/api/auth/login`, {
    data: { username, password },
    headers: { Origin: url.origin },
  });
  const csrf = (await login.json()).data.csrfToken;
  const api = async (method, route, data) => {
    const response = await context.fetch(`${url.origin}${route}`, {
      method,
      data,
      headers: { Origin: url.origin, "X-CSRF-Token": csrf },
      timeout: 30000,
    });
    let body = null;
    try {
      body = await response.json();
    } catch {
      body = null;
    }
    return { http: response.status(), code: body?.code, data: body?.data };
  };
  return { context, api };
};
try {
  const before = await session();
  const info = await before.api("GET", "/api/vm/info");
  step("before", { uptime: info.data?.uptime });
  const capabilities = await before.api("GET", "/api/capabilities");
  step("capability", {
    reboot: capabilities.data?.board?.features?.device_reboot?.availability,
  });
  step(
    "without_confirmation",
    await before.api("POST", "/api/vm/system/reboot", {}),
  );
  step(
    "reboot",
    await before.api("POST", "/api/vm/system/reboot", { confirm: true }),
  );
  await before.context.dispose();
  const started = Date.now();
  let down = false;
  let back = null;
  while (Date.now() - started < 600000) {
    await sleep(5000);
    try {
      const probe = await request.newContext({ ignoreHTTPSErrors: true });
      const health = await probe.get(`${url.origin}/api/health`, {
        timeout: 4000,
      });
      await probe.dispose();
      if (health.status() === 200 && down) {
        back = Math.round((Date.now() - started) / 1000);
        break;
      }
      if (health.status() !== 200) down = true;
    } catch {
      down = true;
    }
  }
  step("return", { wentDown: down, backAfterSeconds: back });
  await sleep(10000);
  const after = await session();
  password = "";
  const afterInfo = await after.api("GET", "/api/vm/info");
  step("after", { uptime: afterInfo.data?.uptime });
  step("features", await after.api("GET", "/api/admin/features"));
  step("ssh", await after.api("GET", "/api/vm/ssh"));
  step("lock", await after.api("GET", "/api/vm/session-lock"));
  step("tailscale", await after.api("GET", "/api/extensions/tailscale/status"));
  await after.context.dispose();
  const by = Object.fromEntries(
    receipt.steps.map((entry) => [entry.step, entry]),
  );
  receipt.result =
    by.capability.reboot === "read_write" &&
    by.without_confirmation.data?.applied !== true &&
    by.reboot.data?.applied === true &&
    by.return.wentDown &&
    by.return.backAfterSeconds !== null &&
    by.after.uptime < by.before.uptime &&
    by.after.uptime < 600 &&
    by.ssh.data?.enabled === true
      ? "passed"
      : "failed";
} finally {
  password = "";
  fs.writeFileSync(output, JSON.stringify(receipt, null, 1), { mode: 0o600 });
  console.log(JSON.stringify(receipt));
}
