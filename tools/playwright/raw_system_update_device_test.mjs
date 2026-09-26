// Device test of the RAW system update on a card with the new layout
// (docs/design/system-raw-update-v1.md), one step per run so a test can
// resume after any reboot:
//
//   install   switch the feature on, upload BUNDLE unless an update is
//             already staged, install, wait for the write and the new boot,
//             and compare what must survive
//   rollback  ask for the previous system, wait for the restore, compare
//   reboot    reboot through the system action (counts a boot attempt)
//   confirm   keep the booted system; the sequence becomes the floor
//   off       switch the feature off
//
// The GUI password is read from stdin; a receipt is written under
// evidence/private.
//
// Usage: node raw_system_update_device_test.mjs BASE_URL USERNAME ACTION OUTPUT [BUNDLE]
import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import https from "node:https";
import path from "node:path";
import tls from "node:tls";
import { execFileSync } from "node:child_process";

const [baseUrl, username, action, outputValue, bundlePath] = process.argv.slice(2);
assert.ok(["install", "rollback", "reboot", "confirm", "off"].includes(action), "unknown action");
const url = new URL(baseUrl);
assert.equal(url.protocol, "https:");
const output = path.resolve(outputValue);
assert.equal(path.dirname(output), path.resolve("evidence/private"));
assert.ok(!fs.existsSync(output));
let password = "";
for await (const chunk of process.stdin) password += chunk.toString("utf8");
password = password.replace(/[\r\n]+$/, "");
assert.ok(password.length >= 8);

const receipt = { schemaVersion: 1, probe: "raw-system-update-device", action, steps: [] };
const step = (name, value) => {
  receipt.steps.push({ step: name, at: new Date().toISOString(), ...value });
  console.log(JSON.stringify({ step: name, ...value }).slice(0, 400));
};
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const agent = new https.Agent({ rejectUnauthorized: false, keepAlive: false });

function send(method, route, { body, headers = {}, stream, timeoutMs = 60000 } = {}) {
  return new Promise((resolve, reject) => {
    const request = https.request(
      {
        host: url.hostname,
        port: url.port || 443,
        path: route,
        method,
        agent,
        headers: { Origin: url.origin, ...headers },
        timeout: timeoutMs,
      },
      (response) => {
        const chunks = [];
        response.on("data", (chunk) => chunks.push(chunk));
        response.on("end", () => {
          const text = Buffer.concat(chunks).toString("utf8");
          let json = null;
          try {
            json = JSON.parse(text);
          } catch {
            json = null;
          }
          resolve({ http: response.statusCode, headers: response.headers, json });
        });
      },
    );
    request.on("timeout", () => request.destroy(new Error("timeout")));
    request.on("error", reject);
    if (stream) {
      stream.pipe(request);
    } else {
      if (body !== undefined) request.write(body);
      request.end();
    }
  });
}

let cookie = "";
let csrf = "";
async function login() {
  const response = await send("POST", "/api/auth/login", {
    body: JSON.stringify({ username, password }),
    headers: { "Content-Type": "application/json" },
  });
  cookie = (response.headers["set-cookie"] ?? []).map((value) => value.split(";")[0]).join("; ");
  csrf = response.json?.data?.csrfToken ?? "";
  return response.json?.code === 0;
}
async function api(method, route, data, extra = {}) {
  const response = await send(method, route, {
    body: data === undefined ? undefined : JSON.stringify(data),
    headers: {
      Cookie: cookie,
      "X-CSRF-Token": csrf,
      ...(data === undefined ? {} : { "Content-Type": "application/json" }),
    },
    ...extra,
  });
  return { http: response.http, code: response.json?.code, msg: response.json?.msg, data: response.json?.data };
}
function certificateFingerprint() {
  return new Promise((resolve) => {
    const socket = tls.connect({ host: url.hostname, port: 443, rejectUnauthorized: false }, () => {
      const certificate = socket.getPeerCertificate();
      socket.end();
      resolve(certificate?.fingerprint256 ?? null);
    });
    socket.on("error", () => resolve(null));
    socket.setTimeout(8000, () => {
      socket.destroy();
      resolve(null);
    });
  });
}
function hostKeys() {
  try {
    const scanned = execFileSync("ssh-keyscan", ["-T", "8", url.hostname], {
      stdio: ["ignore", "pipe", "ignore"],
    }).toString();
    return scanned
      .split("\n")
      .filter((line) => line && !line.startsWith("#"))
      .map((line) => line.split(" ").slice(1).join(" "))
      .map((key) => crypto.createHash("sha256").update(key).digest("hex").slice(0, 16))
      .sort();
  } catch {
    return [];
  }
}
async function snapshot(name) {
  const application = await api("GET", "/api/application-update/status");
  const value = {
    certificate: await certificateFingerprint(),
    hostKeys: hostKeys(),
    // The account name only: the session fields change with every login.
    account: (await api("GET", "/api/auth/account")).data?.username ?? null,
    slot: application.data?.confirmed?.slotId,
    features: (await api("GET", "/api/admin/features")).data,
    hostname: (await api("GET", "/api/vm/hostname")).data,
    sessionLock: (await api("GET", "/api/vm/session-lock")).data,
    ssh: (await api("GET", "/api/vm/ssh")).data,
    uptime: (await api("GET", "/api/vm/info")).data?.uptime,
  };
  step(name, value);
  return value;
}
async function waitForBoard(label, limitMs) {
  const started = Date.now();
  let down = false;
  while (Date.now() - started < limitMs) {
    await sleep(15000);
    let healthy = false;
    try {
      healthy = (await send("GET", "/api/health", { timeoutMs: 8000 })).http === 200;
    } catch {
      healthy = false;
    }
    if (!healthy) down = true;
    if (healthy && down) {
      step(label, { wentDown: true, backAfterSeconds: Math.round((Date.now() - started) / 1000) });
      return true;
    }
  }
  step(label, { wentDown: down, backAfterSeconds: null });
  return false;
}

async function statusAfterBoot() {
  // The boot report marks the update once the web stack answers.
  let status = null;
  for (let attempt = 0; attempt < 40; attempt += 1) {
    status = await api("GET", "/api/system-update/status");
    if (status.data?.phase !== "written") break;
    await sleep(15000);
  }
  return status;
}
async function rebootAndCompare(label, trigger, expected) {
  const before = await snapshot("before");
  const triggered = await trigger();
  step(label, triggered);
  assert.equal(triggered.data?.applied, true, "the reboot was not applied");
  // Backup, write, read-back and grow of the root take up to about 30 minutes.
  const back = await waitForBoard(`return_after_${label}`, 75 * 60 * 1000);
  assert.ok(back, "the board did not come back");
  await sleep(20000);
  assert.ok(await login(), "login after the reboot failed");
  const status = expected === "booted" ? await statusAfterBoot() : await api("GET", "/api/system-update/status");
  step(`status_after_${label}`, status);
  const after = await snapshot("after");
  const same = (key) => JSON.stringify(before[key]) === JSON.stringify(after[key]);
  const carried = Object.fromEntries(
    ["certificate", "hostKeys", "account", "slot", "hostname", "sessionLock", "ssh"].map((key) => [key, same(key)]),
  );
  step("carried", carried);
  return (
    (expected === null || status.data?.phase === expected) &&
    Object.values(carried).every(Boolean) &&
    after.uptime < before.uptime
  );
}

try {
  assert.ok(await login(), "login failed");
  let passed = false;
  if (action === "install") {
    step("switch_on", await api("POST", "/api/system-update/raw-enabled", { enabled: true }));
    let status = await api("GET", "/api/system-update/status");
    step("status_before_install", status);
    if (status.data?.phase !== "staged") {
      assert.ok(bundlePath, "nothing is staged and no bundle was given");
      status = await api("POST", "/api/system-update/upload", undefined, {
        headers: {
          Cookie: cookie,
          "X-CSRF-Token": csrf,
          "Content-Type": "application/octet-stream",
          "Content-Length": String(fs.statSync(bundlePath).size),
        },
        stream: fs.createReadStream(bundlePath),
        timeoutMs: 60 * 60 * 1000,
      });
      step("upload", status);
    }
    assert.equal(status.data?.phase, "staged", "no update is staged");
    passed = await rebootAndCompare(
      "install",
      () => api("POST", "/api/system-update/install", { confirm: true }),
      process.env.RAW_EXPECTED_PHASE ?? "booted",
    );
  } else if (action === "rollback") {
    passed = await rebootAndCompare(
      "rollback",
      () => api("POST", "/api/system-update/rollback", { confirm: true }),
      "rolled_back",
    );
  } else if (action === "reboot") {
    passed = await rebootAndCompare(
      "reboot",
      () => api("POST", "/api/vm/system/reboot", { confirm: true }),
      null,
    );
  } else if (action === "confirm") {
    step("confirm", await api("POST", "/api/system-update/confirm"));
    const status = await api("GET", "/api/system-update/status");
    step("status_confirmed", status);
    passed = status.data?.phase === "confirmed" && status.data?.minimum_sequence >= 2;
  } else if (action === "off") {
    const off = await api("POST", "/api/system-update/raw-enabled", { enabled: false });
    step("switch_off", off);
    const refused = await api("POST", "/api/system-update/install", { confirm: true });
    step("install_refused", refused);
    passed = off.data?.enabled === false && refused.http === 403;
  }
  receipt.result = passed ? "passed" : "failed";
} catch (error) {
  step("error", { message: String(error?.message ?? error) });
  receipt.result = "failed";
} finally {
  password = "";
  fs.writeFileSync(output, JSON.stringify(receipt, null, 1), { mode: 0o600 });
  console.log(`result: ${receipt.result}`);
}
