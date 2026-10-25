// What the installed Firefox makes of the QUIC transport, on a board or on
// the host's own stack: whether it has WebTransport, whether a session opens
// with the gateway's certificate hash (the board's certificate is its own),
// whether WebCodecs decodes H.264, and what the page itself does with QUIC
// chosen and H.264 the Screen menu's mode.
//
// Firefox is driven through its own geckodriver (WebDriver), headless;
// Playwright has no part in it. The probe logs in once, watches, and asks
// for no lease. The board's QUIC switch is turned on through the API if it
// is off, and is left as it was found. The GUI password is read from stdin;
// the receipt is written under evidence/private, without tokens.
//
// Usage: node quic_firefox_probe.mjs BASE_URL USERNAME OUTPUT
//   FIREFOX_GECKODRIVER names the driver (default: geckodriver on the PATH).
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";

const [baseUrl, username, outputValue] = process.argv.slice(2);
const board = new URL(baseUrl);
// Without TLS only this host's own stack is talked to.
assert.ok(
  board.protocol === "https:" ||
    (board.protocol === "http:" && board.hostname === "127.0.0.1"),
);
const output = path.resolve(outputValue);
assert.equal(path.dirname(output), path.resolve("evidence/private"));
assert.ok(!fs.existsSync(output));
let password = "";
for await (const chunk of process.stdin) password += chunk.toString("utf8");
password = password.replace(/[\r\n]+$/, "");
assert.ok(password.length >= 8);

const PATIENCE = 60000;
const now = () => performance.now();
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(what, condition, patience = PATIENCE) {
  const deadline = now() + patience;
  for (;;) {
    const value = await condition();
    if (value) return value;
    assert.ok(now() < deadline, `timed out: ${what}`);
    await sleep(250);
  }
}
async function freeTcpPort() {
  const server = net.createServer();
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address();
  await new Promise((resolve) => server.close(resolve));
  return port;
}

const receipt = {
  schemaVersion: 1,
  probe: "quic-firefox",
  steps: [],
};
const step = (name, detail = {}) => {
  receipt.steps.push({ step: name, ...detail });
  console.log(`${name}: ${JSON.stringify(detail)}`);
};

const driverPort = await freeTcpPort();
const driver = spawn(
  process.env.FIREFOX_GECKODRIVER ?? "geckodriver",
  ["--port", String(driverPort)],
  { stdio: ["ignore", "ignore", "ignore"] },
);
let driverExited = false;
driver.once("exit", () => (driverExited = true));
const driverUrl = `http://127.0.0.1:${driverPort}`;

/** A WebDriver command: its value, or an error with the driver's message. */
async function command(method, route, body) {
  const response = await fetch(`${driverUrl}${route}`, {
    method,
    headers: { "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const answer = await response.json();
  if (!response.ok) {
    throw new Error(
      `${route.replace(/\/session\/[^/]+/, "")}: ${String(answer.value?.message).slice(0, 300)}`,
    );
  }
  return answer.value;
}

let session = null;
let result = "failed";
let loggedIn = false;
let switchedOn = false;
/** Runs `body` in the page; it may await, and returns what it returns. */
const inPage = (body, ...args) =>
  command("POST", `/session/${session}/execute/async`, {
    script: `const done = arguments[arguments.length - 1];
      (async (...args) => { ${body} })(...Array.from(arguments).slice(0, -1))
        .then((value) => done({ value }), (error) => done({ error: String(error && error.message || error) }));`,
    args,
  }).then((answer) => {
    if (answer?.error) throw new Error(answer.error);
    return answer?.value;
  });
// A request of the login from the page, with its CSRF token from the cookie.
const api = (method, route, body) =>
  inPage(
    `const [method, route, body] = args;
     const csrf = decodeURIComponent(document.cookie.match(/hardened-blikvm-csrf=([^;]+)/)?.[1] ?? "");
     const response = await fetch(route, {
       method,
       credentials: "include",
       headers: { "Content-Type": "application/json", "x-csrf-token": csrf },
       body: body === null ? undefined : JSON.stringify(body),
     });
     let parsed = null;
     try { parsed = await response.json(); } catch {}
     return { status: response.status, body: parsed };`,
    method,
    route,
    body ?? null,
  );

try {
  await until(
    "the driver listens",
    async () => {
      assert.ok(!driverExited, "geckodriver ended");
      return fetch(`${driverUrl}/status`).then(
        (response) => response.ok,
        () => false,
      );
    },
    20000,
  );
  const created = await command("POST", "/session", {
    capabilities: {
      alwaysMatch: {
        acceptInsecureCerts: true,
        "moz:firefoxOptions": { args: ["-headless"] },
      },
    },
  });
  session = created.sessionId;
  receipt.browser = `${created.capabilities.browserName} ${created.capabilities.browserVersion}`;
  step("browser", { browser: receipt.browser });
  await command("POST", `/session/${session}/timeouts`, {
    script: 60000,
    pageLoad: 60000,
  });
  await command("POST", `/session/${session}/url`, { url: `${board.origin}/` });

  // The one login, from the page. Its CSRF token stays in the browser, in
  // the cookie the application keeps it in.
  const login = await inPage(
    `const [username, password] = args;
     const response = await fetch("/api/auth/login", {
       method: "POST",
       credentials: "include",
       headers: { "Content-Type": "application/json" },
       body: JSON.stringify({ username, password }),
     });
     let token = null;
     try { token = (await response.json()).data.csrfToken; } catch {}
     if (token) {
       document.cookie = "hardened-blikvm-csrf=" + encodeURIComponent(token) +
         "; path=/; SameSite=Strict" + (location.protocol === "https:" ? "; Secure" : "");
     }
     return { status: response.status, token: Boolean(token) };`,
    username,
    password,
  );
  password = "";
  assert.deepEqual(login, { status: 200, token: true }, "login failed");
  loggedIn = true;

  const found = (await api("GET", "/api/transport")).body.data.quic;
  step("found", { quic: found });
  assert.ok(found.supported, "this board has no QUIC transport");
  if (!found.enabled && board.protocol === "https:") {
    switchedOn = true;
    assert.equal((await api("POST", "/api/transport/quic/enable")).status, 200);
  }
  await until(
    "the gateway reaches the server",
    async () => (await api("GET", "/api/transport")).body.data.quic.connected,
  );

  // What the browser has, and a session of its own with the certificate's
  // hash.
  receipt.features = await inPage(
    `const features = {
       secureContext: window.isSecureContext,
       webTransport: typeof WebTransport !== "undefined",
       videoDecoder: typeof VideoDecoder !== "undefined",
       h264: null,
       session: null,
     };
     if (features.videoDecoder) {
       try {
         const support = await VideoDecoder.isConfigSupported({ codec: "avc1.42e028", optimizeForLatency: true });
         features.h264 = support.supported === true;
       } catch (error) {
         features.h264 = String(error && error.message || error).slice(0, 200);
       }
     }
     if (features.webTransport) {
       const csrf = decodeURIComponent(document.cookie.match(/hardened-blikvm-csrf=([^;]+)/)?.[1] ?? "");
       const grant = (await (await fetch("/api/transport/quic/session", {
         method: "POST",
         credentials: "include",
         headers: { "Content-Type": "application/json", "x-csrf-token": csrf },
         body: JSON.stringify({ pageTime: Date.now() }),
       })).json()).data;
       if (!grant.granted) {
         features.session = "no token: " + grant.reason;
       } else {
         const hash = Uint8Array.from(grant.certificateSha256.match(/../g), (pair) => Number.parseInt(pair, 16));
         const started = performance.now();
         try {
           const transport = new WebTransport("https://" + location.hostname + ":" + grant.port + "/kvm", {
             serverCertificateHashes: [{ algorithm: "sha-256", value: hash }],
           });
           transport.closed.catch(() => undefined);
           const outcome = await Promise.race([
             transport.ready.then(() => "ready", (error) => "refused: " + String(error && error.message || error).slice(0, 200)),
             new Promise((resolve) => setTimeout(() => resolve("no answer in 8 s"), 8000)),
           ]);
           features.session = outcome + " after " + Math.round(performance.now() - started) + " ms";
           try { transport.close(); } catch {}
         } catch (error) {
           features.session = "constructor: " + String(error && error.message || error).slice(0, 200);
         }
       }
     }
     return features;`,
  );
  step("features", receipt.features);

  // The page itself, with QUIC chosen and H.264 the Screen menu's mode.
  await inPage(
    `localStorage.setItem("hardened-blikvm-transport", "quic");
     localStorage.setItem("hardened-blikvm-video-mode", "h264");
     localStorage.setItem("hardened-blikvm-menu-auto-hide", "always");
     return true;`,
  );
  await command("POST", `/session/${session}/url`, { url: `${board.origin}/` });
  const look = () =>
    inPage(
      `const screen = document.querySelector("#screen");
       const notice = document.querySelector('[data-testid="transport-notice"]');
       const status = document.querySelector('[data-testid="transport-status"]');
       const video = document.querySelector('[data-testid="transport-video"]');
       return {
         menu: document.querySelector('[data-testid="menu-bar"]') !== null,
         screen: screen ? screen.tagName.toLowerCase() : null,
         overQuic: screen?.getAttribute("data-video-transport") === "quic",
         frames: screen?.dataset.quicFrames ? Number(screen.dataset.quicFrames) : null,
         framesPerSecond: screen?.dataset.quicFps ? Number(screen.dataset.quicFps) : null,
         ageMedianMs: screen?.dataset.quicAgeMedian ? Number(screen.dataset.quicAgeMedian) : null,
         keyFrameRequests: screen?.dataset.quicRequests ? Number(screen.dataset.quicRequests) : null,
         notice: notice ? notice.getAttribute("data-reason") : null,
         status: status ? { state: status.getAttribute("data-state"), says: status.textContent } : null,
         video: video ? { state: video.getAttribute("data-video"), says: video.textContent } : null,
       };`,
    );
  await until("the page is there", async () => (await look()).menu);
  // A notice of a failed session stays for a few seconds only: the first
  // look counts.
  const notices = new Set();
  const note = (seen) => {
    if (seen.notice) notices.add(seen.notice);
    return seen;
  };
  note(await look());
  // Settings > Network > Transport says what the session and the picture do.
  const click = async (selector) => {
    const element = await command("POST", `/session/${session}/element`, {
      using: "css selector",
      value: selector,
    });
    await command(
      "POST",
      `/session/${session}/element/${Object.values(element)[0]}/click`,
      {},
    );
  };
  await click("svg.lucide-settings");
  await until(
    "Settings opens",
    () =>
      inPage(
        `return document.querySelector('[data-settings-tab="system"]') !== null;`,
      ),
    10000,
  );
  await click('[data-settings-tab="system"]');
  // Long enough for a session that gets no answer to give up and fall back.
  const deadline = now() + 40000;
  let seen = note(await look());
  while (now() < deadline) {
    await sleep(1000);
    seen = note(await look());
    if (seen.status?.state === "failed") break;
    if (
      seen.status?.state === "connected" &&
      seen.video &&
      seen.video.state !== "starting" &&
      (seen.video.state !== "quic" || (seen.frames ?? 0) >= 3)
    )
      break;
  }
  receipt.notices = [...notices];
  receipt.page = seen;
  step("page", seen);
  await command("POST", `/session/${session}/url`, { url: "about:blank" });
  result = "passed";
} catch (error) {
  receipt.error = String(error?.message ?? error).slice(0, 400);
  console.error(receipt.error);
} finally {
  // The switch as it was found, and the login ended.
  if (session && loggedIn) {
    try {
      await command("POST", `/session/${session}/url`, {
        url: `${board.origin}/`,
      });
      if (switchedOn) {
        const off = await api("POST", "/api/transport/quic/disable");
        receipt.switchedOff = off.status === 200;
        const state = (await api("GET", "/api/transport")).body?.data?.quic;
        receipt.left = { enabled: state?.enabled, connected: state?.connected };
        if (state?.enabled !== false) result = "failed";
      }
      await api("POST", "/api/auth/logout", {});
    } catch (error) {
      receipt.cleanupError = String(error?.message ?? error).slice(0, 200);
      result = "failed";
    }
  }
  if (session) {
    await command("DELETE", `/session/${session}`).catch(() => undefined);
  }
  if (!driverExited) {
    await new Promise((resolve) => {
      driver.once("exit", resolve);
      driver.kill("SIGTERM");
    });
  }
  receipt.result = result;
  fs.writeFileSync(output, JSON.stringify(receipt, null, 1));
}
console.log(`result: ${result}`);
process.exit(result === "passed" ? 0 : 1);
