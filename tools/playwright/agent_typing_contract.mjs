// Text typed on a phone with the target's layout following its letters (web/src/lib/agent-typing.ts,
// docs/design/target-agent-v1.md), through the built page with a played board and agent. With the option on, the
// phone keyboard's "Привет, world" switches the target to its Russian layout before the Cyrillic run and to its
// English one before the Latin run, each once the keys before it are typed: the Cyrillic letters and the comma at the
// Russian layout's keys, the Latin ones at the US layout's. Latin text while the target is already English switches
// nothing, and with the option off nothing is switched at all. "Tänan, ok?" switches to the Estonian layout, its
// letters and its question mark at that layout's keys, and Latin text after it keeps that layout.
import assert from "node:assert/strict";
import { defaultManifest, startApp } from "./app_harness.mjs";
import { launchChromium } from "./quality_fixture.mjs";

const browser = await launchChromium();
const feature = (availability) => ({
  availability,
  evidence: "source_verified",
});
const app = await startApp({
  manifest: defaultManifest({
    board: {
      features: {
        device_settings: feature("read_only"),
        target_agent: feature("read_write"),
      },
    },
  }),
});
const SHIFT = 0x02;
const ENGLISH = {
  id: "04090409",
  language: "en-US",
  name: "English (United States) - US",
};
const RUSSIAN = {
  id: "04190419",
  language: "ru-RU",
  name: "Russian (Russia) - Russian",
};
const ESTONIAN = {
  id: "04250425",
  language: "et-EE",
  name: "Estonian (Estonia) - Estonian",
};
// A key pressed, then all keys up.
const typed = (modifier, usage) => [
  [modifier, 0, usage, 0, 0, 0, 0, 0],
  [0, 0, 0, 0, 0, 0, 0, 0],
];

async function eventually(condition, timeoutMs = 6000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await condition()) return true;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  return condition();
}

/**
 * A phone's page with remote control and the phone keyboard open, and what it
 * sends in order: the agent's calls and the keyboard's reports.
 */
async function open({ follow, installed = [ENGLISH, RUSSIAN] }) {
  const context = await browser.newContext({
    viewport: { width: 390, height: 844 },
    isMobile: true,
    hasTouch: true,
  });
  await context.addCookies([
    { name: "hardened-blikvm-csrf", value: "fixture-csrf", url: app.baseUrl },
  ]);
  await context.addInitScript(
    (follow) =>
      localStorage.setItem(
        "hardened-blikvm-agent-layout-sync",
        follow ? "yes" : "no",
      ),
    follow,
  );
  const page = await context.newPage();
  page.setDefaultTimeout(8000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  const sent = [];
  let current = ENGLISH;
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
            system: null,
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
    sent.push({ call, at: performance.now() });
    if (call?.set_layout)
      current = installed.find((layout) => layout.id === call.set_layout.id);
    const answer =
      call === "keyboard"
        ? { keyboard: { current, installed } }
        : call === "monitors"
          ? { monitors: [] }
          : "done";
    return route.fulfill({
      json: { code: 0, data: { answer, roundTripUs: 3000 } },
    });
  });
  await page.route("**/api/hid/lease", (route) => {
    const body = route.request().postDataJSON();
    if (body?.action === "report" && body.report?.mode === "keyboard")
      sent.push({ report: body.report.data, at: performance.now() });
    return route.continue();
  });
  await page.goto(app.baseUrl, { waitUntil: "networkidle" });
  await page.getByRole("button", { name: "Enable remote control" }).click();
  await page.getByRole("button", { name: "Confirm and control" }).click();
  await page.locator("svg.lucide-smartphone").first().locator("..").tap();
  await page.getByTestId("key-helper").waitFor();
  return { context, page, errors, sent };
}

const layoutCalls = (sent) =>
  sent.filter((entry) => entry.call?.set_layout).map((entry) => entry.call);
const reports = (entries) =>
  entries.filter((entry) => entry.report).map((entry) => entry.report);

try {
  {
    const { context, page, errors, sent } = await open({ follow: true });
    // The target's layouts are read as the keyboard opens.
    assert.ok(
      await eventually(() => sent.some((entry) => entry.call === "keyboard")),
      JSON.stringify(sent),
    );
    const start = sent.length;
    await page.keyboard.insertText("Привет, world");
    assert.ok(
      await eventually(() => reports(sent.slice(start)).length >= 26),
      JSON.stringify(sent.slice(start)),
    );
    const typing = sent.slice(start);
    const toRussian = typing.findIndex(
      (entry) => entry.call?.set_layout?.id === RUSSIAN.id,
    );
    const toEnglish = typing.findIndex(
      (entry) => entry.call?.set_layout?.id === ENGLISH.id,
    );
    assert.ok(toRussian >= 0 && toEnglish > toRussian, JSON.stringify(typing));
    assert.deepEqual(reports(typing.slice(0, toRussian)), []);
    // "Привет, " at the Russian layout's keys: П on G with Shift, р on H, и
    // on B, в on D, е on T, т on N, the comma on Shift with the slash's key.
    assert.deepEqual(reports(typing.slice(toRussian, toEnglish)), [
      ...typed(SHIFT, 0x0a),
      ...typed(0, 0x0b),
      ...typed(0, 0x05),
      ...typed(0, 0x07),
      ...typed(0, 0x17),
      ...typed(0, 0x11),
      ...typed(SHIFT, 0x38),
      ...typed(0, 0x2c),
    ]);
    // "world" at the US layout's keys, once the window had the layout.
    assert.deepEqual(reports(typing.slice(toEnglish)), [
      ...typed(0, 0x1a),
      ...typed(0, 0x12),
      ...typed(0, 0x15),
      ...typed(0, 0x0f),
      ...typed(0, 0x07),
    ]);
    const firstLatin = typing.slice(toEnglish).find((entry) => entry.report);
    const waited = firstLatin.at - typing[toEnglish].at;
    assert.ok(
      waited >= 50,
      `the first Latin key ${waited} ms after the switch`,
    );
    assert.equal(layoutCalls(typing).length, 2);

    // Latin text while the target is English switches nothing.
    const more = sent.length;
    await page.keyboard.insertText("ok");
    assert.ok(await eventually(() => reports(sent.slice(more)).length >= 4));
    await page.waitForTimeout(300);
    assert.deepEqual(layoutCalls(sent.slice(more)), []);

    // Cyrillic again: Russian, and the closed on-screen keyboard, English,
    // does not switch it back.
    const again = sent.length;
    await page.keyboard.insertText("да");
    assert.ok(await eventually(() => reports(sent.slice(again)).length >= 4));
    await page.waitForTimeout(600);
    assert.deepEqual(layoutCalls(sent.slice(again)), [
      { set_layout: { id: RUSSIAN.id } },
    ]);

    // The on-screen keyboard switches the target to its language as it
    // opens, and as its language is chosen.
    const lastCall = () =>
      JSON.stringify(layoutCalls(sent.slice(again)).at(-1));
    await page
      .locator("svg.lucide-keyboard:not(:has(+ svg.lucide-smartphone))")
      .first()
      .tap();
    await page.getByText("On-screen keyboard", { exact: true }).tap();
    const onScreen = page.getByTestId("touch-virtual-keyboard");
    await onScreen.waitFor();
    assert.ok(
      await eventually(
        () => lastCall() === JSON.stringify({ set_layout: { id: ENGLISH.id } }),
      ),
      lastCall(),
    );
    await onScreen.locator("select").selectOption("ru");
    assert.ok(
      await eventually(
        () => lastCall() === JSON.stringify({ set_layout: { id: RUSSIAN.id } }),
      ),
      lastCall(),
    );
    assert.equal(layoutCalls(sent.slice(again)).length, 3);
    assert.deepEqual(errors, []);
    await context.close();
  }

  {
    // With the option off the keys are typed as they are, and nothing is
    // switched.
    const { context, page, errors, sent } = await open({ follow: false });
    const start = sent.length;
    await page.keyboard.insertText("Привет");
    assert.ok(await eventually(() => reports(sent.slice(start)).length >= 12));
    await page.waitForTimeout(300);
    assert.deepEqual(layoutCalls(sent), []);
    // П on G with Shift, first of the keys pressed (all keys let go may come
    // before it, as the page's own keyboard is set aside for the phone's).
    const pressed = reports(sent.slice(start)).filter((report) =>
      report.some(Boolean),
    );
    assert.deepEqual(pressed[0], typed(SHIFT, 0x0a)[0]);
    assert.deepEqual(errors, []);
    await context.close();
  }
  {
    // Estonian: the layout for the letters only it has, then kept for
    // Latin letters, which it has where the US layout has them.
    const { context, page, errors, sent } = await open({
      follow: true,
      installed: [ENGLISH, RUSSIAN, ESTONIAN],
    });
    assert.ok(
      await eventually(() => sent.some((entry) => entry.call === "keyboard")),
    );
    const start = sent.length;
    await page.keyboard.insertText("Tänan, ok?");
    assert.ok(
      await eventually(() => reports(sent.slice(start)).length >= 20),
      JSON.stringify(sent.slice(start)),
    );
    const typing = sent.slice(start);
    const toEstonian = typing.findIndex(
      (entry) => entry.call?.set_layout?.id === ESTONIAN.id,
    );
    assert.ok(toEstonian >= 0, JSON.stringify(typing));
    assert.deepEqual(reports(typing.slice(0, toEstonian)), []);
    // T, ä on the US quote key, n a n, the comma, a space, o k, and the
    // question mark Shift on the US '-' key.
    assert.deepEqual(reports(typing.slice(toEstonian)), [
      ...typed(SHIFT, 0x17),
      ...typed(0, 0x34),
      ...typed(0, 0x11),
      ...typed(0, 0x04),
      ...typed(0, 0x11),
      ...typed(0, 0x36),
      ...typed(0, 0x2c),
      ...typed(0, 0x12),
      ...typed(0, 0x0e),
      ...typed(SHIFT, 0x2d),
    ]);
    const more = sent.length;
    await page.keyboard.insertText("hi");
    assert.ok(await eventually(() => reports(sent.slice(more)).length >= 4));
    await page.waitForTimeout(300);
    assert.deepEqual(layoutCalls(sent), [{ set_layout: { id: ESTONIAN.id } }]);
    assert.deepEqual(errors, []);
    await context.close();
  }
  console.log(
    'agent typing contract: ok (the target switched to Russian before "Привет, " and to English before "world", each after the keys before it; nothing switched for Latin text on English, by the closed on-screen keyboard or with the option off; the on-screen keyboard\'s language as it opens and as it is chosen; "Tänan, ok?" in the Estonian layout, kept for Latin text)',
  );
} finally {
  await browser.close();
  await new Promise((resolve) => app.server.close(resolve));
}
