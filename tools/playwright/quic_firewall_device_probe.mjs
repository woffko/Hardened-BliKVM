// On a board: the QUIC transport's port in the firewall modes that close
// WebRTC. Restricted admits UDP 443 from the local ranges; Paranoid does not.
//
// The probe applies Restricted and then Paranoid through the API. In each it
// reads the firewall status and the owned table's listing, asks the server
// for a session, and opens a connection to the gateway from a page of the
// board:
// - Restricted: the status says the port is not blocked, the table has the
//   rule, a session opens and its pings are answered; then the page itself,
//   with QUIC chosen and H.264 the Screen menu's mode, draws the picture
//   from the session and offers no WebRTC mode;
// - Paranoid: the status says the port is blocked, the table has no such
//   rule, the server refuses a session with `blocked`, and a connection with
//   the certificate's hash from before gets no answer.
//
// A mode is confirmed after its first checks, and the mode found at the
// start is applied again at the end, whatever happens. A probe that dies
// before a confirmation is undone by the owner after 60 s. The probe refuses
// a board that is in a restrictive mode already, or has a change pending.
// The QUIC switch is turned on if it is off and left as it was found. No
// lease is asked for: any /api/hid request fails the probe. The GUI password
// is read from stdin; a receipt is written under evidence/private, without
// tokens.
//
// Usage: node quic_firewall_device_probe.mjs BASE_URL USERNAME OUTPUT
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { watchingMayAsk } from "./app_harness.mjs";

const [baseUrl, username, outputValue] = process.argv.slice(2);
assert.equal(new URL(baseUrl).protocol, "https:");
const origin = new URL(baseUrl).origin;
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

const PATIENCE = 30000;
const PINGS = 10;
const now = () => performance.now();
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(what, condition, patience = PATIENCE) {
  const deadline = now() + patience;
  for (;;) {
    const value = await condition();
    if (value) return value;
    assert.ok(now() < deadline, `timed out: ${what}`);
    await sleep(200);
  }
}

const receipt = {
  schemaVersion: 1,
  probe: "quic-firewall-device",
  browser: browser.version(),
  steps: [],
  hidRequests: {},
  errors: [],
};
const step = (name, detail = {}) => {
  receipt.steps.push({ step: name, ...detail });
  console.log(`${name}: ${JSON.stringify(detail).slice(0, 700)}`);
};

let result = "failed";
let context = null;
let csrf = "";
let initialMode = null;
let switchedOn = false;

// A request of the login, with its CSRF token.
async function call(method, route, data) {
  const response = await context.request.fetch(`${origin}${route}`, {
    method,
    data,
    headers: { Origin: origin, "X-CSRF-Token": csrf },
    timeout: 60000,
  });
  const body = await response.json().catch(() => null);
  return { status: response.status(), body };
}
const modeStatus = async () =>
  (await call("GET", "/api/system/firewall/mode")).body?.data ?? null;

/** Applies a mode; a restrictive one then waits for its confirmation. */
async function apply(mode) {
  const change = await call("POST", "/api/system/firewall/mode", {
    mode,
    confirm: true,
  });
  assert.equal(change.status, 200, `the ${mode} mode was refused`);
  return until(`the ${mode} mode is in force`, async () => {
    const status = await modeStatus();
    return status?.mode === mode ? status : null;
  });
}
async function confirm(status) {
  if (!status.pendingOperationId) return;
  const confirmed = await call("POST", "/api/system/firewall/mode/confirm", {
    operationId: status.pendingOperationId,
  });
  assert.equal(confirmed.status, 200, "the confirmation was refused");
}
/**
 * The firewall page's status once it names `mode`. The server keeps its
 * status for five seconds and reads it again right after a change, which
 * may be before the owner's table has the new rules: a read straight after
 * a change can be the mode before it.
 */
async function firewallViewOf(mode) {
  let view = null;
  await until(`the status says ${mode}`, async () => {
    view = await firewallView();
    return view.effectiveMode === mode;
  });
  return view;
}
/** What the firewall page's status says, and whether the table has the rule. */
async function firewallView() {
  const data = (await call("GET", "/api/system/firewall")).body?.data ?? {};
  const listing = String(data.rules?.nft ?? "");
  return {
    effectiveMode: data.effectiveMode,
    webrtcBlocked: data.webrtcBlocked,
    quicBlocked: data.quicBlocked,
    httpsPort: data.httpsPort,
    // The owned table's rule for the QUIC transport's port.
    quicRule: new RegExp(`udp dport ${data.httpsPort ?? 443} accept`).test(
      listing,
    ),
    listingBytes: listing.length,
  };
}

// In a page of the board: a session of this login, its pings answered; or a
// bare connection with a known hash, to see whether the port answers at all.
async function inPage([kind, pings, known]) {
  const csrf = decodeURIComponent(
    document.cookie.match(/hardened-blikvm-csrf=([^;]+)/)?.[1] ?? "",
  );
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
  const connect = async (port, hashHex) => {
    const hash = Uint8Array.from(hashHex.match(/../g), (pair) =>
      Number.parseInt(pair, 16),
    );
    const started = performance.now();
    const transport = new WebTransport(
      `https://${location.hostname}:${port}/kvm`,
      { serverCertificateHashes: [{ algorithm: "sha-256", value: hash }] },
    );
    transport.closed.catch(() => undefined);
    const outcome = await Promise.race([
      transport.ready.then(
        () => "ready",
        () => "refused",
      ),
      new Promise((resolve) => setTimeout(() => resolve("silent"), 7000)),
    ]);
    return {
      transport,
      outcome,
      afterMs: Math.round(performance.now() - started),
    };
  };

  if (kind === "bare") {
    const { transport, outcome, afterMs } = await connect(
      known.port,
      known.hash,
    );
    try {
      transport.close();
    } catch {
      // Never opened.
    }
    return { outcome, afterMs };
  }

  const answer = await fetch("/api/transport/quic/session", {
    method: "POST",
    credentials: "include",
    headers: { "Content-Type": "application/json", "x-csrf-token": csrf },
    body: JSON.stringify({ pageTime: Date.now() }),
  });
  const grant = (await answer.json()).data;
  if (!grant.granted) return { granted: false, reason: grant.reason };
  const { transport, outcome, afterMs } = await connect(
    grant.port,
    grant.certificateSha256,
  );
  if (outcome !== "ready") {
    return { granted: true, outcome, afterMs, port: grant.port };
  }
  const control = await transport.createBidirectionalStream();
  const writer = control.writable.getWriter();
  const hear = reading(control.readable);
  await writer.write(frame({ t: "hello", protocol: 2, token: grant.token }));
  const welcome = await hear();
  const trips = [];
  if (welcome?.t === "welcome") {
    for (let id = 1; id <= pings; id += 1) {
      const sent = performance.now();
      await writer.write(frame({ t: "ping", id, pageTime: sent }));
      for (;;) {
        const message = await hear();
        if (!message) break;
        if (message.t === "pong" && message.id === id) {
          trips.push(performance.now() - sent);
          break;
        }
      }
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
  }
  await writer.write(frame({ t: "bye" })).catch(() => undefined);
  transport.close();
  trips.sort((left, right) => left - right);
  return {
    granted: true,
    outcome,
    afterMs,
    port: grant.port,
    hash: grant.certificateSha256,
    welcome: welcome?.t ?? null,
    pingsAnswered: trips.length,
    roundTripMedianMs:
      trips.length > 0
        ? Math.round(trips[Math.floor(trips.length / 2)] * 10) / 10
        : null,
  };
}

try {
  context = await browser.newContext({
    ignoreHTTPSErrors: true,
    locale: "en-US",
    viewport: { width: 1440, height: 1000 },
  });
  const login = await context.request.post(`${origin}/api/auth/login`, {
    data: { username, password },
    headers: { Origin: origin },
  });
  password = "";
  csrf = (await login.json()).data?.csrfToken;
  assert.ok(csrf, "login failed");

  const foundMode = await modeStatus();
  const foundQuic = (await call("GET", "/api/transport")).body.data.quic;
  step("found", { firewall: foundMode, quic: foundQuic });
  assert.ok(foundMode, "this board has no firewall modes");
  assert.ok(
    ["baseline", "moderate"].includes(foundMode.mode) &&
      !foundMode.pendingOperationId,
    "the board is in a restrictive mode or has a change pending",
  );
  assert.ok(foundQuic.supported, "this board has no QUIC transport");
  initialMode = foundMode.mode;
  if (!foundQuic.enabled) {
    switchedOn = true;
    assert.equal(
      (await call("POST", "/api/transport/quic/enable")).status,
      200,
    );
  }
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
  await page.goto(`${origin}/`, { waitUntil: "domcontentloaded" });
  await page.getByTestId("menu-bar").waitFor({ timeout: 30000 });
  // The token never leaves the page; the hash is the certificate's and is
  // kept for the connection without a token.
  const session = () => page.evaluate(inPage, ["session", PINGS, null]);

  // As found: a session opens. Its certificate's hash serves later.
  const open = await session();
  step(`session in ${initialMode}`, { ...open, hash: undefined });
  assert.equal(open.pingsAnswered, PINGS);
  const known = { port: open.port, hash: open.hash };

  // Restricted: the port is admitted from this (local) address.
  {
    const status = await apply("restricted");
    const view = await firewallViewOf("restricted");
    const transport = (await call("GET", "/api/transport")).body.data.quic;
    const opened = await session();
    step("restricted", {
      view,
      transportBlocked: transport.blocked,
      session: { ...opened, hash: undefined },
    });
    assert.equal(view.quicBlocked, false);
    assert.equal(view.webrtcBlocked, true);
    assert.equal(view.quicRule, true, "the table has no rule for the port");
    assert.equal(transport.blocked, false);
    assert.equal(opened.outcome, "ready");
    assert.equal(opened.pingsAnswered, PINGS);
    await confirm(status);

    // The page itself: H.264 from the session, no WebRTC mode on offer.
    const viewer = await context.newPage();
    viewer.setDefaultTimeout(PATIENCE);
    viewer.on("request", (request) => {
      const route = new URL(request.url()).pathname;
      if (
        route.startsWith("/api/hid") &&
        !watchingMayAsk(request.method(), route)
      ) {
        const key = `${request.method()} ${route}`;
        receipt.hidRequests[key] = (receipt.hidRequests[key] ?? 0) + 1;
      }
    });
    await viewer.addInitScript(() => {
      localStorage.setItem("hardened-blikvm-transport", "quic");
      localStorage.setItem("hardened-blikvm-video-mode", "h264");
      localStorage.setItem("hardened-blikvm-menu-auto-hide", "always");
    });
    await viewer.goto(`${origin}/`, { waitUntil: "domcontentloaded" });
    await viewer.getByTestId("menu-bar").waitFor({ timeout: 30000 });
    const frames = await until("the picture comes over the session", () =>
      viewer.evaluate(() => {
        const canvas = document.querySelector(
          'canvas#screen[data-video-transport="quic"]',
        );
        const drawn = Number(canvas?.dataset.quicFrames ?? 0);
        return drawn >= 2 ? drawn : null;
      }),
    );
    await viewer.locator("svg.lucide-monitor").first().click();
    await viewer.getByText("Video Mode", { exact: true }).hover();
    // The menu reads the firewall's status when it opens.
    await viewer
      .locator('[data-video-mode="vp8"][aria-disabled="true"]')
      .waitFor();
    const described = async (entry) => ({
      name: await entry.locator("[data-video-mode-name]").innerText(),
      disabled: (await entry.getAttribute("aria-disabled")) === "true",
    });
    // H.264 HW and H.264 SW where the board offers the choice of its
    // engine, one H.264 entry where it does not
    // (docs/design/h264-engine-choice-and-quality-monitor-v1.md).
    const h264 = viewer.locator('[data-video-mode="h264"]');
    await h264.first().waitFor();
    const entries = { h264: [], vp8: null };
    for (const entry of await h264.all())
      entries.h264.push(await described(entry));
    entries.vp8 = await described(viewer.locator('[data-video-mode="vp8"]'));
    const remembered = await viewer.evaluate(() =>
      localStorage.getItem("hardened-blikvm-video-mode"),
    );
    step("restricted, the page", { frames, entries, remembered });
    const names =
      entries.h264.length === 2
        ? ["H.264 HW (QUIC)", "H.264 SW (QUIC)"]
        : ["H.264 (QUIC)"];
    assert.deepEqual(
      entries.h264,
      names.map((name) => ({ name, disabled: false })),
    );
    assert.equal(entries.vp8.disabled, true);
    assert.equal(remembered, "h264");
    await viewer.goto("about:blank");
    await viewer.close();
  }

  // Paranoid: the port is closed, to the server's knowledge and in fact.
  {
    const status = await apply("paranoid");
    const view = await firewallViewOf("paranoid");
    const refused = await session();
    const bare = await page.evaluate(inPage, ["bare", 0, known]);
    step("paranoid", { view, session: refused, connection: bare });
    assert.equal(view.quicBlocked, true);
    assert.equal(view.quicRule, false, "the table admits the port");
    assert.deepEqual(refused, { granted: false, reason: "blocked" });
    assert.notEqual(bare.outcome, "ready", "the port answered in Paranoid");
    await confirm(status);
  }

  // Back to the mode that was found: a session opens again.
  const back = await apply(initialMode);
  const again = await session();
  step(`back in ${initialMode}`, {
    status: back,
    session: { ...again, hash: undefined },
  });
  assert.equal(again.pingsAnswered, PINGS);
  assert.deepEqual(receipt.hidRequests, {}, "a HID request was made");
  assert.deepEqual(receipt.errors, []);
  await page.goto("about:blank");
  result = "passed";
} catch (error) {
  receipt.error = String(error?.message ?? error).slice(0, 400);
  console.error(receipt.error);
} finally {
  if (context && csrf) {
    try {
      // The mode that was found, whatever state the probe stopped in.
      if (initialMode) {
        let status = await modeStatus();
        if (status?.pendingOperationId) {
          await confirm(status).catch(() => undefined);
          status = await modeStatus();
        }
        if (status?.mode !== initialMode) {
          await call("POST", "/api/system/firewall/mode", {
            mode: initialMode,
            confirm: true,
          });
          await sleep(2000);
          status = await modeStatus();
        }
        receipt.leftFirewall = {
          mode: status?.mode,
          pending: Boolean(status?.pendingOperationId),
        };
        if (status?.mode !== initialMode || status?.pendingOperationId) {
          result = "failed";
        }
      }
      if (switchedOn) {
        const off = await call("POST", "/api/transport/quic/disable");
        receipt.switchedOff = off.status === 200;
      }
      const quic = (await call("GET", "/api/transport")).body?.data?.quic;
      receipt.leftQuic = { enabled: quic?.enabled };
      await call("POST", "/api/auth/logout", {});
    } catch (error) {
      receipt.cleanupError = String(error?.message ?? error).slice(0, 200);
      result = "failed";
    }
  }
  receipt.result = result;
  fs.writeFileSync(output, JSON.stringify(receipt, null, 1));
  await browser.close();
}
console.log(`result: ${result}`);
process.exit(result === "passed" ? 0 : 1);
