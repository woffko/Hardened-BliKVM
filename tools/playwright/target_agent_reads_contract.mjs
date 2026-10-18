// The target agent's reads on the page (web/src/hooks/useTargetAgent.ts, docs/design/target-agent-v1.md), through the
// built page with a played board. The page asks the agent for nothing until remote control holds the keyboard's and
// mouse's lease; then it reads the monitors and the keyboard layouts. With the board's monitor on the right of two,
// and no layout kept on the board, the absolute mouse lands on the right monitor by itself: the picture's right edge on
// its last columns, its middle on its middle. Settings show the target's Windows and its monitors, the board's marked;
// the Keyboard menu shows the target's layout, which a read of the layouts begun before a switch does not put back.
// The actions go to the agent as the operator makes them: a layout
// switched, the clipboard read and written, the session locked after a confirmation, and a monitors' change kept in
// the dialog that counts down to its undoing, which a refused Keep leaves open with the reason, counting on, until a
// Keep is done. The operator's word on which monitor is the board's goes to the agent,
// and the mouse follows it. The toolbar's Monitors menu says the mode the monitors are in, changes it as Windows
// does, and the mouse follows the new mode.
import assert from "node:assert/strict";
import { defaultManifest, startApp } from "./app_harness.mjs";
import { launchChromium } from "./quality_fixture.mjs";

const browser = await launchChromium();
const feature = (availability) => ({
  availability,
  evidence: "source_verified",
});
const manifest = defaultManifest({
  board: {
    features: {
      device_settings: feature("read_only"),
      hid_screen_area: feature("read_write"),
      target_agent: feature("read_write"),
    },
  },
});
const app = await startApp({ manifest });
const MAX = 0x7fff;
const pixelOf = (value, total) => Math.floor((value * total) / (MAX + 1));

/** An MJPEG stream of one grey 2560x1440 frame. */
const STREAM = await (async () => {
  const page = await browser.newPage();
  const base64 = await page.evaluate(async () => {
    const canvas = new OffscreenCanvas(2560, 1440);
    const context = canvas.getContext("2d");
    context.fillStyle = "#606060";
    context.fillRect(0, 0, 2560, 1440);
    const blob = await canvas.convertToBlob({
      type: "image/jpeg",
      quality: 0.7,
    });
    let text = "";
    for (const byte of new Uint8Array(await blob.arrayBuffer()))
      text += String.fromCharCode(byte);
    return btoa(text);
  });
  await page.close();
  const jpeg = Buffer.from(base64, "base64");
  return Buffer.concat([
    Buffer.from(
      `--frame\r\nContent-Type: image/jpeg\r\nContent-Length: ${jpeg.length}\r\n\r\n`,
    ),
    jpeg,
    Buffer.from("\r\n"),
  ]);
})();

const SYSTEM = {
  product: "Windows 11 Pro",
  release: "24H2",
  build: 26100,
  revision: 2033,
  architecture: "x86_64",
  locked: false,
};
const LEFT = "00000000000000a1";
const RIGHT = "00000000000000b2";
// The monitor the agent marks as the board's: the right one, until the
// operator chooses; and the monitors' mode.
let boardId = RIGHT;
let displays = "extend";
// The agent's monitors refused once, as the board refuses an answer out of
// its bounds.
let failMonitors = false;
let refuseKeeps = 0;
// A clipboard read the agent refuses once, its session locked (the security
// review of 2026-10-18, HIGH-08).
let lockedReads = 0;
// The next monitors' change is answered this late, its round trip with it.
let slowChangeMs = 0;
// How long the played agent waits before it puts a change back: long enough
// for the dialog's steps under the host's load.
const REVERT_SECONDS = 30;
const monitor = (x) => ({
  id: x === 0 ? LEFT : RIGHT,
  name: x === 0 ? "DELL U2719D" : "USB Capture",
  maker: x === 0 ? "DEL" : "MSI",
  product: x === 0 ? 0xd0a5 : 0x2131,
  x,
  y: 0,
  width: 2560,
  height: 1440,
  refresh_millihertz: 59951,
  primary: x === 0,
  scale: 100,
  board: (x === 0 ? LEFT : RIGHT) === boardId,
});
const ENGLISH = {
  id: "04090409",
  language: "en-US",
  name: "English (United States) - US",
};
const LAYOUT = {
  id: "04190419",
  language: "ru-RU",
  name: "Russian (Russia) - Russian",
};

const calls = [];
// The target's layout in front, as the played agent switches it; the next
// read of the layouts may be answered late with what it was when asked.
let current = LAYOUT;
let staleKeyboardMs = 0;
let staleAsked = false;
let staleAnswered = false;
const absoluteReports = () =>
  app.requestDetails
    .filter(
      (request) =>
        request.route === "/api/hid/lease" &&
        request.body?.action === "report" &&
        request.body.report?.mode === "absolute_mouse",
    )
    .map((request) => request.body.report.data);

async function eventually(condition, timeoutMs = 6000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await condition()) return true;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  return condition();
}

try {
  const context = await browser.newContext({
    viewport: { width: 1280, height: 800 },
  });
  await context.addCookies([
    { name: "hardened-blikvm-csrf", value: "fixture-csrf", url: app.baseUrl },
  ]);
  await context.addInitScript(() =>
    localStorage.setItem("hardened-blikvm-menu-auto-hide", "never"),
  );
  const page = await context.newPage();
  page.setDefaultTimeout(8000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.route("**/api/hid/screen-area", (route) =>
    route.fulfill({ json: { code: 0, data: { area: null } } }),
  );
  await page.route("**/api/hid/mode", (route) =>
    route.fulfill({ json: { code: 0, data: { mode: "normal" } } }),
  );
  await page.route("**/api/target-agent", (route) =>
    route.fulfill({
      json: {
        code: 0,
        data: {
          available: true,
          enabled: true,
          open: true,
          problem: null,
          agent: {
            program: "hardened-blikvm-agent",
            version: "0.1.0",
            commit: "3073fd2d1a2b3c4d",
            system: SYSTEM,
          },
          refused: null,
          messagesIn: 4,
          messagesOut: 4,
          dropped: 0,
        },
      },
    }),
  );
  await page.route("**/api/target-agent/call", (route) => {
    const { call } = route.request().postDataJSON();
    calls.push(call);
    if (call === "monitors" && failMonitors)
      return route.fulfill({
        json: {
          code: -409,
          msg: "the agent did not do it: the agent's answer is not the call's or is out of its bounds",
        },
      });
    if (call === "read_clipboard" && lockedReads > 0) {
      lockedReads -= 1;
      return route.fulfill({
        json: {
          code: -409,
          msg: "the agent did not do it: the target's session is locked",
        },
      });
    }
    if (call === "keep_displays" && refuseKeeps > 0) {
      refuseKeeps -= 1;
      return route.fulfill({
        json: {
          code: -409,
          msg: "the agent did not do it: Windows refused the monitors' configuration (87)",
        },
      });
    }
    if (call?.set_layout)
      current = [LAYOUT, ENGLISH].find((one) => one.id === call.set_layout.id);
    if (call === "keyboard" && staleKeyboardMs > 0) {
      const late = staleKeyboardMs;
      const was = current;
      staleKeyboardMs = 0;
      staleAsked = true;
      return new Promise((resolve) => setTimeout(resolve, late)).then(() => {
        staleAnswered = true;
        return route.fulfill({
          json: {
            code: 0,
            data: {
              answer: {
                keyboard: { current: was, installed: [LAYOUT, ENGLISH] },
              },
              roundTripUs: late * 1000,
            },
          },
        });
      });
    }
    if (call?.set_board_monitor) boardId = call.set_board_monitor.id;
    if (call?.set_displays) displays = call.set_displays.mode;
    const both = [monitor(0), monitor(2560)];
    const shown =
      displays === "board_only"
        ? both.filter((one) => one.board)
        : displays === "others_only"
          ? both.filter((one) => !one.board)
          : displays === "duplicate"
            ? both.map((one) => ({ ...one, x: 0 }))
            : both;
    const answer =
      call === "monitors"
        ? { monitors: shown }
        : call === "keyboard"
          ? { keyboard: { current, installed: [LAYOUT, ENGLISH] } }
          : call === "read_clipboard"
            ? { clipboard: "from the target" }
            : call?.set_displays
              ? { displays_changed: { revert_in_seconds: REVERT_SECONDS } }
              : call === "ping"
                ? "pong"
                : "done";
    if (call?.set_displays && slowChangeMs > 0) {
      const late = slowChangeMs;
      slowChangeMs = 0;
      return new Promise((resolve) => setTimeout(resolve, late)).then(() =>
        route.fulfill({
          json: { code: 0, data: { answer, roundTripUs: late * 1000 } },
        }),
      );
    }
    return route.fulfill({
      json: { code: 0, data: { answer, roundTripUs: 3000 } },
    });
  });
  await page.route("**/api/stream/mjpeg**", (route) =>
    route.fulfill({
      status: 200,
      headers: { "content-type": "multipart/x-mixed-replace; boundary=frame" },
      body: STREAM,
    }),
  );
  await page.goto(app.baseUrl, { waitUntil: "networkidle" });
  await page.waitForFunction(
    () => document.querySelector("img#screen")?.naturalWidth > 0,
  );

  // The quality monitor says the agent's state in a row of its own.
  await page.getByTestId("quality-monitor-toggle").click();
  await page.getByTestId("quality-monitor").waitFor();
  const agentRow = async () => {
    for (const worker of page.workers()) {
      const row = await worker
        .evaluate(() => self.qualityMonitorRows?.agent ?? null)
        .catch(() => null);
      if (row) return row;
    }
    return null;
  };
  assert.ok(
    await eventually(async () => (await agentRow()) === "connected, 0.1.0"),
    `the monitor's agent row: ${await agentRow()}`,
  );
  await page.getByTestId("quality-monitor-toggle").click();

  // Without remote control the page asks the agent for nothing, and the
  // Monitors menu says to enable it.
  const monitorsMenu = page.locator("svg.lucide-monitor-cog").first();
  await monitorsMenu.click();
  await page
    .getByTestId("agent-monitors-unread")
    .getByText("Enable remote control to read the target's monitors")
    .waitFor();
  await monitorsMenu.click();
  await page.waitForTimeout(1500);
  assert.deepEqual(calls, []);

  await page.getByRole("button", { name: "Enable remote control" }).click();
  const confirm = page.getByRole("button", { name: "Confirm and control" });
  await confirm.click();
  // A click while the confirmation still opens can be lost.
  await page.waitForTimeout(300);
  if (await confirm.isVisible()) await confirm.click();
  await confirm.waitFor({ state: "hidden" });

  // Under remote control and before any input, the Monitors menu takes the
  // lease and reads the monitors; a read the board refuses says why, and
  // the next opening reads them.
  failMonitors = true;
  await monitorsMenu.click();
  await page
    .getByTestId("agent-monitors")
    .getByRole("alert")
    .getByText("the agent's answer is not the call's or is out of its bounds", {
      exact: false,
    })
    .waitFor();
  await monitorsMenu.click();
  failMonitors = false;
  await monitorsMenu.click();
  await page
    .getByTestId("agent-monitors-now")
    .getByText("Now: Extend")
    .waitFor();
  await monitorsMenu.click();
  const screen = page.locator("#screen");
  const box = await screen.boundingBox();
  // The first move takes the lease; the reads follow.
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  assert.ok(
    await eventually(
      () => calls.includes("monitors") && calls.includes("keyboard"),
    ),
    `${calls}`,
  );

  // The board's monitor is the right of two: the picture maps onto it.
  const pointAt = async (shareX, shareY) => {
    const area = await screen.boundingBox();
    const before = absoluteReports().length;
    // Three pixels in from the right edge: the side panel's divider lies
    // over the screen's last two.
    await page.mouse.move(
      area.x + Math.min(area.width - 3, shareX * area.width),
      area.y + Math.min(area.height - 3, shareY * area.height),
    );
    assert.ok(await eventually(() => absoluteReports().length > before));
    // A report of the point before may still arrive: the last one is this
    // point's once no more come for a while.
    for (let seen = -1; seen !== absoluteReports().length; ) {
      seen = absoluteReports().length;
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
    const data = absoluteReports().at(-1);
    return pixelOf(data[1] | (data[2] << 8), 5120);
  };
  await pointAt(0.25, 0.25);
  const middle = await pointAt(0.5, 0.5);
  assert.ok(Math.abs(middle - 3840) <= 2, `middle on ${middle}`);
  const right = await pointAt(1, 0.5);
  assert.ok(right >= 5110 && right <= 5119, `right edge on ${right}`);
  const left = await pointAt(0, 0.5);
  assert.equal(left, 2560, `left edge on ${left}`);

  // The Keyboard menu shows the target's layout.
  await page.locator("svg.lucide-keyboard").first().click();
  await page
    .getByTestId("target-layout")
    .getByText("Target's layout: Russian (Russia) - Russian")
    .waitFor();
  // The actions: a layout switched, the clipboard both ways, the session
  // locked after a confirmation.
  const called = (call) =>
    eventually(() =>
      calls.some((made) => JSON.stringify(made) === JSON.stringify(call)),
    );
  const actions = page.getByTestId("agent-keyboard");
  // The layout switched while the page's next read of the layouts is on its
  // way: that read's answer, from before the switch, does not put Russian
  // back.
  staleKeyboardMs = 3000;
  assert.ok(await eventually(() => staleAsked, 20000), "the layouts' read");
  await actions.getByRole("button", { name: "en-US", exact: true }).click();
  assert.ok(
    await called({ set_layout: { id: "04090409" } }),
    JSON.stringify(calls),
  );
  assert.ok(await eventually(() => staleAnswered, 6000));
  await page.waitForTimeout(300);
  await page
    .getByTestId("target-layout")
    .getByText("Target's layout: English (United States) - US")
    .waitFor();
  await actions.getByRole("button", { name: "Target's clipboard" }).click();
  const clipboard = page.getByRole("dialog", { name: "Target's clipboard" });
  // While the target's session is locked the agent says so, and the read
  // made after it is unlocked gets the text.
  lockedReads = 1;
  await clipboard.getByRole("button", { name: "Read the target's" }).click();
  await clipboard.getByText("the target's session is locked").waitFor();
  await clipboard.getByRole("button", { name: "Read the target's" }).click();
  const text = clipboard.getByTestId("agent-clipboard-text");
  await eventually(async () => (await text.inputValue()) === "from the target");
  assert.equal(await text.inputValue(), "from the target");
  await text.fill("to the target, ünïcode");
  await clipboard.getByRole("button", { name: "Put on the target" }).click();
  assert.ok(
    await called({ write_clipboard: { text: "to the target, ünïcode" } }),
  );
  await clipboard.getByText("The text is on the target's clipboard").waitFor();
  await clipboard.getByRole("button", { name: "Close" }).click();
  await clipboard.waitFor({ state: "hidden" });
  // The menu may have closed with the dialog: it is opened again if so.
  const lockButton = actions.getByRole("button", { name: "Lock the target" });
  await page.waitForTimeout(300);
  if (!(await lockButton.isVisible()))
    await page.locator("svg.lucide-keyboard").first().click();
  await lockButton.click();
  const lockDialog = page
    .getByRole("dialog")
    .filter({ hasText: "Lock the target's Windows session?" });
  assert.equal(calls.includes("lock"), false);
  await lockDialog.getByRole("button", { name: "Lock the target" }).click();
  assert.ok(await called("lock"));
  await page.keyboard.press("Escape");

  // Settings show the target's Windows and its monitors, the board's marked.
  await page.locator("svg.lucide-settings").click();
  await page.locator('[data-settings-tab="device"]').click();
  await page
    .getByTestId("target-agent-system")
    .getByText("Windows 11 Pro 24H2 (build 26100.2033)")
    .waitFor();
  const monitors = page.getByTestId("target-agent-monitors");
  await monitors
    .getByText("DELL U2719D: 2560x1440 at 0,0", { exact: true })
    .waitFor();
  await monitors
    .getByText("USB Capture: 2560x1440 at 2560,0 — the board's", {
      exact: true,
    })
    .waitFor();
  // A monitors' change waits to be kept, and is kept. Answered after 4 s,
  // it still opens the dialog, which says how long it took.
  slowChangeMs = 4000;
  await page
    .getByTestId("target-agent-displays")
    .getByRole("button", { name: "Only the board's monitor" })
    .click();
  assert.ok(await called({ set_displays: { mode: "board_only" } }));
  // The played agent shows both monitors again for what follows.
  displays = "extend";
  const keep = page.getByRole("dialog", { name: "Keep these monitors?" });
  await keep.getByText(/in (29|30) s unless you keep them/).waitFor({
    timeout: 12000,
  });
  await keep
    .getByTestId("agent-displays-took")
    .getByText("The change took 4.0 s.")
    .waitFor();
  // The first Keep is refused: the dialog stays with the reason and counts
  // on; Keep again keeps the change and closes it.
  refuseKeeps = 1;
  await keep.getByRole("button", { name: "Keep", exact: true }).click();
  assert.ok(await called("keep_displays"));
  await keep
    .getByTestId("agent-displays-keep-failure")
    .getByText("Windows refused the monitors' configuration (87)", {
      exact: false,
    })
    .waitFor();
  await keep
    .getByText(/in (2[0-7]|1\d|\d) s unless you keep them/)
    .waitFor({ timeout: 6000 });
  await keep.getByRole("button", { name: "Keep", exact: true }).click();
  assert.ok(
    await eventually(
      () => calls.filter((made) => made === "keep_displays").length >= 2,
    ),
  );
  await keep.waitFor({ state: "hidden" });

  // The operator says the left monitor is the board's: the agent is told,
  // the list marks it, and the picture maps onto it.
  assert.equal(
    await monitors.getByRole("button", { name: "This is the board's" }).count(),
    1,
  );
  await monitors.getByRole("button", { name: "This is the board's" }).click();
  assert.ok(await called({ set_board_monitor: { id: LEFT } }));
  await monitors
    .getByText("DELL U2719D: 2560x1440 at 0,0 — the board's", { exact: true })
    .waitFor();
  await page.locator(".ant-modal-close:visible").click();
  await page.locator(".ant-modal-wrap:visible").waitFor({ state: "detached" });
  const chosenMiddle = await pointAt(0.5, 0.5);
  assert.ok(Math.abs(chosenMiddle - 1280) <= 2, `middle on ${chosenMiddle}`);
  const chosenRight = await pointAt(1, 0.5);
  assert.ok(
    chosenRight >= 2550 && chosenRight <= 2559,
    `right edge on ${chosenRight}`,
  );

  // The toolbar's Monitors menu: the mode now, and Windows' choices.
  await page.locator("svg.lucide-monitor-cog").first().click();
  const menu = page.getByTestId("agent-monitors");
  await menu
    .getByTestId("agent-monitors-now")
    .getByText("Now: Extend")
    .waitFor();
  await menu.getByRole("button", { name: "Only the board's monitor" }).click();
  assert.ok(await called({ set_displays: { mode: "board_only" } }));
  const keepAgain = page.getByRole("dialog", { name: "Keep these monitors?" });
  await keepAgain.getByRole("button", { name: "Keep", exact: true }).click();
  assert.ok(
    await eventually(
      () => calls.filter((made) => made === "keep_displays").length >= 3,
    ),
  );
  // The Keep, a click beside the menu, closed it; opened again, it shows
  // the monitors read again: only the board's, the whole picture on it.
  await menu.waitFor({ state: "hidden" });
  await page.locator("svg.lucide-monitor-cog").first().click();
  await menu
    .getByTestId("agent-monitors-now")
    .getByText("Now: Only the board's monitor")
    .waitFor({ timeout: 15000 });
  await page.locator("svg.lucide-monitor-cog").first().click();
  await menu.waitFor({ state: "hidden" });
  // The same point of the picture is now the middle of the whole desktop,
  // which is the board's monitor alone.
  await pointAt(0.25, 0.25);
  const onlyMiddle = (await pointAt(0.5, 0.5)) / 2;
  assert.ok(Math.abs(onlyMiddle - 1280) <= 2, `middle on ${onlyMiddle}`);
  assert.deepEqual(errors, []);
  await context.close();
  console.log(
    `target agent reads contract: ok (the picture's middle on column ${middle} of 5120, right edge ${right}, left edge ${left}; with the left monitor chosen as the board's, middle ${chosenMiddle}, right edge ${chosenRight}; only the board's monitor from the Monitors menu, kept, middle ${onlyMiddle} of 2560)`,
  );
} finally {
  await browser.close();
  await new Promise((resolve) => app.server.close(resolve));
}
