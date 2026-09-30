// Verify the default-off remote ISO download on the device: refused while
// switched off; with the switch on an invalid URL is refused, an ISO fixture
// downloads with progress into the image list, a file that is not an ISO
// fails, and the downloaded image is deleted again; refused once switched off.
// FIXTURE_URL is a directory that serves probe-download.iso and
// probe-notiso.iso. Leaves remote download switched off. The GUI password is
// read from stdin; a receipt is written under evidence/private.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const [baseUrl, username, fixtureValue, outputValue] = process.argv.slice(2);
const url = new URL(baseUrl);
assert.equal(url.protocol, "https:");
const fixture = new URL(
  fixtureValue.endsWith("/") ? fixtureValue : `${fixtureValue}/`,
);
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
const receipt = {
  schemaVersion: 1,
  probe: "remote-download-switch",
  steps: [],
};
const step = (name, value) => receipt.steps.push({ step: name, ...value });
const image = "probe-download.iso";
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
try {
  const login = await context.post(`${url.origin}/api/auth/login`, {
    data: { username, password },
    headers: { Origin: url.origin },
  });
  password = "";
  const csrf = (await login.json()).data.csrfToken;
  const headers = { Origin: url.origin, "X-CSRF-Token": csrf };
  const api = async (method, route, data) => {
    const response = await context.fetch(`${url.origin}${route}`, {
      method,
      data,
      headers,
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
  const download = (file) => api("POST", "/api/download/image", { file });
  // Polls the status until it leaves in_progress; keeps the progress seen.
  const settle = async () => {
    const seen = [];
    for (let i = 0; i < 240; i += 1) {
      const status = await api("GET", "/api/download/image/status");
      const value = status.data?.status;
      if (status.data?.percentage) seen.push(status.data.percentage);
      if (value !== "in_progress") return { final: value, progress: seen };
      await sleep(500);
    }
    return { final: "timeout", progress: seen };
  };
  const listed = async () =>
    (await api("GET", "/api/storage/image")).data?.files ?? [];

  step("flags_initial", await api("GET", "/api/download/image/enabled"));
  step("download_while_off", await download(new URL(image, fixture).href));
  step(
    "switch_on",
    await api("POST", "/api/download/image/remote/enabled", { enabled: true }),
  );
  step("flags_on", await api("GET", "/api/download/image/enabled"));
  step("invalid_scheme", await download("ftp://example.org/a.iso"));
  step("invalid_name", await download(new URL("probe.img", fixture).href));
  step("start", await download(new URL(image, fixture).href));
  step("second_while_running", await download(new URL(image, fixture).href));
  step("settled", await settle());
  step("listed", { files: await listed() });
  step(
    "not_iso_start",
    await download(new URL("probe-notiso.iso", fixture).href),
  );
  step("not_iso_settled", await settle());
  step("listed_after_not_iso", { files: await listed() });
  step(
    "delete",
    await api("POST", "/api/storage/image/delete", { file: image }),
  );
  step("listed_after_delete", { files: await listed() });
  step(
    "switch_off",
    await api("POST", "/api/download/image/remote/enabled", { enabled: false }),
  );
  step("download_after_off", await download(new URL(image, fixture).href));
  step("flags_final", await api("GET", "/api/download/image/enabled"));
  const by = Object.fromEntries(
    receipt.steps.map((entry) => [entry.step, entry]),
  );
  const names = (entry) => JSON.stringify(entry.files ?? []);
  receipt.result =
    by.flags_initial.data?.remoteEnabled === false &&
    by.download_while_off.http === 403 &&
    by.switch_on.data?.enabled === true &&
    by.flags_on.data?.remoteEnabled === true &&
    by.invalid_scheme.code !== 0 &&
    by.invalid_name.code !== 0 &&
    by.start.code === 0 &&
    by.start.data?.status === "in_progress" &&
    by.settled.final === "idle" &&
    names(by.listed).includes(image) &&
    by.not_iso_start.code === 0 &&
    by.not_iso_settled.final === "failed" &&
    !names(by.listed_after_not_iso).includes("probe-notiso.iso") &&
    by.delete.code === 0 &&
    !names(by.listed_after_delete).includes(image) &&
    by.switch_off.data?.enabled === false &&
    by.download_after_off.http === 403 &&
    by.flags_final.data?.remoteEnabled === false
      ? "passed"
      : "failed";
} finally {
  await context.dispose();
  fs.writeFileSync(output, JSON.stringify(receipt, null, 1), { mode: 0o600 });
  console.log(JSON.stringify(receipt));
}
