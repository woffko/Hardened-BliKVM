// On a board: the QUIC transport through the page, from the switch to the
// end. The switch is turned on in Settings (the question must name the UDP
// port), this browser chooses QUIC and the page's session opens. For SECONDS
// a second session of the same login pings the gateway four times a second
// and collects the round trips, by the page's clock and as the gateway's
// QUIC stack measures them on the board; the page's own session must stay
// connected all the while. A logout must end both sessions and send the page
// to the login. A second login, made at the start, then switches QUIC off;
// afterwards the port must answer nothing and the server must issue no
// token.
//
// Remote control is never enabled: the probe fails if any /api/hid request
// is made. Whatever happens, the switch is left off. The GUI password is
// read from stdin; a receipt is written under evidence/private, without
// tokens. No screenshot is taken.
//
// Usage: node quic_transport_device_probe.mjs BASE_URL USERNAME OUTPUT [SECONDS]
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { watchingMayAsk } from "./app_harness.mjs";

const [baseUrl, username, outputValue, secondsValue] = process.argv.slice(2);
assert.equal(new URL(baseUrl).protocol, "https:");
const output = path.resolve(outputValue);
assert.equal(path.dirname(output), path.resolve("evidence/private"));
assert.ok(!fs.existsSync(output));
const SECONDS = Number(secondsValue ?? 60);
assert.ok(SECONDS >= 5 && SECONDS <= 600);
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

// How fast this host's monotonic clock runs against its raw one. The spans
// taken here and in the page are on the monotonic clock; a rate other than 1
// tells by how much they are off.
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
  if (values.length === 0) return null;
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
  probe: "quic-transport-device",
  browser: browser.version(),
  seconds: SECONDS,
  steps: [],
  hidRequests: [],
  errors: [],
};
const step = (name, detail = {}) => {
  receipt.steps.push({ step: name, ...detail });
  console.log(`${name}: ${JSON.stringify(detail)}`);
};

async function login(viewport) {
  const context = await browser.newContext({
    ignoreHTTPSErrors: true,
    locale: "en-US",
    viewport,
  });
  const answer = await context.request.post(`${baseUrl}/api/auth/login`, {
    data: { username, password },
    headers: { Origin: baseUrl },
  });
  const csrf = (await answer.json()).data?.csrfToken;
  assert.ok(csrf, "login failed");
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
    )
      receipt.hidRequests.push(`${request.method()} ${route}`);
  });
  return { context, page, csrf };
}

// A request of a login, with its CSRF token.
async function call(who, method, route, data) {
  const response = await who.context.request.fetch(`${baseUrl}${route}`, {
    method,
    data,
    headers: { Origin: baseUrl, "X-CSRF-Token": who.csrf },
    timeout: 60000,
  });
  const body = await response.json().catch(() => null);
  return { status: response.status(), data: body?.data ?? null };
}
const transport = async (who) =>
  (await call(who, "GET", "/api/transport")).data?.quic ?? null;

async function openSettings(page) {
  await page.goto(`${baseUrl}/`, { waitUntil: "domcontentloaded" });
  await page.getByTestId("menu-bar").waitFor({ timeout: 30000 });
  await page.locator("svg.lucide-settings").first().click();
  await page.locator('[data-settings-tab="system"]').click();
  await page.getByTestId("transport-settings").waitFor();
  await page.waitForFunction(
    () =>
      !document
        .querySelector('[data-testid="transport-device-switch"]')
        .classList.contains("ant-switch-loading"),
  );
}
const switchIsOn = (page) =>
  page
    .getByTestId("transport-device-switch")
    .getAttribute("aria-checked")
    .then((value) => value === "true");
const quicOption = (page) =>
  page.locator('[data-testid="transport-choice"] label', { hasText: "QUIC" });
const quicIsOffered = async (page) =>
  !(await quicOption(page).getAttribute("class")).includes(
    "ant-segmented-item-disabled",
  );
const statusState = (page) =>
  page.getByTestId("transport-status").getAttribute("data-state");

let result = "failed";
let user = null;
let keeper = null;
let switchedOn = false;
const clocksBefore = hostClocks();
try {
  // Two logins while the password is at hand: the one that uses QUIC and is
  // logged out, and the one that switches QUIC off afterwards.
  user = await login({ width: 1440, height: 1000 });
  keeper = await login({ width: 1280, height: 800 });
  password = "";

  // What the page tells Node, also when it is about to navigate away.
  const reports = [];
  await user.context.exposeBinding("__quicProbeReport", (_source, report) => {
    reports.push(report);
  });

  await openSettings(user.page);
  const found = await transport(user);
  step("found", { quic: found });
  assert.ok(found?.supported, "this board has no QUIC transport");
  assert.equal(found.enabled, false, "the switch is on already");
  assert.equal(found.connected, false);
  assert.equal(await switchIsOn(user.page), false);
  assert.equal(await quicIsOffered(user.page), false);
  assert.equal(
    await user.page.getByTestId("transport-hindrance").innerText(),
    "QUIC is switched off on this device.",
  );
  // Off, the port answers nothing and no token is to be had.
  const refusedOff = await call(user, "POST", "/api/transport/quic/session", {
    pageTime: Date.now(),
  });
  assert.deepEqual(refusedOff.data, { granted: false, reason: "unavailable" });

  // Switch on: the question names the UDP port.
  await user.page.getByTestId("transport-device-switch").click();
  const dialog = user.page.locator(".ant-modal-confirm");
  await dialog.waitFor();
  const question = await dialog.innerText();
  assert.match(question, /UDP port 443\b/);
  const switchStarted = now();
  switchedOn = true;
  await dialog.getByRole("button", { name: "Switch on" }).click();
  await until("the switch shows on", () => switchIsOn(user.page));
  await until("QUIC is offered", () => quicIsOffered(user.page));
  const on = await transport(user);
  step("switched on", {
    ms: Math.round(now() - switchStarted),
    quic: on,
  });
  assert.deepEqual(
    [on.enabled, on.active, on.connected, on.blocked, on.port],
    [true, true, true, false, 443],
  );

  // This browser chooses QUIC: the page's session.
  const chosen = now();
  await quicOption(user.page).click();
  await user.page.waitForFunction(() => {
    const line = document.querySelector('[data-testid="transport-status"]');
    return (
      line?.getAttribute("data-state") === "connected" &&
      /round trip \d+\.\d ms/.test(line.textContent)
    );
  });
  step("page session", {
    ms: Math.round(now() - chosen),
    status: await user.page.getByTestId("transport-status").innerText(),
  });

  // A second session of the same login: pings for SECONDS.
  const pinging = user.page.evaluate(async (seconds) => {
    const report = window.__quicProbeReport;
    const csrf = decodeURIComponent(
      document.cookie.match(/hardened-blikvm-csrf=([^;]+)/)?.[1] ?? "",
    );
    const asked = performance.now();
    const answer = await fetch("/api/transport/quic/session", {
      method: "POST",
      credentials: "include",
      headers: {
        "Content-Type": "application/json",
        "x-csrf-token": csrf,
      },
      body: JSON.stringify({ pageTime: Date.now() }),
    });
    const grant = (await answer.json()).data;
    if (!grant?.granted) return { error: `no token: ${grant?.reason}` };
    const granted = performance.now();
    const hash = Uint8Array.from(grant.certificateSha256.match(/../g), (pair) =>
      Number.parseInt(pair, 16),
    );
    const session = new WebTransport(
      `https://${location.hostname}:${grant.port}/kvm`,
      { serverCertificateHashes: [{ algorithm: "sha-256", value: hash }] },
    );
    session.closed.catch(() => undefined);
    await session.ready;
    const ready = performance.now();
    const stream = await session.createBidirectionalStream();
    const writer = stream.writable.getWriter();
    const reader = stream.readable.getReader();
    const encoder = new TextEncoder();
    const decoder = new TextDecoder();
    const say = (message) => {
      const body = encoder.encode(JSON.stringify(message));
      const out = new Uint8Array(2 + body.length);
      out[0] = body.length >> 8;
      out[1] = body.length & 0xff;
      out.set(body, 2);
      return writer.write(out);
    };
    let buffer = new Uint8Array(0);
    const hear = async () => {
      for (;;) {
        if (buffer.length >= 2) {
          const length = (buffer[0] << 8) | buffer[1];
          if (buffer.length >= 2 + length) {
            const message = JSON.parse(
              decoder.decode(buffer.subarray(2, 2 + length)),
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
    await say({ t: "hello", protocol: 2, token: grant.token });
    const welcome = await hear();
    if (welcome?.t !== "welcome") return { error: `no welcome: ${welcome?.t}` };
    const welcomed = performance.now();

    const roundTrips = [];
    const boardRoundTrips = [];
    let packetsLost = 0;
    let pongs = 0;
    // Reads on after the pings are over: the end of the session is reported.
    (async () => {
      for (;;) {
        const message = await hear().catch(() => null);
        if (!message) {
          report({ type: "ended", at: performance.now() });
          return;
        }
        if (message.t === "pong") {
          pongs += 1;
          roundTrips.push(performance.now() - message.pageTime);
        } else if (message.t === "counters") {
          boardRoundTrips.push(message.roundTripMicros / 1000);
          packetsLost = message.packetsLost;
        } else if (message.t === "closing") {
          report({
            type: "closing",
            reason: message.reason,
            at: performance.now(),
          });
        }
      }
    })();
    let pings = 0;
    const until = performance.now() + seconds * 1000;
    while (performance.now() < until) {
      pings += 1;
      await say({ t: "ping", id: pings, pageTime: performance.now() });
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
    return {
      tokenMs: granted - asked,
      readyMs: ready - granted,
      welcomeMs: welcomed - ready,
      maxDatagram: welcome.maxDatagram,
      pings,
      pongs,
      roundTrips,
      boardRoundTrips,
      packetsLost,
    };
  }, SECONDS);
  // Meanwhile the page's own session stays connected.
  const states = [];
  let pingingDone = false;
  const pinged = pinging.finally(() => (pingingDone = true));
  while (!pingingDone) {
    states.push(await statusState(user.page));
    await sleep(1000);
  }
  const pings = await pinged;
  assert.equal(pings.error, undefined, pings.error);
  const lastStatus = await user.page
    .getByTestId("transport-status")
    .innerText();
  step("pings", {
    tokenMs: Math.round(pings.tokenMs * 10) / 10,
    readyMs: Math.round(pings.readyMs * 10) / 10,
    welcomeMs: Math.round(pings.welcomeMs * 10) / 10,
    maxDatagram: pings.maxDatagram,
    pings: pings.pings,
    pongs: pings.pongs,
    roundTripMs: summary(pings.roundTrips),
    boardRoundTripMs: summary(pings.boardRoundTrips),
    packetsLost: pings.packetsLost,
    pageSessionSamples: states.length,
    pageSessionNotConnected: states.filter((state) => state !== "connected")
      .length,
    pageSessionStatus: lastStatus,
  });
  assert.equal(pings.pongs, pings.pings, "a ping stayed unanswered");
  assert.ok(pings.boardRoundTrips.length >= SECONDS / 2 - 3, "few counters");
  assert.deepEqual(
    states.filter((state) => state !== "connected"),
    [],
    "the page's session did not stay connected",
  );
  assert.match(lastStatus, /^The QUIC session is open: round trip /);

  // A logout ends both sessions; the page asks the server and is sent to
  // the login.
  await user.page.evaluate(() => {
    const csrf = decodeURIComponent(
      document.cookie.match(/hardened-blikvm-csrf=([^;]+)/)?.[1] ?? "",
    );
    window.__quicProbeReport({ type: "logout", at: performance.now() });
    void fetch("/api/auth/logout", {
      method: "POST",
      credentials: "include",
      headers: { "x-csrf-token": csrf },
    });
  });
  const closing = await until("the session is told it ends", () =>
    reports.find((report) => report.type === "closing"),
  );
  const loggedOut = reports.find((report) => report.type === "logout");
  await until("the page asks for a login", () =>
    /\/auth\/login/.test(user.page.url()),
  );
  step("logout", {
    closing: closing.reason,
    afterMs: Math.round(closing.at - loggedOut.at),
    page: new URL(user.page.url()).hash,
  });
  assert.equal(closing.reason, "session");

  // The other login switches QUIC off.
  await openSettings(keeper.page);
  assert.equal(await switchIsOn(keeper.page), true);
  const certificate = (
    await call(keeper, "POST", "/api/transport/quic/session", {
      pageTime: Date.now(),
    })
  ).data;
  assert.equal(certificate.granted, true);
  const offStarted = now();
  await keeper.page.getByTestId("transport-device-switch").click();
  await until(
    "the switch shows off",
    async () => !(await switchIsOn(keeper.page)),
  );
  switchedOn = false;
  const off = await transport(keeper);
  step("switched off", { ms: Math.round(now() - offStarted), quic: off });
  assert.deepEqual(
    [off.enabled, off.active, off.connected],
    [false, false, false],
  );
  assert.equal(await quicIsOffered(keeper.page), false);

  // Enforced twice: the port answers nothing to a page that holds the
  // certificate's hash and a token from before, and the server issues no
  // token.
  const port = await keeper.page.evaluate(
    async ([number, hex]) => {
      const started = performance.now();
      const hash = Uint8Array.from(hex.match(/../g), (pair) =>
        Number.parseInt(pair, 16),
      );
      const session = new WebTransport(
        `https://${location.hostname}:${number}/kvm`,
        { serverCertificateHashes: [{ algorithm: "sha-256", value: hash }] },
      );
      session.closed.catch(() => undefined);
      const answered = await Promise.race([
        session.ready.then(
          () => true,
          () => false,
        ),
        new Promise((resolve) => setTimeout(() => resolve(false), 6000)),
      ]);
      try {
        session.close();
      } catch {
        // Never opened.
      }
      return { answered, afterMs: Math.round(performance.now() - started) };
    },
    [certificate.port, certificate.certificateSha256],
  );
  const refused = await call(keeper, "POST", "/api/transport/quic/session", {
    pageTime: Date.now(),
  });
  step("after switching off", { port, session: refused.data });
  assert.equal(port.answered, false, "the port still answers");
  assert.deepEqual(refused.data, { granted: false, reason: "unavailable" });

  assert.deepEqual(receipt.hidRequests, [], "no HID request without control");
  assert.deepEqual(receipt.errors, []);
  result = "passed";
} catch (error) {
  receipt.errors.push(String(error?.message ?? error).slice(0, 300));
} finally {
  password = "";
  // The switch is left off, whatever happened.
  if (switchedOn) {
    for (const who of [keeper, user]) {
      if (!who) continue;
      try {
        const answer = await call(who, "POST", "/api/transport/quic/disable");
        receipt.cleanup = { status: answer.status, quic: answer.data?.quic };
        if (answer.status === 200) break;
      } catch (error) {
        receipt.cleanup = {
          error: String(error?.message ?? error).slice(0, 200),
        };
      }
    }
  }
  if (keeper) {
    await call(keeper, "POST", "/api/auth/logout").catch(() => undefined);
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
