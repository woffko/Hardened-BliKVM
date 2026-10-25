// Keyboard and mouse over the QUIC transport's input stream, through the
// built page, the host's build of the gateway and a played server
// (quic_fixture.mjs). With QUIC chosen and its session open, enabling remote
// control takes the HID lease over the session: the lease's requests reach
// the server's side of the link, in order, none over HTTPS; only the
// confirmation of the target stays on HTTPS. The transport is chosen when
// control is enabled, a lost session turns control off, and the standard
// path serves the next time.
import assert from "node:assert/strict";
import { startApp } from "./app_harness.mjs";
import { controlIsOn, releaseControl, takeControl } from "./take_control.mjs";
import {
  Board,
  connected,
  launch,
  openPage,
  openSettings,
  sleep,
  until,
} from "./quic_fixture.mjs";

const browser = await launch();
const app = await startApp();
const board = new Board(Number(new URL(app.baseUrl).port));
await board.listen();
board.enabled = true;
board.startGateway();
await board.gatewayOnTheLink();
const open = (options) => openPage(browser, app, board, options);

const LEASE = "b".repeat(32);
// What went over HTTPS, and what came over the link.
const overHttps = () =>
  app.requestDetails
    .filter((request) => request.route === "/api/hid/lease" && request.body)
    .map((request) => request.body);
const overQuic = () => board.leases.map((lease) => lease.request);
const actions = (requests) => requests.map((request) => request.action);
// The key codes of the keyboard reports, a run of equal ones counted once
// and a release before the first key left out.
const keys = (requests) => {
  const codes = requests
    .filter((request) => request.report?.mode === "keyboard")
    .map((request) => request.report.data[2])
    .filter((code, index, all) => index === 0 || code !== all[index - 1]);
  return codes[0] === 0 ? codes.slice(1) : codes;
};
const typed = (codes) => codes.flatMap((code) => [code, 0]);

const enableControl = (page) => takeControl(page);
const controlIsOff = (page) =>
  page.waitForFunction(
    () => document.documentElement.dataset.remoteControl === "off",
  );

// A page whose QUIC session is open, with Settings closed again.
async function openConnected() {
  const opened = await open({ choice: "quic" });
  await openSettings(opened.page);
  await connected(opened.page);
  await closeSettings(opened.page);
  return opened;
}
async function closeSettings(page) {
  await page.keyboard.press("Escape");
  await page
    .locator('[data-testid="transport-settings"]')
    .waitFor({ state: "detached" });
}
const inputLine = (page) => page.locator('[data-testid="transport-input"]');
// Ends a scenario: the page leaves, as a closed tab does, and what its
// leaving still sends (a release, the session's end) has arrived before the
// next one starts counting.
async function finish(page, context) {
  await page.goto("about:blank");
  await until("the page's sessions are closed", () =>
    board.sessions.length === 0 ? true : null,
  );
  await context.close();
  await sleep(100);
  board.leases.length = 0;
}

try {
  // The lease over the session: acquire, the reports in the order of the
  // keys, release; nothing of it over HTTPS.
  {
    const { context, page, errors } = await openConnected();
    const focusBefore = app.requests.filter((request) =>
      request.endsWith("/api/hid/focus"),
    ).length;
    await enableControl(page);
    for (const key of ["KeyA", "KeyB", "KeyC"]) await page.keyboard.press(key);
    await until("the reports reach the link", () =>
      keys(overQuic()).length >= 6 ? true : null,
    );
    assert.equal(actions(overQuic())[0], "acquire");
    assert.deepEqual(keys(overQuic()), typed([4, 5, 6]));
    assert.ok(
      overQuic()
        .slice(1)
        .every((request) => request.leaseId === LEASE),
      "a request names another lease than the one granted",
    );
    assert.ok(
      overQuic().every((request) => /^[0-9a-f]{32}$/.test(request.operationId)),
    );
    // One session's requests, all of them.
    assert.deepEqual(
      [...new Set(board.leases.map((lease) => lease.session))],
      [board.sessions[0]],
    );
    assert.deepEqual(overHttps(), []);
    // The operator's confirmation of the target went over HTTPS.
    const focus = app.requests
      .filter((request) => request.endsWith("/api/hid/focus"))
      .slice(focusBefore);
    assert.ok(focus.includes("GET /api/hid/focus"), focus.join());
    assert.ok(focus.includes("POST /api/hid/focus"), focus.join());

    // Settings say what carries the input.
    await openSettings(page);
    await until(
      "the section says input goes over QUIC",
      async () => (await inputLine(page).getAttribute("data-input")) === "quic",
    );
    assert.equal(
      await inputLine(page).innerText(),
      "Keyboard and mouse are using this session.",
    );
    await closeSettings(page);

    await releaseControl(page);
    await until("the lease is released over the link", () =>
      actions(overQuic()).at(-1) === "release" ? true : null,
    );
    assert.deepEqual(overHttps(), []);
    assert.deepEqual(errors, []);
    await finish(page, context);
  }

  // A slow server: a burst of typing arrives whole and in order.
  {
    board.leaseDelayMs = 60;
    const { context, page, errors } = await openConnected();
    await enableControl(page);
    await page.keyboard.type("hello world");
    // h e l l o space w o r l d
    const expected = typed([11, 8, 15, 15, 18, 44, 26, 18, 21, 15, 7]);
    await until(
      "every key report reaches the link",
      () => (keys(overQuic()).length >= expected.length ? true : null),
      30000,
    );
    // Two equal keys in a row are one run each: l, l.
    const codes = overQuic()
      .filter((request) => request.report?.mode === "keyboard")
      .map((request) => request.report.data[2]);
    const pressed = codes.filter((code) => code !== 0);
    assert.deepEqual(pressed, [11, 8, 15, 15, 18, 44, 26, 18, 21, 15, 7]);
    // Every key was released before the next went down.
    for (let index = 0; index < codes.length - 1; index += 1) {
      assert.ok(
        codes[index] === 0 || codes[index + 1] === 0,
        `two keys down at once at report ${index}`,
      );
    }
    assert.deepEqual(overHttps(), []);
    board.leaseDelayMs = 0;
    assert.deepEqual(errors, []);
    await finish(page, context);
  }

  // The server refuses, as it does without a current confirmation of the
  // target (a socket opened again clears it): the page confirms the target
  // again and tries again, control stays on, and the key is not lost once
  // the server takes it.
  {
    board.leaseAnswer = () => ({
      status: 200,
      body: { code: -1, msg: "a current focus confirmation is required" },
    });
    const { context, page, errors } = await openConnected();
    await enableControl(page);
    const focusBefore = app.requests.filter((request) =>
      request.endsWith("POST /api/hid/focus"),
    ).length;
    await page.keyboard.press("KeyA");
    await until("the acquire reaches the link", () =>
      overQuic().length >= 1 ? true : null,
    );
    await sleep(300);
    assert.deepEqual(overHttps(), []);
    assert.equal(
      await controlIsOn(page),
      true,
      "the refusal turned control off",
    );
    board.leaseAnswer = null;
    await until("the key reaches the link once the server takes it", () =>
      keys(overQuic()).includes(4) ? true : null,
    );
    assert.ok(
      actions(overQuic()).filter((action) => action === "acquire").length >= 2,
      "the acquire was not tried again",
    );
    assert.ok(
      app.requests.filter((request) => request.endsWith("POST /api/hid/focus"))
        .length > focusBefore,
      "the target was not confirmed again",
    );
    assert.equal(await controlIsOn(page), true);

    // A lease that is not granted turns control off with a notice, and the
    // next press asks again.
    await releaseControl(page);
    board.leases.length = 0;
    board.leaseAnswer = () => ({
      status: 200,
      body: {
        code: 0,
        msg: "success",
        data: { applied: false, rejection: "lease_conflict" },
      },
    });
    await enableControl(page);
    await page.keyboard.press("KeyA");
    await until("the acquire reaches the link", () =>
      overQuic().length >= 1 ? true : null,
    );
    await controlIsOff(page);
    await page.getByText("Another session controls the target").waitFor();
    assert.deepEqual(actions(overQuic()), ["acquire"]);
    await page.keyboard.press("KeyB");
    await page
      .getByRole("button", { name: "Confirm and control" })
      .waitFor({ state: "visible" });
    assert.deepEqual(actions(overQuic()), ["acquire"]);
    board.leaseAnswer = null;
    assert.deepEqual(errors, []);
    await finish(page, context);
  }

  // The transport is chosen when the lease is taken. A lease taken over
  // HTTPS stays there when the session opens, and Settings say so; taken
  // again, it goes over the session.
  {
    const httpsBefore = overHttps().length;
    const { context, page, errors } = await open();
    await enableControl(page);
    await page.keyboard.press("KeyA");
    await until("the report goes over HTTPS", () =>
      keys(overHttps().slice(httpsBefore)).length >= 2 ? true : null,
    );
    await openSettings(page);
    await page
      .locator('[data-testid="transport-choice"] label', { hasText: "QUIC" })
      .click();
    await connected(page);
    assert.equal(await inputLine(page).getAttribute("data-input"), "https");
    assert.match(
      await inputLine(page).innerText(),
      /still use the standard transport/,
    );
    await closeSettings(page);
    await page.keyboard.press("KeyB");
    await until("the next report goes over HTTPS too", () =>
      keys(overHttps().slice(httpsBefore)).length >= 4 ? true : null,
    );
    assert.deepEqual(keys(overHttps().slice(httpsBefore)), typed([4, 5]));
    assert.deepEqual(overQuic(), []);

    await releaseControl(page);
    await until("the lease is released over HTTPS", () =>
      actions(overHttps().slice(httpsBefore)).at(-1) === "release"
        ? true
        : null,
    );
    const httpsAfter = overHttps().length;
    await enableControl(page);
    await page.keyboard.press("KeyC");
    await until("the report goes over the session", () =>
      keys(overQuic()).length >= 2 ? true : null,
    );
    assert.deepEqual(keys(overQuic()), typed([6]));
    assert.equal(overHttps().length, httpsAfter);
    assert.deepEqual(errors, []);
    await finish(page, context);
  }

  // The session is lost while control is on: the key that finds no session
  // takes the lease again over HTTPS and is sent with it, and control stays
  // on, without asking (2026-10-24: control turned off at every reconnect).
  {
    const { context, page, errors } = await openConnected();
    await enableControl(page);
    await page.keyboard.press("KeyA");
    await until("the report reaches the link", () =>
      keys(overQuic()).length >= 2 ? true : null,
    );
    const httpsBefore = overHttps().length;
    await board.stopGateway();
    // The next key finds no session.
    await page.keyboard.press("KeyB");
    await until("the key goes over HTTPS", () =>
      keys(overHttps().slice(httpsBefore)).length >= 1 ? true : null,
    );
    assert.deepEqual(keys(overQuic()), typed([4]));
    assert.equal(actions(overHttps().slice(httpsBefore))[0], "acquire");
    assert.equal(
      await controlIsOn(page),
      true,
      "the lost session took control",
    );
    assert.equal(
      await page
        .getByRole("button", { name: "Confirm and control" })
        .isVisible(),
      false,
      "the lost session asked for control again",
    );
    await page.keyboard.press("KeyC");
    await until("the next report goes over HTTPS", () =>
      keys(overHttps().slice(httpsBefore)).length >= 4 ? true : null,
    );
    assert.deepEqual(keys(overHttps().slice(httpsBefore)), typed([5, 6]));
    board.startGateway();
    await board.gatewayOnTheLink();
    assert.deepEqual(errors, []);
    await finish(page, context);
  }

  // The web session has ended: the input stream says so, and the page asks
  // the server, which sends it to the login.
  {
    const { context, page, errors } = await openConnected();
    board.leaseAnswer = () => ({
      status: 401,
      body: { code: -401, msg: "unauthorized" },
    });
    board.loggedOut = true;
    await enableControl(page);
    await page.keyboard.press("KeyA");
    await until("the page asks for a login", () =>
      /\/auth\/login/.test(page.url()),
    );
    assert.deepEqual(actions(overQuic()), ["acquire"]);
    board.leaseAnswer = null;
    board.loggedOut = false;
    assert.deepEqual(errors, []);
    await finish(page, context);
  }

  console.log("quic input contract: ok");
} catch (error) {
  if (board.stderr) console.error(`gateway:\n${board.stderr}`);
  throw error;
} finally {
  await browser.close();
  await board.close();
  app.server.close();
}
