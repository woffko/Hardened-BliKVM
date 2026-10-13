// The absolute mouse on a target whose desktop spans more than one monitor (web/src/lib/screen-area.ts,
// docs/design/screen-area-v1.md), through the built page with a played board. The Mouse menu's Desktop entry writes
// the layout the board keeps (`/api/hid/screen-area`): "Left of two" with the captured screen's size, applied, makes
// the desktop twice as wide with this screen on the left. With it, the mouse's reports carry the desktop's position:
// the picture's right edge lands on the left monitor's last columns, its middle on its middle; a reloaded page reads
// the layout back from the board; the whole desktop again sends the picture's own position. On a phone a finger's
// reports stay on the captured screen. The entry speaks Russian.
import assert from "node:assert/strict";
import { defaultManifest, startApp } from "./app_harness.mjs";
import { launchChromium } from "./quality_fixture.mjs";

const browser = await launchChromium();
const app = await startApp();
const MAX = 0x7fff;
/** The target's pixel for a report's value: the range is MAX + 1 steps over its desktop. */
const pixelOf = (value, total) => Math.floor((value * total) / (MAX + 1));

/** An MJPEG stream of one grey 2560x1440 frame: the captured screen at its own size. */
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
    for (const byte of new Uint8Array(await blob.arrayBuffer())) {
      text += String.fromCharCode(byte);
    }
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

/** The layout the played board keeps, and what the page wrote to it. */
const board = { area: null, writes: [] };

async function open({ mobile = false, language = null } = {}) {
  const context = await browser.newContext({
    viewport: mobile
      ? { width: 390, height: 844 }
      : { width: 1280, height: 800 },
    ...(mobile ? { isMobile: true, hasTouch: true, deviceScaleFactor: 2 } : {}),
  });
  await context.addCookies([
    { name: "hardened-blikvm-csrf", value: "fixture-csrf", url: app.baseUrl },
  ]);
  await context.addInitScript((language) => {
    localStorage.setItem("hardened-blikvm-menu-auto-hide", "never");
    if (language) localStorage.setItem("hardened-blikvm-language", language);
  }, language);
  const page = await context.newPage();
  page.setDefaultTimeout(8000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  const sent = [];
  // Without the board's power control the page sends input over its
  // WebSocket at once, with no lease.
  const manifest = defaultManifest();
  await page.route("**/api/capabilities", (route) =>
    route.fulfill({
      json: {
        code: 0,
        data: {
          ...manifest,
          power: {
            ...manifest.power,
            status: { ...manifest.power.status, availability: "read_only" },
          },
          board: {
            ...manifest.board,
            features: {
              ...manifest.board.features,
              hid_screen_area: {
                availability: "read_write",
                evidence: "source_verified",
              },
            },
          },
        },
      },
    }),
  );
  await page.route("**/api/hid/screen-area", async (route) => {
    if (route.request().method() === "POST") {
      const { area } = route.request().postDataJSON();
      board.writes.push(area);
      board.area = area;
    }
    await route.fulfill({ json: { code: 0, data: { area: board.area } } });
  });
  // The Mouse menu's other entries ask what the board offers them.
  await page.route("**/api/hid/mode", (route) =>
    route.fulfill({ json: { code: 0, data: { mode: "normal" } } }),
  );
  await page.route("**/api/stream/mjpeg**", (route) =>
    route.fulfill({
      status: 200,
      headers: { "content-type": "multipart/x-mixed-replace; boundary=frame" },
      body: STREAM,
    }),
  );
  await page.routeWebSocket("**/api/ws", (socket) =>
    socket.onMessage((message) => {
      if (typeof message !== "string") sent.push([...message]);
    }),
  );
  await page.goto(app.baseUrl, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(
    () => document.querySelector("img#screen")?.naturalWidth > 0,
  );
  return { context, page, errors, sent };
}

/** The last absolute report's position, waited for. */
async function lastPosition(page, sent, from) {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    const report = sent
      .slice(from)
      .filter((data) => data[0] === 2)
      .at(-1);
    if (report)
      return [report[2] | (report[3] << 8), report[4] | (report[5] << 8)];
    await page.waitForTimeout(50);
  }
  assert.fail("no mouse report sent");
}

/** The mouse at a share of the picture, and where the report puts it. */
async function pointAt(page, sent, shareX, shareY) {
  const box = await page.locator("#screen").boundingBox();
  const from = sent.length;
  // Three pixels in from the right edge: the side panel's divider lies
  // over the screen's last two.
  await page.mouse.move(
    box.x + Math.min(box.width - 3, shareX * box.width),
    box.y + Math.min(box.height - 3, shareY * box.height),
  );
  return lastPosition(page, sent, from);
}

async function openEntry(page, title) {
  await page.locator("svg.lucide-mouse").first().click();
  await page.getByText(title, { exact: true }).hover();
}

try {
  // Desktop: the entry writes "Left of two" for a 2560x1440 screen.
  let measured;
  {
    const { context, page, errors, sent } = await open();
    const [plainX] = await pointAt(page, sent, 1, 0.5);
    assert.ok(plainX > MAX * 0.99, `the whole desktop: ${plainX}`);
    await openEntry(page, "Desktop");
    await page.locator('[data-desktop-area="width"]').fill("2560");
    await page.locator('[data-desktop-area="height"]').fill("1440");
    await page.locator('[data-desktop-area-beside="left"]').click();
    assert.equal(
      await page.locator('[data-desktop-area="desktopWidth"]').inputValue(),
      "5120",
    );
    await page.getByRole("button", { name: "Apply", exact: true }).click();
    await page
      .getByText("Part of 5120×1440, at 0,0", { exact: true })
      .waitFor();
    assert.deepEqual(board.writes.at(-1), {
      desktop: { width: 5120, height: 1440 },
      screen: { x: 0, y: 0, width: 2560, height: 1440 },
    });
    await page.keyboard.press("Escape");
    const [rightX, middleY] = await pointAt(page, sent, 1, 0.5);
    const right = pixelOf(rightX, 5120);
    assert.ok(
      right >= 2550 && right <= 2559,
      `the picture's right edge on ${right}`,
    );
    assert.ok(Math.abs(pixelOf(middleY, 1440) - 720) <= 1, `${middleY}`);
    const [middleX] = await pointAt(page, sent, 0.5, 0.5);
    assert.ok(Math.abs(pixelOf(middleX, 5120) - 1280) <= 2, `${middleX}`);
    const [leftX] = await pointAt(page, sent, 0, 0);
    assert.equal(pixelOf(leftX, 5120), 0);
    measured = { right, middle: pixelOf(middleX, 5120) };
    assert.deepEqual(errors, []);
    await context.close();
  }

  // A reloaded page reads the layout back from the board; the whole
  // desktop again sends the picture's own position.
  {
    const { context, page, errors, sent } = await open();
    const [rightX] = await pointAt(page, sent, 1, 0.5);
    const right = pixelOf(rightX, 5120);
    assert.ok(
      right >= 2550 && right <= 2559,
      `read back after a reload: ${right}`,
    );
    await openEntry(page, "Desktop");
    await page.locator('[data-desktop-area-entry="whole"]').click();
    for (let attempt = 0; attempt < 40 && board.area !== null; attempt += 1) {
      await page.waitForTimeout(50);
    }
    assert.equal(board.area, null);
    await page.keyboard.press("Escape");
    const [plainX] = await pointAt(page, sent, 1, 0.5);
    assert.ok(plainX > MAX * 0.99, `the whole desktop again: ${plainX}`);
    assert.deepEqual(errors, []);
    await context.close();
  }

  // A phone: a finger's reports stay on the captured screen, the left half
  // of the desktop.
  board.area = {
    desktop: { width: 5120, height: 1440 },
    screen: { x: 0, y: 0, width: 2560, height: 1440 },
  };
  {
    const { context, page, errors, sent } = await open({ mobile: true });
    const touch = await context.newCDPSession(page);
    const view = await page.locator("[data-kvm-screen-scroll]").boundingBox();
    const from = sent.length;
    const y = view.y + view.height / 2;
    // A long way right: the cursor stops at the screen's right edge.
    await touch.send("Input.dispatchTouchEvent", {
      type: "touchStart",
      touchPoints: [{ x: view.x + 10, y }],
    });
    for (let step = 1; step <= 20; step += 1) {
      await touch.send("Input.dispatchTouchEvent", {
        type: "touchMove",
        touchPoints: [{ x: view.x + 10 + step * 30, y }],
      });
    }
    await touch.send("Input.dispatchTouchEvent", {
      type: "touchEnd",
      touchPoints: [],
    });
    const [fingerX] = await lastPosition(page, sent, from);
    const pixel = pixelOf(fingerX, 5120);
    assert.ok(pixel >= 0 && pixel <= 2559, `a finger at ${pixel} of 5120`);
    measured.finger = pixel;
    assert.deepEqual(errors, []);
    await context.close();
  }

  // Russian.
  {
    const { context, page, errors } = await open({ language: "ru" });
    await openEntry(page, "Рабочий стол");
    await page
      .getByText("Часть 5120×1440, в точке 0,0", { exact: true })
      .waitFor();
    await page
      .getByRole("button", { name: "Левый из двух", exact: true })
      .waitFor();
    assert.deepEqual(errors, []);
    await context.close();
  }

  console.log(
    `Screen area: "Left of two" written to the board, the picture's right edge on the left monitor's column ${measured.right} of 5120 and its middle on ${measured.middle}, read back after a reload, the whole desktop again, a finger on column ${measured.finger}, Russian passed`,
  );
} finally {
  await browser.close();
  await new Promise((resolve) => app.server.close(resolve));
}
