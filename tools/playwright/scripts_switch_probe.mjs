// Verify default-off scripts on the device: refused while switched off;
// upload, list, foreground and background runs and delete while on; refused
// again once switched off. Leaves scripts switched off and no probe script
// behind. The GUI password is read from stdin; a receipt is written under
// evidence/private.
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
const receipt = { schemaVersion: 1, probe: "scripts-switch", steps: [] };
const step = (name, value) => receipt.steps.push({ step: name, ...value });
const name = "probe-scripts.sh";
try {
  const login = await context.post(`${url.origin}/api/auth/login`, {
    data: { username, password },
    headers: { Origin: url.origin },
  });
  password = "";
  const csrf = (await login.json()).data.csrfToken;
  const headers = { Origin: url.origin, "X-CSRF-Token": csrf };
  const result = async (response) => {
    let body = null;
    try {
      body = await response.json();
    } catch {
      body = null;
    }
    return { http: response.status(), code: body?.code, data: body?.data };
  };
  const api = async (method, route, data) =>
    result(
      await context.fetch(`${url.origin}${route}`, { method, data, headers }),
    );
  const upload = async (
    fileName = name,
    text = "echo script-$((6*7)); id -u\n",
  ) =>
    result(
      await context.post(`${url.origin}/api/vm/script/upload`, {
        headers,
        multipart: {
          file: {
            name: fileName,
            mimeType: "text/x-sh",
            buffer: Buffer.from(text),
          },
        },
      }),
    );

  step("list_while_off", await api("GET", "/api/vm/script"));
  step("upload_while_off", await upload());
  step(
    "switch_on",
    await api("POST", "/api/admin/features", {
      feature: "scripts",
      enabled: true,
    }),
  );
  step("upload", await upload());
  step("list", await api("GET", "/api/vm/script"));
  step(
    "run_foreground",
    await api("POST", "/api/vm/script/run", { name, type: "foreground" }),
  );
  // A script that leaves a background child must not hold the run (review R5).
  const childName = name.replace(/\.sh$/, "-child.sh");
  step(
    "upload_child",
    await upload(childName, "sleep 20 &\necho child-started\n"),
  );
  const childStarted = Date.now();
  step("run_with_child", {
    ...(await api("POST", "/api/vm/script/run", {
      name: childName,
      type: "foreground",
    })),
    elapsedMs: Date.now() - childStarted,
  });
  step(
    "delete_child",
    await api("DELETE", "/api/vm/script", { name: childName }),
  );
  step(
    "run_background",
    await api("POST", "/api/vm/script/run", { name, type: "background" }),
  );
  step(
    "bad_name",
    await api("POST", "/api/vm/script/run", {
      name: "../x.sh",
      type: "foreground",
    }),
  );
  step("delete", await api("DELETE", "/api/vm/script", { name }));
  step("list_after_delete", await api("GET", "/api/vm/script"));
  step(
    "switch_off",
    await api("POST", "/api/admin/features", {
      feature: "scripts",
      enabled: false,
    }),
  );
  step("list_after_off", await api("GET", "/api/vm/script"));
  const by = Object.fromEntries(
    receipt.steps.map((entry) => [entry.step, entry]),
  );
  const log = by.run_foreground.data?.log ?? "";
  receipt.result =
    by.list_while_off.http === 403 &&
    by.upload_while_off.http === 403 &&
    by.switch_on.data?.enabled === true &&
    by.upload.data?.file === name &&
    by.list.data?.files?.includes(name) &&
    /script-42/.test(log) &&
    /^0$/m.test(log) &&
    /child-started/.test(by.run_with_child.data?.log ?? "") &&
    by.run_with_child.elapsedMs < 2500 &&
    by.run_background.code === 0 &&
    by.bad_name.code !== 0 &&
    by.delete.code === 0 &&
    !by.list_after_delete.data?.files?.includes(name) &&
    by.switch_off.data?.enabled === false &&
    by.list_after_off.http === 403
      ? "passed"
      : "failed";
} finally {
  await context.dispose();
  fs.writeFileSync(output, JSON.stringify(receipt, null, 1), { mode: 0o600 });
  console.log(JSON.stringify(receipt));
}
