// The video menu's H.264 entries by engine
// (docs/design/h264-engine-choice-and-quality-monitor-v1.md), against the
// stand-in for the board's side (quality_fixture.mjs): where the board
// offers its choice, H.264 HW and H.264 SW with the transport in brackets;
// the check mark on the board's choice while this browser's mode is H.264;
// the choice posted only where it is another, with a reload only where the
// mode changes; the chosen entry that says what encodes where that is not
// what was chosen; the video engine's entry disabled with the encoder's
// reason where it cannot be had; entries that only choose this browser's
// mode without the right to change the engine; Russian; and a board without
// the choice that keeps its one entry.
import assert from "node:assert/strict";
import { defaultManifest, startApp } from "./app_harness.mjs";
import {
  boardState,
  engineChoiceVideo,
  launchChromium,
  routeEngineAndQuality,
} from "./quality_fixture.mjs";

const browser = await launchChromium();
const app = await startApp();
const MODE_KEY = "hardened-blikvm-video-mode";
const LANGUAGE_KEY = "hardened-blikvm-language";

async function open({
  mode,
  board,
  video = engineChoiceVideo(),
  language,
  refuse,
}) {
  const manifest = defaultManifest();
  manifest.video = { ...manifest.video, ...video };
  const context = await browser.newContext();
  await context.addCookies([
    { name: "hardened-blikvm-csrf", value: "fixture-csrf", url: app.baseUrl },
  ]);
  // The mode and the language once: a reload keeps what the page stored.
  await context.addInitScript(
    ([modeKey, modeValue, languageKey, languageValue]) => {
      if (sessionStorage.getItem("fixture-set")) return;
      sessionStorage.setItem("fixture-set", "1");
      localStorage.setItem(modeKey, modeValue);
      if (languageValue) localStorage.setItem(languageKey, languageValue);
    },
    [MODE_KEY, mode, LANGUAGE_KEY, language ?? null],
  );
  const page = await context.newPage();
  page.setDefaultTimeout(8000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.route("**/api/capabilities", (route) =>
    route.fulfill({ json: { code: 0, data: manifest } }),
  );
  await page.route("**/api/stream/mjpeg/detect", (route) =>
    route.fulfill({ json: { code: 0, data: { enabled: true } } }),
  );
  await page.route("**/api/system/firewall**", (route) =>
    route.fulfill({
      json: {
        code: 0,
        data: { effectiveMode: "baseline", webrtcBlocked: false },
      },
    }),
  );
  const asked = await routeEngineAndQuality(page, board, { refuse });
  await page.goto(app.baseUrl, { waitUntil: "networkidle" });
  return { context, page, errors, asked };
}

async function openMenu(page, label = "Video Mode") {
  await page.locator("svg.lucide-monitor").first().click();
  await page.getByText(label, { exact: true }).hover();
  await page.locator('[data-video-mode="mjpeg"]').waitFor();
}

const entry = (page, engine) =>
  page.locator(`[data-video-mode="h264"][data-h264-engine="${engine}"]`);

/** What the two H.264 entries show: name, checked, disabled, the note. */
async function entries(page) {
  const shown = {};
  for (const engine of ["hardware", "software"]) {
    const item = entry(page, engine);
    shown[engine] = {
      name: await item
        .locator(`[data-video-mode-name="h264-${engine}"]`)
        .innerText(),
      checked: await item.getAttribute("data-checked"),
      disabled: await item.getAttribute("aria-disabled"),
      note:
        (await item.locator("[data-h264-encodes]").count()) > 0
          ? await item.locator("[data-h264-encodes]").innerText()
          : null,
    };
  }
  return shown;
}

const changes = (asked) => asked.filter((line) => line.startsWith("POST"));

async function tooltip(page, engine, text) {
  await entry(page, engine).hover();
  await page.getByText(text).waitFor();
}

try {
  // Both entries, the check mark on the board's choice; SW chosen in place.
  {
    const board = boardState();
    const { context, page, errors, asked } = await open({
      mode: "h264",
      board,
    });
    await openMenu(page);
    assert.deepEqual(await entries(page), {
      hardware: {
        name: "H.264 HW (WebRTC)",
        checked: "true",
        disabled: "false",
        note: null,
      },
      software: {
        name: "H.264 SW (WebRTC)",
        checked: "false",
        disabled: "false",
        note: null,
      },
    });
    assert.equal(
      await page
        .locator('[data-video-mode="h264"]:not([data-h264-engine])')
        .count(),
      0,
      "the one H.264 entry is gone",
    );
    await tooltip(page, "software", "The engine is the board's");
    // Choosing the board's choice again changes nothing.
    await entry(page, "hardware").click();
    await page.waitForTimeout(300);
    assert.deepEqual(changes(asked), []);
    await entry(page, "software").click();
    await page.waitForFunction(() =>
      document.querySelector(
        '[data-h264-engine="software"][data-checked="true"]',
      ),
    );
    assert.deepEqual(changes(asked), ["POST /api/vm/screen h264_engine=0"]);
    assert.equal(board.engine, "software");
    // No reload: the stream goes on with the other engine, and what encodes
    // is read again.
    const before = asked.filter((line) => line === "GET /api/vm/screen").length;
    await page.waitForTimeout(1400);
    assert.ok(
      asked.filter((line) => line === "GET /api/vm/screen").length > before,
      "what encodes was not read again",
    );
    assert.equal(
      await entry(page, "software").getAttribute("data-checked"),
      "true",
    );
    assert.equal(
      await page.evaluate((key) => localStorage.getItem(key), MODE_KEY),
      "h264",
    );
    assert.deepEqual(errors, []);
    await context.close();
  }

  // From another mode: the choice is posted, the mode becomes H.264, the page
  // reloads and checks the chosen entry.
  {
    const board = boardState();
    const { context, page, errors, asked } = await open({
      mode: "mjpeg",
      board,
    });
    await openMenu(page);
    assert.equal((await entries(page)).hardware.checked, "false");
    await Promise.all([
      page.waitForEvent("load", { timeout: 8000 }),
      entry(page, "software").click(),
    ]);
    assert.deepEqual(changes(asked), ["POST /api/vm/screen h264_engine=0"]);
    assert.equal(
      await page.evaluate((key) => localStorage.getItem(key), MODE_KEY),
      "h264",
    );
    await page.waitForLoadState("networkidle");
    await openMenu(page);
    const shown = await entries(page);
    assert.equal(shown.software.checked, "true");
    assert.equal(shown.hardware.checked, "false");
    assert.deepEqual(errors, []);
    await context.close();
  }

  // The engine is chosen, x264 encodes: the chosen entry says so.
  {
    const board = boardState({ encoder: "x264" });
    const { context, page, errors } = await open({ mode: "h264", board });
    await openMenu(page);
    const shown = await entries(page);
    assert.equal(shown.hardware.checked, "true");
    assert.equal(shown.hardware.note, "x264 encodes");
    assert.equal(shown.software.note, null);
    assert.deepEqual(errors, []);
    await context.close();
  }

  // The engine was asked for and could not be had (busy): the board's choice
  // is still hardware and x264 encodes. H.264 HW stays choosable and says why,
  // and choosing it again posts the choice again, which is the retry that the
  // encoder waits for; once the engine is free it takes the stream over.
  {
    const board = boardState({
      engine: "hardware",
      encoder: "x264",
      hardwareAvailable: false,
      hardwareReason: "/dev/hardened-blikvm-ve-enc: Device or resource busy",
    });
    const { context, page, errors, asked } = await open({
      mode: "h264",
      board,
    });
    await openMenu(page);
    const shown = await entries(page);
    assert.equal(shown.hardware.disabled, "false");
    assert.equal(shown.hardware.checked, "true");
    assert.equal(shown.hardware.note, "x264 encodes");
    assert.equal(shown.software.disabled, "false");
    await tooltip(page, "hardware", "Device or resource busy");
    await page.getByText("asks the encoder to try again").waitFor();
    // The engine is free again; the choice made again brings it back.
    board.hardwareAvailable = true;
    board.hardwareReason = null;
    await entry(page, "hardware").click();
    await page.waitForFunction(
      () => !document.querySelector("[data-h264-encodes]"),
    );
    assert.deepEqual(changes(asked), ["POST /api/vm/screen h264_engine=1"]);
    assert.equal(board.encoder, "hardware");
    assert.deepEqual(errors, []);
    await context.close();
  }

  // x264 was chosen while the engine is busy: H.264 HW is choosable too, and
  // choosing it posts the choice.
  {
    const board = boardState({
      engine: "software",
      encoder: "x264",
      hardwareAvailable: false,
      hardwareReason: "/dev/hardened-blikvm-ve-enc: Device or resource busy",
    });
    const { context, page, errors, asked } = await open({
      mode: "h264",
      board,
    });
    await openMenu(page);
    const shown = await entries(page);
    assert.equal(shown.hardware.disabled, "false");
    assert.equal(shown.software.disabled, "false");
    assert.equal(shown.software.checked, "true");
    await tooltip(page, "hardware", "Device or resource busy");
    await entry(page, "hardware").click();
    await page.waitForFunction(() =>
      document.querySelector(
        '[data-h264-engine="hardware"][data-checked="true"]',
      ),
    );
    assert.deepEqual(changes(asked), ["POST /api/vm/screen h264_engine=1"]);
    assert.deepEqual(errors, []);
    await context.close();
  }

  // The encoder has no engine to offer at all (its unit names no device, so
  // it gives no reason): H.264 HW is disabled, and choosing it posts nothing.
  {
    const board = boardState({
      engine: "software",
      encoder: "x264",
      hardwareAvailable: false,
      hardwareReason: null,
    });
    const { context, page, errors, asked } = await open({
      mode: "h264",
      board,
    });
    await openMenu(page);
    const shown = await entries(page);
    assert.equal(shown.hardware.disabled, "true");
    assert.equal(shown.software.disabled, "false");
    assert.equal(shown.software.checked, "true");
    await tooltip(page, "hardware", "cannot encode H.264 now");
    await entry(page, "hardware").click({ force: true });
    await page.waitForTimeout(400);
    assert.deepEqual(changes(asked), []);
    assert.equal(board.engine, "software");
    assert.deepEqual(errors, []);
    await context.close();
  }

  // Without the right to change the engine: the entries set this browser's
  // mode only, and say who chooses the engine.
  {
    const board = boardState();
    const video = engineChoiceVideo({
      status: { availability: "read_only", evidence: "source_verified" },
    });
    const { context, page, errors, asked } = await open({
      mode: "mjpeg",
      board,
      video,
    });
    await openMenu(page);
    await tooltip(page, "software", "The board's operator chooses");
    await Promise.all([
      page.waitForEvent("load", { timeout: 8000 }),
      entry(page, "software").click(),
    ]);
    assert.deepEqual(changes(asked), []);
    assert.equal(board.engine, "hardware");
    assert.equal(
      await page.evaluate((key) => localStorage.getItem(key), MODE_KEY),
      "h264",
    );
    await page.waitForLoadState("networkidle");
    await openMenu(page);
    // The board's choice is still the engine: its entry is checked.
    const shown = await entries(page);
    assert.equal(shown.hardware.checked, "true");
    assert.equal(shown.software.checked, "false");
    await entry(page, "software").click();
    await page.waitForTimeout(400);
    assert.deepEqual(changes(asked), []);
    assert.deepEqual(errors, []);
    await context.close();
  }

  // A choice the board refuses: the message is the sentence the page owns, in
  // the page's language, and not the server's rejection code; the choice
  // stays the board's.
  for (const [language, sentence] of [
    [undefined, "The board did not change the H.264 engine."],
    ["ru", "Плата не изменила движок H.264."],
  ]) {
    const board = boardState();
    const { context, page, errors, asked } = await open({
      mode: "h264",
      board,
      language,
      refuse: "unsupported_setting",
    });
    await openMenu(page, language === "ru" ? "Видеорежим" : "Video Mode");
    await entry(page, "software").click();
    await page.getByText(sentence).waitFor();
    assert.equal(await page.getByText("unsupported_setting").count(), 0);
    assert.deepEqual(changes(asked), ["POST /api/vm/screen h264_engine=0"]);
    assert.equal(board.engine, "hardware");
    assert.equal(
      await entry(page, "software").getAttribute("data-checked"),
      "false",
    );
    assert.deepEqual(errors, []);
    await context.close();
  }

  // Russian.
  {
    const board = boardState({ encoder: "x264" });
    const { context, page, errors } = await open({
      mode: "h264",
      board,
      language: "ru",
    });
    await openMenu(page, "Видеорежим");
    const shown = await entries(page);
    assert.equal(shown.hardware.name, "H.264 HW (WebRTC)");
    assert.equal(shown.hardware.note, "кодирует x264");
    await tooltip(page, "software", "Движок общий для платы");
    assert.deepEqual(errors, []);
    await context.close();
  }

  // A board without the choice keeps the one entry.
  {
    const board = boardState();
    const video = engineChoiceVideo({
      settings: ["fps", "resolution", "webrtc_bitrate"],
    });
    const { context, page, errors, asked } = await open({
      mode: "h264",
      board,
      video,
    });
    await openMenu(page);
    assert.equal(await page.locator("[data-h264-engine]").count(), 0);
    assert.equal(
      await page.locator('[data-video-mode-name="h264"]').innerText(),
      "H.264 (WebRTC)",
    );
    assert.deepEqual(changes(asked), []);
    assert.deepEqual(errors, []);
    await context.close();
  }

  console.log(
    "H.264 engine menu: HW and SW with the transport, the board's choice checked and changed in place or with the mode, what encodes beside the choice, an engine that failed or is busy shown with its reason and choosable again, an absent engine disabled, a refused choice told in the page's own sentence, no right choosing the mode only, Russian, and the one entry without the choice passed",
  );
} finally {
  await browser.close();
  await new Promise((resolve) => app.server.close(resolve));
}
