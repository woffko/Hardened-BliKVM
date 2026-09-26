// Verify the default-off PicoClaw assistant on the device: refused while
// switched off; with the switch on the pinned runtime installs, a model is
// configured without the key coming back, the runtime starts, a chat through
// the gateway relay makes the runtime take a screenshot and move the mouse
// through the operator's HID lease, manual input is held back meanwhile, the
// session appears in the history and is deleted, switching off stops the
// runtime, and uninstall removes it. MODEL_BASE is an OpenAI-compatible base
// URL of tools/device/fake_openai_model.py (which also serves /log). Leaves
// PicoClaw switched off and uninstalled. The GUI password is read from stdin;
// a receipt is written under evidence/private.
import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const [baseUrl, username, modelBase, outputValue] = process.argv.slice(2);
const url = new URL(baseUrl);
assert.equal(url.protocol, "https:");
const model = new URL(modelBase);
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
const receipt = { schemaVersion: 1, probe: "picoclaw-switch", steps: [] };
const step = (name, value) => receipt.steps.push({ step: name, ...value });
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const probeKey = `probe-${crypto.randomBytes(6).toString("hex")}`;
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
  const csrf = (await login.json()).data.csrfToken;
  const headers = { Origin: url.origin, "X-CSRF-Token": csrf };
  const api = async (method, route, data) => {
    const response = await context.request.fetch(`${url.origin}${route}`, {
      method,
      data,
      headers,
      timeout: 60000,
    });
    const text = await response.text();
    let body = null;
    try {
      body = JSON.parse(text);
    } catch {
      body = null;
    }
    return {
      http: response.status(),
      code: body?.code,
      message: body?.message ?? body?.msg,
      data: body?.data,
      leaksKey: text.includes(probeKey),
    };
  };
  const status = async () => api("GET", "/api/picoclaw/runtime/status");
  const feature = (enabled) =>
    api("POST", "/api/admin/features", { feature: "picoclaw", enabled });

  step("status_while_off", await status());
  step("switch_on", await feature(true));
  step("status_on", await status());
  step("install", await api("POST", "/api/picoclaw/runtime/install"));
  const stages = [];
  let installed = null;
  for (let i = 0; i < 150; i += 1) {
    const current = await status();
    const data = current.data ?? {};
    stages.push(
      `${data.status}:${data.install_stage ?? ""}:${data.install_progress ?? ""}`,
    );
    if (data.installed && !data.installing) {
      installed = current;
      break;
    }
    if (data.status === "install_failed") {
      installed = current;
      break;
    }
    await sleep(2000);
  }
  step("installed", { ...installed, stages: [...new Set(stages)] });
  step(
    "model",
    await api("POST", "/api/picoclaw/model/config", {
      model: "openai/fake-model",
      api_base: model.href.replace(/\/$/, ""),
      api_key: probeKey,
    }),
  );
  step(
    "profile_default",
    await api("POST", "/api/picoclaw/agent/profile", { profile: "default" }),
  );
  step(
    "profile_kvm",
    await api("POST", "/api/picoclaw/agent/profile", { profile: "kvm" }),
  );
  step("start", await api("POST", "/api/picoclaw/runtime/start"));
  step("status_started", await status());

  const listed = async () =>
    ((await api("GET", "/api/picoclaw/sessions")).data ?? []).map(
      (item) => item.id,
    );
  const before = await listed();
  const page = await context.newPage();
  await page.goto(`${url.origin}/`);
  await page.waitForSelector("#screen", { timeout: 20000 });
  const toggle = page.locator('button[aria-label$="remote control"]');
  await toggle.first().click();
  await page.getByRole("button", { name: "Confirm and control" }).click();
  step("control", {
    pressed: await toggle.first().getAttribute("aria-pressed"),
  });

  // Drive the real assistant sidebar and watch its gateway frames.
  const chat = { types: [], observation: 0, final: "", manual: [] };
  page.on("websocket", (socket) => {
    if (!socket.url().includes("/api/picoclaw/gateway/ws")) return;
    socket.on("framereceived", (frame) => {
      let message = null;
      try {
        message = JSON.parse(String(frame.payload));
      } catch {
        return;
      }
      chat.types.push(message.type);
      const payload = message.payload ?? {};
      if (
        message.type === "observation" &&
        String(payload.image_base64 ?? "").length > 1000
      ) {
        chat.observation += 1;
      }
      const content = String(payload.content ?? "");
      if (
        (message.type === "message.create" ||
          message.type === "message.update") &&
        content.includes("FAKE-DONE")
      ) {
        chat.final = content.slice(0, 600);
      }
    });
  });
  page.on("response", async (response) => {
    if (!response.url().includes("/api/hid/lease")) return;
    try {
      const body = await response.json();
      if (body?.data?.rejection) chat.manual.push(body.data.rejection);
    } catch {
      // not JSON
    }
  });
  await page.getByRole("button", { name: "PicoClaw Assistant" }).click();
  const input = page.getByPlaceholder(
    "Describe what you want the PicoClaw to do",
  );
  await input.waitFor({ timeout: 30000 });
  await page.waitForFunction(
    () => !document.querySelector("textarea")?.hasAttribute("disabled"),
    null,
    { timeout: 30000 },
  );
  await input.fill("Take a screenshot and centre the mouse.");
  await input.press("Enter");
  // Manual input while the assistant has control: over the screen and on
  // the raw HID socket.
  const box = await page.locator("#screen").boundingBox();
  for (let i = 0; i < 5; i += 1) {
    await page.mouse.move(
      box.x + box.width * (0.2 + i * 0.1),
      box.y + box.height / 3,
    );
    await sleep(150);
  }
  chat.raw = await page.evaluate(
    () =>
      new Promise((resolve) => {
        const hid = new WebSocket(`wss://${location.host}/api/ws`);
        hid.binaryType = "arraybuffer";
        hid.onopen = () => hid.send(new Uint8Array([2, 0, 0, 0, 0, 0, 0]));
        hid.onmessage = (event) => {
          resolve(String(event.data).slice(0, 120));
          hid.close();
        };
        setTimeout(() => resolve("no answer"), 5000);
      }),
  );
  for (let i = 0; i < 240 && !chat.final; i += 1) await sleep(500);
  chat.controlAfter = await toggle.first().getAttribute("aria-pressed");
  chat.types = [...new Set(chat.types)];
  step("chat", chat);
  await sleep(1500);
  step("status_after_chat", await status());
  // The runtime keys its history by an opaque ID, as in the reference.
  const added = (await listed()).filter((id) => !before.includes(id));
  step("sessions", { before: before.length, added });
  const historyId = added[0] ?? "missing";
  const detail = await api("GET", `/api/picoclaw/sessions/${historyId}`);
  step("session_detail", {
    http: detail.http,
    code: detail.code,
    messages: detail.data?.messages?.length ?? 0,
  });
  step(
    "session_delete",
    await api("DELETE", `/api/picoclaw/sessions/${historyId}`),
  );
  step("sessions_after_delete", { ids: await listed() });
  let modelLog = [];
  try {
    modelLog = await (await fetch(new URL("/log", model))).json();
  } catch (error) {
    modelLog = [{ error: String(error) }];
  }
  step("model_log", {
    requests: modelLog.length,
    tools: modelLog[0]?.tools ?? [],
    answers: modelLog.map((entry) => entry.answer).slice(0, 6),
    results: modelLog.at(-1)?.tool_results ?? [],
  });
  step("switch_off", await feature(false));
  step("status_off", await status());
  step("switch_on_again", await feature(true));
  step("status_after_off", await status());
  step("uninstall", await api("POST", "/api/picoclaw/runtime/uninstall"));
  step("switch_off_final", await feature(false));
  step("features_final", await api("GET", "/api/admin/features"));

  const by = Object.fromEntries(
    receipt.steps.map((entry) => [entry.step, entry]),
  );
  const leaked = receipt.steps.some((entry) => entry.leaksKey);
  receipt.result =
    by.status_while_off.http === 403 &&
    by.switch_on.data?.enabled === true &&
    by.status_on.data?.installed === false &&
    by.install.code === 0 &&
    by.installed.data?.installed === true &&
    by.model.code === 0 &&
    by.model.data?.status?.model_configured === true &&
    by.profile_default.data?.profile === "default" &&
    by.profile_kvm.data?.profile === "kvm" &&
    by.start.code === 0 &&
    by.status_started.data?.ready === true &&
    by.control.pressed === "true" &&
    by.chat.observation >= 1 &&
    /FAKE-DONE/.test(by.chat.final) &&
    /assistant_active/.test(by.chat.raw) &&
    by.chat.controlAfter === "true" &&
    /hid_writes/.test(by.chat.final) &&
    by.sessions.added.length === 1 &&
    !by.sessions_after_delete.ids.includes(historyId) &&
    by.session_detail.messages >= 2 &&
    by.session_delete.code === 0 &&
    by.model_log.answers.length >= 3 &&
    by.switch_off.data?.enabled === false &&
    by.status_off.http === 403 &&
    by.status_after_off.data?.ready === false &&
    by.uninstall.code === 0 &&
    by.uninstall.data?.installed === false &&
    by.features_final.data?.picoclaw === false &&
    !leaked
      ? "passed"
      : "failed";
  receipt.keyLeaked = leaked;
} finally {
  await browser.close();
  fs.writeFileSync(output, JSON.stringify(receipt, null, 1), { mode: 0o600 });
  console.log(JSON.stringify(receipt));
}
