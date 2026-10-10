// The QUIC transport's page side against the real gateway
// (docs/design/quic-transport-v1.md): Settings > Network > Transport, the
// session over WebTransport with the certificate's hash, and every row of the
// plan's failure table. The gateway is the host's build of
// bli-kvm-quic-gateway, bound to the UDP port with the number of the page's
// TCP port, so the shipped Content-Security-Policy and the gateway's origin
// check hold as they do on a board. The web server's side of the link and the
// transport API are played here.
import assert from "node:assert/strict";
import dgram from "node:dgram";
import { startApp } from "./app_harness.mjs";
import {
  Board,
  CHOICE_KEY,
  connected,
  launch,
  now,
  openPage,
  openSettings,
  sleep,
  until,
} from "./quic_fixture.mjs";

const browser = await launch();
const app = await startApp();
const PORT = Number(new URL(app.baseUrl).port);
const board = new Board(PORT);
await board.listen();
const open = (options) => openPage(browser, app, board, options);

const status = (page) => page.locator('[data-testid="transport-status"]');
const hindrance = (page) => page.locator('[data-testid="transport-hindrance"]');
const quicOption = (page) =>
  page.locator('[data-testid="transport-choice"] label', { hasText: "QUIC" });
const quicIsOffered = async (page) =>
  !(await quicOption(page).getAttribute("class")).includes(
    "ant-segmented-item-disabled",
  );
const switchIsOn = (page) =>
  page
    .locator('[data-testid="transport-device-switch"]')
    .getAttribute("aria-checked")
    .then((value) => value === "true");
const stored = (page) =>
  page.evaluate((key) => localStorage.getItem(key), CHOICE_KEY);

// The notice a failed session raises: its reason, whether it offers another
// attempt, and that the page says the standard transport is in use.
async function notice(page, reason, { retry }) {
  const text = page.locator(
    `[data-testid="transport-notice"][data-reason="${reason}"]`,
  );
  await text.waitFor();
  assert.match(await text.innerText(), /The standard transport is used\.$/);
  const box = page.locator(".ant-notification-notice", { has: text });
  assert.match(await box.innerText(), /^QUIC is not in use/);
  assert.equal(
    await box.locator('[data-testid="transport-notice-retry"]').count(),
    retry ? 1 : 0,
    `${reason}: retry offered`,
  );
  return box;
}

// Whether anything answers a WebTransport session on the gateway's port.
function portAnswers(page, certificate) {
  return page.evaluate(
    async ([port, hex]) => {
      const value = Uint8Array.from(hex.match(/../g), (pair) =>
        Number.parseInt(pair, 16),
      );
      const transport = new WebTransport(`https://127.0.0.1:${port}/kvm`, {
        serverCertificateHashes: [{ algorithm: "sha-256", value }],
      });
      transport.closed.catch(() => undefined);
      const answer = await Promise.race([
        transport.ready.then(
          () => true,
          () => false,
        ),
        new Promise((resolve) => setTimeout(() => resolve(false), 3000)),
      ]);
      try {
        transport.close();
      } catch {
        // Never opened.
      }
      return answer;
    },
    [PORT, certificate],
  );
}

try {
  // The switch is off by default: only Standard is offered, the page says
  // why, nothing runs and no session is asked for.
  {
    const { context, page, errors } = await open();
    await openSettings(page);
    assert.equal(await switchIsOn(page), false);
    assert.equal(await quicIsOffered(page), false);
    assert.equal(
      await hindrance(page).innerText(),
      "QUIC is switched off on this device.",
    );
    assert.equal(await status(page).getAttribute("data-state"), "off");
    assert.equal(
      await status(page).innerText(),
      "The standard transport is in use.",
    );
    assert.equal(await stored(page), "standard");
    assert.equal(board.child, null);
    assert.deepEqual(board.sessionRequests, []);

    // Switching on asks first, and names the UDP port.
    await page.locator('[data-testid="transport-device-switch"]').click();
    const dialog = page.locator(".ant-modal-confirm");
    await dialog.waitFor();
    assert.match(await dialog.innerText(), /Switch QUIC on\?/);
    assert.match(await dialog.innerText(), /UDP port 443\b/);
    await dialog.getByRole("button", { name: "Cancel" }).click();
    await dialog.waitFor({ state: "detached" });
    assert.equal(board.enabled, false, "a cancelled question switched it on");

    await page.locator('[data-testid="transport-device-switch"]').click();
    await page
      .locator(".ant-modal-confirm")
      .getByRole("button", { name: "Switch on" })
      .click();
    await until("the switch shows on", () => switchIsOn(page));
    await until("QUIC is offered", () => quicIsOffered(page));
    assert.equal(board.enabled, true);
    assert.equal(board.hello.port, PORT);
    assert.equal(await hindrance(page).count(), 0);
    assert.match(
      await page.locator('[data-testid="transport-settings"]').innerText(),
      new RegExp(`answers on UDP port ${PORT}\\.`),
    );
    // Switching the device on does not change what this browser uses.
    assert.equal(await stored(page), "standard");
    assert.deepEqual(board.sessionRequests, []);

    // This browser chooses QUIC: one token, redeemed once, and the session
    // answers pings.
    await quicOption(page).click();
    await connected(page);
    assert.equal(await stored(page), "quic");
    assert.equal(board.sessionRequests.length, 1);
    const pageTime = board.sessionRequests[0].pageTime;
    assert.deepEqual(Object.keys(board.sessionRequests[0]), ["pageTime"]);
    // The page's wall clock, in milliseconds since 1970.
    assert.ok(Math.abs(pageTime - Date.now()) < 60000, `pageTime ${pageTime}`);
    assert.deepEqual(board.redeemed, [board.grants[0].token]);
    assert.equal(board.sessions.length, 1);
    assert.equal(await page.locator(".ant-notification-notice").count(), 0);

    // A reload keeps the choice: the old session is closed, a new one opens
    // with a token of its own.
    const first = board.sessions[0];
    await page.reload({ waitUntil: "load" });
    await until("the first session is closed", () =>
      board.closed.includes(first),
    );
    await openSettings(page);
    await connected(page);
    assert.equal(board.redeemed.length, 2);
    assert.notEqual(board.redeemed[1], board.redeemed[0]);

    // Back to Standard: the session is closed, and stays closed.
    const second = board.sessions[0];
    await page
      .locator('[data-testid="transport-choice"] label', {
        hasText: "Standard",
      })
      .click();
    await until("the session is closed", () => board.closed.includes(second));
    assert.equal(await status(page).getAttribute("data-state"), "off");
    assert.equal(await stored(page), "standard");
    await sleep(300);
    assert.equal(board.redeemed.length, 2);
    assert.deepEqual(errors, []);
    await context.close();
  }

  // A session that ends with the web session: the page is told, does not try
  // again, and asks the server at once, which sends it to the login. Settings
  // stay closed, so nothing but the session can have asked.
  {
    const { context, page, errors } = await open({ choice: "quic" });
    await until("the session opens", () => board.sessions.length === 1);
    const requests = board.sessionRequests.length;
    board.loggedOut = true;
    board.endSessions();
    await until("the page asks for a login", () =>
      /\/auth\/login/.test(page.url()),
    );
    assert.equal(board.sessionRequests.length, requests, "it tried again");
    board.loggedOut = false;
    assert.deepEqual(errors, []);
    await context.close();
  }

  // A phone: the same section, the same session.
  {
    const { context, page, errors } = await open({
      choice: "quic",
      mobile: true,
    });
    await openSettings(page);
    await connected(page);
    assert.equal(await switchIsOn(page), true);
    const overflow = await page.evaluate(() => {
      const section = document.querySelector(
        '[data-testid="transport-settings"]',
      );
      return section.getBoundingClientRect().right - window.innerWidth;
    });
    assert.ok(overflow <= 0, `the section leaves the page by ${overflow} px`);
    assert.deepEqual(errors, []);
    await context.close();
  }

  // The token is refused: the reason is shown, nothing is tried again by
  // itself, and the operator's own retry works once the token is good.
  {
    board.unknownToken = true;
    const requests = board.sessionRequests.length;
    const redeemed = board.redeemed.length;
    const { context, page, errors } = await open({ choice: "quic" });
    const box = await notice(page, "token", { retry: false });
    assert.match(await box.innerText(), /The session token was refused\./);
    await sleep(1500);
    assert.equal(board.sessionRequests.length, requests + 1);
    assert.equal(board.redeemed.length, redeemed + 1);
    await box.locator(".ant-notification-notice-close").click();
    await openSettings(page);
    assert.equal(await status(page).getAttribute("data-state"), "failed");
    assert.equal(
      await status(page).innerText(),
      "The session token was refused. The standard transport is used.",
    );
    // QUIC stays the choice and can be left.
    assert.equal(await stored(page), "quic");
    board.unknownToken = false;
    await page.locator('[data-testid="transport-retry"]').click();
    await connected(page);
    assert.equal(
      await page.locator('[data-testid="transport-retry"]').count(),
      0,
    );
    assert.deepEqual(errors, []);
    await context.close();
  }

  // The certificate's hash does not match: one fresh attempt with a new
  // token and hash, then the error; no token reaches the gateway.
  {
    board.wrongHash = true;
    const requests = board.sessionRequests.length;
    const redeemed = board.redeemed.length;
    const { context, page, errors } = await open({ choice: "quic" });
    const box = await notice(page, "handshake", { retry: false });
    assert.match(
      await box.innerText(),
      /was not accepted \(certificate or limit\)/,
    );
    await sleep(1500);
    assert.equal(board.sessionRequests.length, requests + 2);
    assert.equal(board.redeemed.length, redeemed);
    board.wrongHash = false;
    assert.deepEqual(errors, []);
    await context.close();
  }

  // The gateway speaks another version than the page.
  {
    const redeemed = board.redeemed.length;
    const { context, page, errors } = await open({
      choice: "quic",
      prepare: (context) =>
        context.addInitScript(() => {
          // The page's hello leaves with another protocol number.
          const Real = window.WebTransport;
          window.WebTransport = class extends Real {
            async createBidirectionalStream(...args) {
              const stream = await super.createBidirectionalStream(...args);
              const writer = stream.writable.getWriter();
              let first = true;
              const writable = new WritableStream({
                write(chunk) {
                  if (!first) return writer.write(chunk);
                  first = false;
                  const hello = JSON.parse(
                    new TextDecoder().decode(chunk.subarray(2)),
                  );
                  hello.protocol += 1;
                  const body = new TextEncoder().encode(JSON.stringify(hello));
                  const out = new Uint8Array(2 + body.length);
                  out[0] = body.length >> 8;
                  out[1] = body.length & 0xff;
                  out.set(body, 2);
                  return writer.write(out);
                },
                close: () => writer.close(),
                abort: (reason) => writer.abort(reason),
              });
              return { readable: stream.readable, writable };
            }
          };
        }),
    });
    const box = await notice(page, "protocol", { retry: false });
    assert.match(
      await box.innerText(),
      /another version of the QUIC transport/,
    );
    // The gateway refused before it asked the server about the token.
    assert.equal(board.redeemed.length, redeemed);
    assert.deepEqual(errors, []);
    await context.close();
  }

  // The web session already has its QUIC sessions: said. A session lost on
  // a bad path counts until the gateway lets it go, so the page tries again
  // by itself once it is looked at again (and every 35 s while it is): the
  // session opens once the sessions are let go, and the notice goes.
  {
    board.tooManySessions = true;
    const { context, page, errors } = await open({ choice: "quic" });
    await notice(page, "sessions", { retry: false });
    const requests = board.sessionRequests.length;
    board.tooManySessions = false;
    await page.evaluate(() =>
      document.dispatchEvent(new Event("visibilitychange")),
    );
    await openSettings(page);
    await connected(page);
    assert.equal(board.sessionRequests.length, requests + 1);
    await until(
      "the notice goes once the session is open",
      async () =>
        (await page.locator('[data-testid="transport-notice"]').count()) === 0,
    );
    assert.deepEqual(errors, []);
    await context.close();
  }

  // The board's clock and the browser's are too far apart.
  {
    board.clock = true;
    const redeemed = board.redeemed.length;
    const { context, page, errors } = await open({ choice: "quic" });
    const box = await notice(page, "clock", { retry: false });
    assert.match(
      await box.innerText(),
      /clock and this browser's are too far apart/,
    );
    assert.equal(board.redeemed.length, redeemed);
    board.clock = false;
    assert.deepEqual(errors, []);
    await context.close();
  }

  // The firewall mode keeps the port closed: only Standard is offered, with
  // the reason; a browser that had chosen QUIC is told the same.
  {
    board.blocked = true;
    {
      const { context, page, errors } = await open();
      await openSettings(page);
      assert.equal(await quicIsOffered(page), false);
      assert.equal(
        await hindrance(page).innerText(),
        "The Firewall mode keeps the QUIC port closed.",
      );
      assert.equal(await status(page).getAttribute("data-state"), "off");
      assert.deepEqual(errors, []);
      await context.close();
    }
    const redeemed = board.redeemed.length;
    const { context, page, errors } = await open({ choice: "quic" });
    await notice(page, "blocked", { retry: false });
    assert.equal(board.redeemed.length, redeemed);
    board.blocked = false;
    assert.deepEqual(errors, []);
    await context.close();
  }

  // A browser without WebTransport: only Standard, with the reason, and no
  // session is asked for.
  {
    const requests = board.sessionRequests.length;
    const withoutWebTransport = (context) =>
      context.addInitScript(() => {
        delete window.WebTransport;
      });
    {
      const { context, page, errors } = await open({
        prepare: withoutWebTransport,
      });
      await openSettings(page);
      assert.equal(await quicIsOffered(page), false);
      assert.equal(
        await hindrance(page).innerText(),
        "This browser has no WebTransport.",
      );
      assert.deepEqual(errors, []);
      await context.close();
    }
    const { context, page, errors } = await open({
      choice: "quic",
      prepare: withoutWebTransport,
    });
    const box = await notice(page, "unsupported", { retry: false });
    await box.locator(".ant-notification-notice-close").click();
    await openSettings(page);
    // Nothing a retry could change.
    assert.equal(
      await page.locator('[data-testid="transport-retry"]').count(),
      0,
    );
    assert.equal(board.sessionRequests.length, requests);
    assert.deepEqual(errors, []);
    await context.close();
  }

  // A page frozen for 8 s, as a phone's browser freezes the page of an app
  // that is not in front (here its thread is held in the debugger; the
  // lifecycle's own freeze leaves a visible headless page's timers running):
  // the time is not counted against the session, which goes on, and no new
  // one is opened.
  {
    const { context, page, errors } = await open({ choice: "quic" });
    await openSettings(page);
    await connected(page);
    const redeemed = board.redeemed.length;
    const debug = await context.newCDPSession(page);
    await debug.send("Debugger.enable");
    const held = new Promise((resolve) =>
      debug.once("Debugger.paused", resolve),
    );
    await debug.send("Debugger.pause");
    await held;
    await sleep(8000);
    await debug.send("Debugger.resume");
    await debug.send("Debugger.disable");
    await sleep(3000);
    assert.equal(await status(page).getAttribute("data-state"), "connected");
    assert.equal(board.redeemed.length, redeemed, "a new session was opened");
    assert.deepEqual(errors, []);
    await context.close();
  }

  // The session is lost and comes back by itself; lost for good, the page
  // says so after three attempts and offers another.
  {
    const { context, page, errors } = await open({ choice: "quic" });
    await openSettings(page);
    await connected(page);
    await board.stopGateway();
    await page.waitForFunction(
      () =>
        document
          .querySelector('[data-testid="transport-status"]')
          .getAttribute("data-state") === "connecting",
    );
    board.startGateway();
    await connected(page);
    assert.equal(await page.locator(".ant-notification-notice").count(), 0);

    const requests = board.sessionRequests.length;
    await board.stopGateway();
    const box = await notice(page, "lost", { retry: true });
    // Three attempts, 1, 2 and 4 seconds after the one before.
    assert.equal(board.sessionRequests.length, requests + 3);
    const [first, second, third] = board.sessionRequestTimes.slice(requests);
    assert.ok(
      second - first >= 1900 && second - first < 3000,
      `the second attempt came after ${second - first} ms`,
    );
    assert.ok(
      third - second >= 3900 && third - second < 5000,
      `the third attempt came after ${third - second} ms`,
    );
    assert.equal(await status(page).getAttribute("data-state"), "failed");
    assert.equal(
      await status(page).innerText(),
      "The QUIC session was lost and did not come back. The standard transport is used.",
    );
    // The gateway is switched on and not there: the section says so.
    await until(
      "the section says the gateway is not running",
      async () =>
        (await hindrance(page).count()) === 1 &&
        (await hindrance(page).innerText()) ===
          "The QUIC gateway is not running.",
    );

    board.startGateway();
    await board.gatewayOnTheLink();
    await box.locator('[data-testid="transport-notice-retry"]').click();
    await connected(page);
    await until(
      "the notice goes with the failure",
      async () =>
        (await page.locator('[data-testid="transport-notice"]').count()) === 0,
    );
    assert.deepEqual(errors, []);
    await context.close();
  }

  // Nothing answers on the port: within five seconds the page gives up
  // (Chromium does so by itself after about four), says so and offers
  // another attempt, which works once the gateway answers.
  {
    const certificate = board.hello.certificateSha256;
    await board.stopGateway();
    const silent = dgram.createSocket("udp4");
    await new Promise((resolve) => silent.bind(PORT, "127.0.0.1", resolve));
    // The server still believes the gateway is there.
    board.hello = { certificateSha256: certificate, port: PORT };
    const { context, page, errors } = await open({ choice: "quic" });
    const startedAt = now();
    const box = await notice(page, "timeout", { retry: true });
    const waited = now() - startedAt;
    assert.ok(waited >= 3500 && waited < 7000, `gave up after ${waited} ms`);
    assert.match(await box.innerText(), /did not answer over QUIC in time/);
    await new Promise((resolve) => silent.close(resolve));
    board.hello = null;
    board.startGateway();
    await board.gatewayOnTheLink();
    await box.locator('[data-testid="transport-notice-retry"]').click();
    await box
      .locator(".ant-notification-notice-close")
      .click()
      .catch(() => {});
    await openSettings(page);
    await connected(page);
    assert.deepEqual(errors, []);
    await context.close();
  }

  // Switching off: the gateway stops, the port answers nothing and this
  // browser returns to Standard.
  {
    const { context, page, errors } = await open({ choice: "quic" });
    await openSettings(page);
    await connected(page);
    const certificate = board.hello.certificateSha256;
    assert.equal(await portAnswers(page, certificate), true);
    await page.locator('[data-testid="transport-device-switch"]').click();
    await until("the switch shows off", async () => !(await switchIsOn(page)));
    assert.equal(board.enabled, false);
    assert.equal(board.child, null, "the gateway still runs");
    assert.equal(board.hello, null);
    assert.equal(await status(page).getAttribute("data-state"), "off");
    assert.equal(await stored(page), "standard");
    assert.equal(await quicIsOffered(page), false);
    assert.equal(
      await hindrance(page).innerText(),
      "QUIC is switched off on this device.",
    );
    assert.equal(await portAnswers(page, certificate), false);
    assert.deepEqual(errors, []);
    await context.close();
  }

  // Switched off with a browser that had chosen QUIC: it is told, and uses
  // Standard.
  {
    const { context, page, errors } = await open({ choice: "quic" });
    const box = await notice(page, "unavailable", { retry: true });
    assert.match(await box.innerText(), /The QUIC gateway is not running\./);
    assert.deepEqual(errors, []);
    await context.close();
  }

  console.log("quic transport contract: ok");
} catch (error) {
  if (board.stderr) console.error(`gateway:\n${board.stderr}`);
  throw error;
} finally {
  await browser.close();
  await board.close();
  app.server.close();
}
