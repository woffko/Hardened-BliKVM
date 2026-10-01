// What the QUIC contracts share: the host's build of the gateway, bound to
// the UDP port with the number of the page's TCP port (so the shipped
// Content-Security-Policy and the gateway's origin check hold as they do on a
// board), the web server's side of the link and its transport API played in
// this process, and the page with its Transport section.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { defaultManifest } from "./app_harness.mjs";

export const GATEWAY =
  process.env.BLI_KVM_QUIC_GATEWAY ??
  path.resolve("target/debug/bli-kvm-quic-gateway");
assert.ok(fs.existsSync(GATEWAY), `the gateway is not built: ${GATEWAY}`);

// Playwright and its Chromium, where the installer puts them for this user.
export async function launch() {
  const { chromium } = await import(
    pathToFileURL(
      process.env.PLAYWRIGHT_MODULE ??
        path.join(
          os.homedir(),
          ".local/share/playwright/node_modules/playwright/index.mjs",
        ),
    ).href
  );
  return chromium.launch({
    headless: true,
    executablePath:
      process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH ??
      path.join(
        os.homedir(),
        ".cache/ms-playwright/chromium-1228/chrome-linux64/chrome",
      ),
  });
}

export const CHOICE_KEY = "hardened-blikvm-transport";
export const PATIENCE = 15000;
export const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
// Spans are measured on the monotonic clock: a host whose wall clock is
// stepped while the contract runs must not lengthen them.
export const now = () => performance.now();

export async function until(what, condition, patience = PATIENCE) {
  const deadline = now() + patience;
  for (;;) {
    const value = await condition();
    if (value) return value;
    assert.ok(now() < deadline, `timed out: ${what}`);
    await sleep(25);
  }
}

// The board as the page and the gateway see it: the transport API of the web
// server, its side of the link, and the gateway's unit.
export class Board {
  /** `port`: the page's TCP port, and so the gateway's UDP port. */
  constructor(port) {
    this.port = port;
    this.directory = fs.mkdtempSync(path.join(os.tmpdir(), "quic-contract-"));
    this.socketPath = path.join(this.directory, "link.sock");
    // The switch, and what the firewall mode does to the port.
    this.enabled = false;
    this.blocked = false;
    // What the session endpoint and the link are made to do wrong.
    this.wrongHash = false;
    this.unknownToken = false;
    this.clock = false;
    this.tooManySessions = false;
    this.loggedOut = false;
    // What the gateway said about itself, `null` while it is not on the link.
    this.hello = null;
    this.connection = null;
    this.child = null;
    this.stderr = "";
    this.tokens = new Set();
    this.sessions = [];
    // What was seen.
    this.redeemed = [];
    this.closed = [];
    this.sessionRequests = [];
    this.sessionRequestTimes = [];
    this.grants = [];
    // The lease requests that came from input streams, how long the server
    // takes over one, and what it answers when not the usual.
    this.leases = [];
    this.leaseDelayMs = 0;
    this.leaseAnswer = null;
    this.link = net.createServer((socket) => this.serve(socket));
  }

  listen() {
    return new Promise((resolve) => this.link.listen(this.socketPath, resolve));
  }

  say(message) {
    const body = Buffer.from(JSON.stringify(message), "utf8");
    const prefix = Buffer.alloc(4);
    prefix.writeUInt32BE(body.length);
    this.connection?.write(Buffer.concat([prefix, body]));
  }

  serve(socket) {
    this.connection = socket;
    let buffer = Buffer.alloc(0);
    socket.on("data", (chunk) => {
      buffer = Buffer.concat([buffer, chunk]);
      while (buffer.length >= 4) {
        const length = buffer.readUInt32BE(0);
        if (buffer.length < 4 + length) return;
        const message = JSON.parse(buffer.subarray(4, 4 + length).toString());
        buffer = buffer.subarray(4 + length);
        this.hear(message);
      }
    });
    const gone = () => {
      if (this.connection !== socket) return;
      this.connection = null;
      this.hello = null;
      this.tokens.clear();
      this.sessions = [];
    };
    socket.on("close", gone);
    socket.on("error", gone);
  }

  hear(message) {
    switch (message.t) {
      case "hello":
        assert.equal(message.protocol, 1);
        assert.match(message.certificateSha256, /^[0-9a-f]{64}$/);
        this.hello = message;
        this.say({ t: "hello", protocol: 1 });
        break;
      case "redeem":
        this.redeemed.push(message.token);
        if (this.tooManySessions) {
          this.say({ t: "redeemed", id: message.id, refused: "sessions" });
        } else if (this.tokens.delete(message.token)) {
          const session = crypto.randomBytes(16).toString("hex");
          this.sessions.push(session);
          this.say({ t: "redeemed", id: message.id, session });
        } else {
          this.say({ t: "redeemed", id: message.id, refused: "token" });
        }
        break;
      case "closed":
        this.closed.push(message.session);
        this.sessions = this.sessions.filter((one) => one !== message.session);
        break;
      case "lease":
        this.leases.push({
          session: message.session,
          request: message.request,
        });
        this.answerLease(message);
        break;
      default:
        assert.fail(`unexpected link message: ${JSON.stringify(message)}`);
    }
  }

  /**
   * The server's answer to a lease request from a session's input stream:
   * the lease API's status and body. `leaseAnswer` may answer otherwise.
   */
  answerLease(message) {
    const request = message.request;
    const granted = {
      status: 200,
      body: {
        code: 0,
        msg: "success",
        data:
          request.action === "acquire"
            ? {
                applied: true,
                leaseId: "b".repeat(32),
                ttlSeconds: request.ttlSeconds,
              }
            : { applied: true },
      },
    };
    const response = this.leaseAnswer?.(request) ?? granted;
    const answer = () =>
      this.say({ t: "lease_answer", id: message.id, response });
    if (this.leaseDelayMs > 0) setTimeout(answer, this.leaseDelayMs);
    else answer();
  }

  /**
   * The web session ended: the server ends its QUIC sessions. The gateway
   * does not report those as closed; the server already knows.
   */
  endSessions() {
    for (const session of this.sessions)
      this.say({ t: "end", session, reason: "session" });
    this.sessions = [];
  }

  /** The unit is started; the gateway reaches the link by itself. */
  startGateway() {
    assert.equal(this.child, null, "the gateway is already running");
    this.child = spawn(GATEWAY, [], {
      env: {
        BLI_KVM_QUIC_BIND: `127.0.0.1:${this.port}`,
        BLI_KVM_QUIC_LINK_SOCKET: this.socketPath,
      },
      stdio: ["ignore", "ignore", "pipe"],
    });
    this.child.stderr.on("data", (chunk) => (this.stderr += chunk));
  }

  async stopGateway(signal = "SIGTERM") {
    const child = this.child;
    if (!child) return;
    this.child = null;
    const exited = new Promise((resolve) => child.once("exit", resolve));
    child.kill(signal);
    await exited;
    await until("the link notices the gateway is gone", () => !this.hello);
  }

  async gatewayOnTheLink() {
    await until("the gateway reaches the link", () => this.hello);
  }

  view() {
    return {
      quic: {
        supported: true,
        enabled: this.enabled,
        active: this.child !== null,
        connected: this.hello !== null,
        blocked: this.blocked,
        ...(this.hello ? { port: this.hello.port } : {}),
      },
    };
  }

  /** `POST /api/transport/quic/session`. */
  session(body) {
    this.sessionRequests.push(body);
    this.sessionRequestTimes.push(now());
    if (!this.hello) return { granted: false, reason: "unavailable" };
    if (this.blocked) return { granted: false, reason: "blocked" };
    if (this.clock) return { granted: false, reason: "clock" };
    const token = crypto.randomBytes(32).toString("hex");
    if (!this.unknownToken) this.tokens.add(token);
    const grant = {
      granted: true,
      port: this.hello.port,
      certificateSha256: this.wrongHash
        ? "0".repeat(64)
        : this.hello.certificateSha256,
      token,
      expiresInSeconds: 30,
    };
    this.grants.push(grant);
    return grant;
  }

  async install(page) {
    const feature = (availability) => ({
      availability,
      evidence: "source_verified",
    });
    const manifest = defaultManifest({
      board: {
        features: {
          network_quic: feature("read_write"),
          system_settings: feature("read_only"),
        },
      },
    });
    await page.route("**/api/capabilities", (route) =>
      route.fulfill({ json: { code: 0, data: manifest } }),
    );
    // A web session that ended is refused everything.
    const unauthorized = (route) =>
      route.fulfill({ status: 401, json: { code: -1, msg: "unauthorized" } });
    await page.route("**/api/transport", (route) =>
      this.loggedOut
        ? unauthorized(route)
        : route.fulfill({ json: { code: 0, data: this.view() } }),
    );
    await page.route("**/api/transport/quic/enable", (route) => {
      this.enabled = true;
      this.startGateway();
      return route.fulfill({ json: { code: 0, data: this.view() } });
    });
    await page.route("**/api/transport/quic/disable", async (route) => {
      // The owner stops the unit and disables it before it answers.
      await this.stopGateway();
      this.enabled = false;
      return route.fulfill({ json: { code: 0, data: this.view() } });
    });
    await page.route("**/api/transport/quic/session", (route) => {
      const body = route.request().postDataJSON();
      if (this.loggedOut) {
        this.sessionRequests.push(body);
        return unauthorized(route);
      }
      return route.fulfill({ json: { code: 0, data: this.session(body) } });
    });
  }

  async close() {
    await this.stopGateway("SIGKILL").catch(() => undefined);
    this.connection?.destroy();
    await new Promise((resolve) => this.link.close(resolve));
    fs.rmSync(this.directory, { recursive: true, force: true });
  }
}

/** A page of the board, logged in, with the browser's stored choice. */
export async function openPage(
  browser,
  app,
  board,
  { choice = "standard", mobile = false, prepare } = {},
) {
  const context = await browser.newContext(
    mobile
      ? {
          viewport: { width: 390, height: 844 },
          isMobile: true,
          hasTouch: true,
        }
      : { viewport: { width: 1280, height: 720 } },
  );
  await context.addCookies([
    { name: "hardened-blikvm-csrf", value: "fixture-csrf", url: app.baseUrl },
  ]);
  // The browser's choice as an earlier visit left it; a reload keeps what the
  // page stored since.
  await context.addInitScript(
    ([key, value]) => {
      if (localStorage.getItem(key) === null) localStorage.setItem(key, value);
    },
    [CHOICE_KEY, choice],
  );
  if (prepare) await prepare(context);
  const page = await context.newPage();
  page.setDefaultTimeout(PATIENCE);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await board.install(page);
  await page.goto(app.baseUrl, { waitUntil: "load" });
  await page.locator("svg.lucide-settings").first().waitFor();
  return { context, page, errors };
}

export async function openSettings(page) {
  await page.locator("svg.lucide-settings").first().click();
  await page.locator('[data-settings-tab="system"]').click();
  await page.locator('[data-testid="transport-settings"]').waitFor();
  // The device's state has arrived once the switch stops spinning.
  await page.waitForFunction(
    () =>
      !document
        .querySelector('[data-testid="transport-device-switch"]')
        .classList.contains("ant-switch-loading"),
  );
}

/** The Transport section says the session is open and answers pings. */
export async function connected(page) {
  await page.waitForFunction(() => {
    const line = document.querySelector('[data-testid="transport-status"]');
    return (
      line?.getAttribute("data-state") === "connected" &&
      /round trip \d+\.\d ms/.test(line.textContent)
    );
  });
  assert.match(
    await page.locator('[data-testid="transport-status"]').innerText(),
    /^The QUIC session is open: round trip \d+\.\d ms, lost datagrams 0, key-frame requests 0\.$/,
  );
}
