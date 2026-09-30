// Verify the RAW system update switch and API on a board: off by default and
// every changing call refused while off; switched on, an upload is checked
// against the data partition before any byte is kept, nothing staged cannot
// be installed, confirmed or rolled back; switched off again, the calls are
// refused. The GUI password is read from stdin; a receipt is written under
// evidence/private. The probe never installs anything.
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
const receipt = { schemaVersion: 1, probe: "raw-system-update", steps: [] };
const step = (name, value) => receipt.steps.push({ step: name, ...value });
const context = await request.newContext({ ignoreHTTPSErrors: true });
try {
  const login = await context.post(`${url.origin}/api/auth/login`, {
    data: { username, password },
    headers: { Origin: url.origin },
  });
  password = "";
  const csrf = (await login.json()).data.csrfToken;
  const api = async (method, route, data, headers = {}) => {
    const response = await context.fetch(`${url.origin}${route}`, {
      method,
      data,
      headers: { Origin: url.origin, "X-CSRF-Token": csrf, ...headers },
      timeout: 120000,
    });
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
    };
  };
  // Not a bundle: the probe only needs the checks in front of the verifier.
  const bundle = Buffer.alloc(64 * 1024, 0x5a);
  const upload = () =>
    api("POST", "/api/system-update/upload", bundle, {
      "Content-Type": "application/octet-stream",
    });
  const changing = async (prefix) => {
    step(`${prefix}_upload`, await upload());
    step(
      `${prefix}_install`,
      await api("POST", "/api/system-update/install", { confirm: true }),
    );
    step(`${prefix}_confirm`, await api("POST", "/api/system-update/confirm"));
    step(
      `${prefix}_rollback`,
      await api("POST", "/api/system-update/rollback", { confirm: true }),
    );
  };

  step("initial_switch", await api("GET", "/api/system-update/raw-enabled"));
  step("features", await api("GET", "/api/admin/features"));
  step("initial_status", await api("GET", "/api/system-update/status"));
  await changing("off");
  step(
    "switch_on",
    await api("POST", "/api/system-update/raw-enabled", { enabled: true }),
  );
  await changing("on");
  step("on_cancel", await api("POST", "/api/system-update/cancel"));
  step("on_status", await api("GET", "/api/system-update/status"));
  step(
    "switch_off",
    await api("POST", "/api/system-update/raw-enabled", { enabled: false }),
  );
  step("final_switch", await api("GET", "/api/system-update/raw-enabled"));
  step("final_upload", await upload());

  const by = Object.fromEntries(
    receipt.steps.map((entry) => [entry.step, entry]),
  );
  const refusedOff = (entry) => entry.http === 403;
  receipt.result =
    by.initial_switch.data?.enabled === false &&
    by.features.data?.rawSystemUpdate === false &&
    by.initial_status.data?.phase === "none" &&
    ["off_upload", "off_install", "off_confirm", "off_rollback", "final_upload"]
      .map((name) => by[name])
      .every(refusedOff) &&
    by.switch_on.data?.enabled === true &&
    // This hand-built board's data partition cannot hold the update: the
    // helper refuses before receiving the body.
    by.on_upload.code === -409 &&
    /data partition needs/.test(by.on_upload.msg ?? "") &&
    by.on_install.code === -409 &&
    by.on_confirm.code === -409 &&
    by.on_rollback.code === -409 &&
    by.on_cancel.code === -409 &&
    by.on_status.data?.phase === "none" &&
    by.switch_off.data?.enabled === false &&
    by.final_switch.data?.enabled === false
      ? "passed"
      : "failed";
} finally {
  password = "";
  await context.dispose();
  fs.writeFileSync(output, JSON.stringify(receipt, null, 1), { mode: 0o600 });
  console.log(JSON.stringify(receipt));
}
