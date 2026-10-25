// On a board, without remote control: the QUIC session's input stream opens,
// and a report sent on it is refused with the answer HTTPS gives, word for
// word, because nobody confirmed the target. The same is tried REPORTS times
// over each transport and the time to the answer taken: the refusal is made
// by the web server alone, so it shows what the two transports cost on the
// real network without touching the keyboard or the mouse.
//
// The probe never confirms a target and never asks for a lease: it sends
// reports under a lease id that does not exist, and requests that are no
// lease requests. It fails if any /api/hid/focus request is made or any
// lease request is applied. The page's Transport section must say that
// input will use the session once control is enabled. The switch is turned
// on through the API and left off, whatever happens. The GUI password is
// read from stdin; a receipt is written under evidence/private, without
// tokens. No screenshot is taken.
//
// Usage: node quic_input_device_probe.mjs BASE_URL USERNAME OUTPUT [REPORTS]
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { watchingMayAsk } from "./app_harness.mjs";

const [baseUrl, username, outputValue, reportsValue] = process.argv.slice(2);
assert.equal(new URL(baseUrl).protocol, "https:");
const output = path.resolve(outputValue);
assert.equal(path.dirname(output), path.resolve("evidence/private"));
assert.ok(!fs.existsSync(output));
const REPORTS = Number(reportsValue ?? 200);
assert.ok(REPORTS >= 10 && REPORTS <= 2000);
const WARM_UP = 10;
let password = "";
for await (const chunk of process.stdin) password += chunk.toString("utf8");
password = password.replace(/[\r\n]+$/, "");
assert.ok(password.length >= 8);

const { chromium } = await import(
  pathToFileURL(
    process.env.PLAYWRIGHT_MODULE ??
      path.join(
        os.homedir(),
        ".local/share/playwright/node_modules/playwright/index.mjs",
      ),
  ).href
);
const browser = await chromium.launch({
  headless: true,
  executablePath:
    process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH ??
    path.join(
      os.homedir(),
      ".cache/ms-playwright/chromium-1228/chrome-linux64/chrome",
    ),
});

const PATIENCE = 20000;
const now = () => performance.now();
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(what, condition, patience = PATIENCE) {
  const deadline = now() + patience;
  for (;;) {
    const value = await condition();
    if (value) return value;
    assert.ok(now() < deadline, `timed out: ${what}`);
    await sleep(50);
  }
}

function hostClocks() {
  try {
    const [monotonic, raw] = execFileSync(
      "python3",
      [
        "-c",
        "import time;print(time.monotonic(), time.clock_gettime(time.CLOCK_MONOTONIC_RAW))",
      ],
      { encoding: "utf8" },
    )
      .trim()
      .split(" ")
      .map(Number);
    return { monotonic, raw };
  } catch {
    return null;
  }
}

function summary(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const at = (share) =>
    sorted[Math.min(sorted.length - 1, Math.floor(share * sorted.length))];
  const round = (value) => Math.round(value * 100) / 100;
  return {
    count: sorted.length,
    min: round(sorted[0]),
    median: round(at(0.5)),
    p95: round(at(0.95)),
    max: round(sorted[sorted.length - 1]),
    mean: round(sorted.reduce((sum, value) => sum + value, 0) / sorted.length),
  };
}

const receipt = {
  schemaVersion: 1,
  probe: "quic-input-device",
  browser: browser.version(),
  reports: REPORTS,
  steps: [],
  hidRequests: {},
  errors: [],
};
const step = (name, detail = {}) => {
  receipt.steps.push({ step: name, ...detail });
  console.log(`${name}: ${JSON.stringify(detail)}`);
};

let result = "failed";
let context = null;
let csrf = "";
let switchedOn = false;
const clocksBefore = hostClocks();

// A request of the login, with its CSRF token.
async function call(method, route, data) {
  const response = await context.request.fetch(`${baseUrl}${route}`, {
    method,
    data,
    headers: { Origin: baseUrl, "X-CSRF-Token": csrf },
    timeout: 60000,
  });
  const body = await response.json().catch(() => null);
  return { status: response.status(), body };
}

try {
  context = await browser.newContext({
    ignoreHTTPSErrors: true,
    locale: "en-US",
    viewport: { width: 1440, height: 1000 },
  });
  await context.addInitScript(() =>
    localStorage.setItem("hardened-blikvm-transport", "quic"),
  );
  const login = await context.request.post(`${baseUrl}/api/auth/login`, {
    data: { username, password },
    headers: { Origin: baseUrl },
  });
  password = "";
  csrf = (await login.json()).data?.csrfToken;
  assert.ok(csrf, "login failed");

  const found = (await call("GET", "/api/transport")).body.data.quic;
  step("found", { quic: found });
  assert.ok(found.supported, "this board has no QUIC transport");
  assert.equal(found.enabled, false, "the switch is on already");
  switchedOn = true;
  const on = await call("POST", "/api/transport/quic/enable");
  assert.equal(on.status, 200);
  await until(
    "the gateway reaches the server",
    async () => (await call("GET", "/api/transport")).body.data.quic.connected,
  );

  const page = await context.newPage();
  page.setDefaultTimeout(PATIENCE);
  page.on("pageerror", (error) =>
    receipt.errors.push(String(error.message).slice(0, 200)),
  );
  page.on("request", (request) => {
    const route = new URL(request.url()).pathname;
    if (
      route.startsWith("/api/hid") &&
      !watchingMayAsk(request.method(), route)
    ) {
      const key = `${request.method()} ${route}`;
      receipt.hidRequests[key] = (receipt.hidRequests[key] ?? 0) + 1;
    }
  });
  await page.goto(`${baseUrl}/`, { waitUntil: "domcontentloaded" });
  await page.getByTestId("menu-bar").waitFor({ timeout: 30000 });

  // The page's own session, and what its section says about input.
  await page.locator("svg.lucide-settings").first().click();
  await page.locator('[data-settings-tab="system"]').click();
  await page.waitForFunction(
    () =>
      document
        .querySelector('[data-testid="transport-status"]')
        ?.getAttribute("data-state") === "connected",
  );
  const line = page.getByTestId("transport-input");
  step("page session", {
    status: await page.getByTestId("transport-status").innerText(),
    input: await line.getAttribute("data-input"),
    says: await line.innerText(),
  });
  assert.equal(await line.getAttribute("data-input"), "none");
  assert.equal(
    await line.innerText(),
    "Keyboard and mouse use this session once remote control is enabled.",
  );

  // A second session of the login, driven here: the input stream, the same
  // refused requests over both transports, and their times.
  const measured = await page.evaluate(
    async ([count, warmUp]) => {
      const csrf = decodeURIComponent(
        document.cookie.match(/hardened-blikvm-csrf=([^;]+)/)?.[1] ?? "",
      );
      const post = async (route, body) => {
        const response = await fetch(route, {
          method: "POST",
          credentials: "include",
          headers: {
            "Content-Type": "application/json",
            "x-csrf-token": csrf,
          },
          body: JSON.stringify(body),
        });
        let parsed = null;
        try {
          parsed = await response.json();
        } catch {
          // No JSON body.
        }
        return { status: response.status, body: parsed };
      };
      const operation = () =>
        Array.from(crypto.getRandomValues(new Uint8Array(16)), (byte) =>
          byte.toString(16).padStart(2, "0"),
        ).join("");
      const frame = (message) => {
        const body = new TextEncoder().encode(JSON.stringify(message));
        const out = new Uint8Array(2 + body.length);
        out[0] = body.length >> 8;
        out[1] = body.length & 0xff;
        out.set(body, 2);
        return out;
      };
      const reading = (readable) => {
        const reader = readable.getReader();
        let buffer = new Uint8Array(0);
        return async () => {
          for (;;) {
            if (buffer.length >= 2) {
              const length = (buffer[0] << 8) | buffer[1];
              if (buffer.length >= 2 + length) {
                const message = JSON.parse(
                  new TextDecoder().decode(buffer.subarray(2, 2 + length)),
                );
                buffer = buffer.slice(2 + length);
                return message;
              }
            }
            const { value, done } = await reader.read();
            if (done || !value) return null;
            const joined = new Uint8Array(buffer.length + value.length);
            joined.set(buffer);
            joined.set(value, buffer.length);
            buffer = joined;
          }
        };
      };

      const grant = (
        await post("/api/transport/quic/session", { pageTime: Date.now() })
      ).body.data;
      if (!grant.granted) return { error: `no token: ${grant.reason}` };
      const hash = Uint8Array.from(
        grant.certificateSha256.match(/../g),
        (pair) => Number.parseInt(pair, 16),
      );
      const session = new WebTransport(
        `https://${location.hostname}:${grant.port}/kvm`,
        { serverCertificateHashes: [{ algorithm: "sha-256", value: hash }] },
      );
      session.closed.catch(() => undefined);
      await session.ready;
      const control = await session.createBidirectionalStream();
      const hearControl = reading(control.readable);
      await control.writable
        .getWriter()
        .write(frame({ t: "hello", protocol: 2, token: grant.token }));
      const welcome = await hearControl();
      if (welcome?.t !== "welcome")
        return { error: `no welcome: ${welcome?.t}` };
      const opened = performance.now();
      const input = await session.createBidirectionalStream();
      const writer = input.writable.getWriter();
      const hear = reading(input.readable);
      let seq = 0;
      const ask = async (request) => {
        seq += 1;
        await writer.write(frame({ seq, request }));
        const answer = await hear();
        if (answer?.seq !== seq) throw new Error("the answer is another's");
        return answer.response;
      };
      // A lease that nobody was granted.
      const report = (index) => ({
        action: "report",
        operationId: operation(),
        leaseId: "00".repeat(16),
        report: {
          mode: "keyboard",
          data: [0, 0, index % 2 === 0 ? 4 : 0, 0, 0, 0, 0, 0],
        },
      });
      const first = { quic: await ask(report(0)) };
      const inputOpenMs = performance.now() - opened;
      first.https = await post("/api/hid/lease", report(0));
      const notALeaseRequest = { action: "reboot" };
      const other = {
        quic: await ask(notALeaseRequest),
        https: (await post("/api/hid/lease", notALeaseRequest)).status,
      };

      const times = { https: [], quic: [] };
      const answers = new Set();
      for (const transport of ["https", "quic", "https", "quic"]) {
        for (let index = 0; index < (count + warmUp) / 2; index += 1) {
          const request = report(index);
          const started = performance.now();
          const answer =
            transport === "quic"
              ? await ask(request)
              : await post("/api/hid/lease", request);
          const took = performance.now() - started;
          if (index >= warmUp / 2) times[transport].push(took);
          // The same answer in one spelling, whichever transport gave it.
          answers.add(
            JSON.stringify({ status: answer.status, body: answer.body }),
          );
        }
      }
      session.close();
      return { first, other, inputOpenMs, times, answers: [...answers] };
    },
    [REPORTS, WARM_UP],
  );
  assert.equal(measured.error, undefined, measured.error);
  const refusal = {
    status: 200,
    body: { code: -1, msg: "a current focus confirmation is required" },
  };
  step("refusal", {
    quic: measured.first.quic,
    https: measured.first.https,
    inputOpenMs: Math.round(measured.inputOpenMs * 10) / 10,
    notALeaseRequest: measured.other,
  });
  assert.deepEqual(measured.first.quic, refusal);
  assert.deepEqual(measured.first.https, refusal);
  assert.equal(measured.other.quic.status, 422);
  assert.equal(measured.other.https, 422);
  // Every one of the timed requests got that same refusal.
  assert.deepEqual(measured.answers, [JSON.stringify(refusal)]);
  step("refused reports", {
    https: summary(measured.times.https),
    quic: summary(measured.times.quic),
  });

  const off = await call("POST", "/api/transport/quic/disable");
  switchedOn = false;
  assert.equal(off.status, 200);
  const after = (await call("GET", "/api/transport")).body.data.quic;
  step("switched off", { quic: after });
  assert.deepEqual(
    [after.enabled, after.active, after.connected],
    [false, false, false],
  );

  const focus = Object.keys(receipt.hidRequests).filter((key) =>
    key.endsWith("/api/hid/focus"),
  );
  assert.deepEqual(focus, [], "a target was confirmed");
  assert.deepEqual(Object.keys(receipt.hidRequests), ["POST /api/hid/lease"]);
  assert.deepEqual(receipt.errors, []);
  result = "passed";
} catch (error) {
  receipt.errors.push(String(error?.message ?? error).slice(0, 300));
} finally {
  password = "";
  if (context) {
    if (switchedOn) {
      try {
        const answer = await call("POST", "/api/transport/quic/disable");
        receipt.cleanup = { status: answer.status };
      } catch (error) {
        receipt.cleanup = {
          error: String(error?.message ?? error).slice(0, 200),
        };
      }
    }
    await call("POST", "/api/auth/logout").catch(() => undefined);
  }
  const clocksAfter = hostClocks();
  if (clocksBefore && clocksAfter) {
    receipt.hostClockRate =
      Math.round(
        ((clocksAfter.monotonic - clocksBefore.monotonic) /
          (clocksAfter.raw - clocksBefore.raw)) *
          10000,
      ) / 10000;
  }
  receipt.result = result;
  fs.writeFileSync(output, JSON.stringify(receipt, null, 1), { mode: 0o600 });
  await browser.close();
}
console.log(`result: ${result}`);
process.exit(result === "passed" ? 0 : 1);
