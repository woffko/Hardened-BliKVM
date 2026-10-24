// The page's on-screen keyboard (web/src/pages/desktop/virtual-keyboard), on a phone and on a desktop, through the
// built page and the lease. A modifier tapped stays pressed for the next key: its report goes first, then the key
// with it, then the key up and the modifiers up, as on a keyboard; no report names a modifier as a key, which the
// board refuses (and a refused report ends the lease). Ctrl+V, Shift+A and Ctrl+Shift+Esc are typed so, a
// modifier tapped twice is pressed and let go alone (the Windows key opens Start), and remote control stays on. A
// pressed modifier reads white on blue, not white on white.
import assert from "node:assert/strict";
import { startApp } from "./app_harness.mjs";
import { controlIsOn, takeControl } from "./take_control.mjs";
import { launchChromium } from "./quality_fixture.mjs";

const browser = await launchChromium();
const app = await startApp();
const CTRL = 0x01;
const SHIFT = 0x02;
const WIN = 0x08;
const KEY_A = 0x04;
const KEY_V = 0x19;
const ESCAPE = 0x29;
const report = (modifiers, usage = 0) => [modifiers, 0, usage, 0, 0, 0, 0, 0];

const reports = () =>
  app.requestDetails
    .filter(
      (request) =>
        request.route === "/api/hid/lease" &&
        request.body?.action === "report" &&
        request.body.report?.mode === "keyboard",
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

/** The reports some taps send: exactly these, and no more shortly after. */
async function sends(taps, expected, message) {
  const before = reports().length;
  await taps();
  await eventually(() => reports().length >= before + expected.length);
  await new Promise((resolve) => setTimeout(resolve, 300));
  assert.deepEqual(reports().slice(before), expected, message);
}

/** The contrast of a control's text on its own background. */
function contrastOf(element) {
  const channels = (colour) => colour.match(/[\d.]+/g).map(Number);
  const luminance = ([r, g, b]) => {
    const linear = (value) => {
      const share = value / 255;
      return share <= 0.04045
        ? share / 12.92
        : ((share + 0.055) / 1.055) ** 2.4;
    };
    return 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
  };
  const style = getComputedStyle(element);
  const [lighter, darker] = [
    luminance(channels(style.color)),
    luminance(channels(style.backgroundColor)),
  ].sort((a, b) => b - a);
  return (lighter + 0.05) / (darker + 0.05);
}

async function open(phone) {
  const context = await browser.newContext(
    phone
      ? {
          viewport: { width: 390, height: 844 },
          isMobile: true,
          hasTouch: true,
        }
      : { viewport: { width: 1280, height: 800 } },
  );
  await context.addCookies([
    { name: "hardened-blikvm-csrf", value: "fixture-csrf", url: app.baseUrl },
  ]);
  const page = await context.newPage();
  page.setDefaultTimeout(8000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto(app.baseUrl, { waitUntil: "networkidle" });
  await takeControl(page);
  const menu = page
    .locator("svg.lucide-keyboard:not(:has(+ svg.lucide-smartphone))")
    .first();
  if (phone) {
    await menu.tap();
    await page.getByText("On-screen keyboard", { exact: true }).tap();
    await page.getByTestId("touch-virtual-keyboard").waitFor();
  } else {
    await menu.click();
    await page.getByText("On-screen keyboard", { exact: true }).click();
    await page.locator(".hg-button").first().waitFor();
  }
  return { context, page, errors };
}

try {
  for (const phone of [true, false]) {
    const { context, page, errors } = await open(phone);
    const where = phone ? "phone" : "desktop";
    // A key by its label on the phone, by its token on the desktop.
    const key = (label, token) =>
      phone
        ? page
            .getByTestId("touch-virtual-keyboard")
            .getByRole("button", { name: label, exact: true })
            .first()
        : page.locator(`.hg-button[data-skbtn="${token}"]`).first();
    const tap = async (label, token) => {
      const target = key(label, token);
      if (phone) await target.tap();
      else await target.click();
    };

    // Ctrl, then V: Ctrl+V.
    await tap("Ctrl", "{controlleft}");
    const contrast = await key("Ctrl", "{controlleft}").evaluate(contrastOf);
    assert.ok(contrast >= 3, `${where}: a pressed Ctrl's contrast ${contrast}`);
    await sends(
      () => tap("V", "KeyV"),
      [report(CTRL), report(CTRL, KEY_V), report(CTRL), report(0)],
      `${where}: Ctrl+V`,
    );
    // The modifier was for one key: V alone now.
    await sends(
      () => tap("V", "KeyV"),
      [report(0, KEY_V), report(0)],
      `${where}: V after Ctrl+V`,
    );
    await sends(
      async () => {
        await tap("Shift", "{shiftleft}");
        await tap("A", "KeyA");
      },
      [report(SHIFT), report(SHIFT, KEY_A), report(SHIFT), report(0)],
      `${where}: Shift+A`,
    );
    await sends(
      async () => {
        await tap("Ctrl", "{controlleft}");
        await tap("Shift", "{shiftleft}");
        await tap("Esc", "{escape}");
      },
      [
        report(CTRL | SHIFT),
        report(CTRL | SHIFT, ESCAPE),
        report(CTRL | SHIFT),
        report(0),
      ],
      `${where}: Ctrl+Shift+Esc`,
    );
    // Win tapped twice: pressed and let go alone.
    await sends(
      async () => {
        await tap("Win", "{winleft}");
        await tap("Win", "{winleft}");
      },
      [report(WIN), report(0)],
      `${where}: Win alone`,
    );
    // No report named a modifier as a key, and control is still on.
    assert.ok(
      reports().every((data) =>
        data.every((value) => Number.isInteger(value) && value <= 0xe7),
      ),
      JSON.stringify(reports()),
    );
    assert.equal(
      await controlIsOn(page),
      true,
      `${where}: remote control turned off`,
    );
    if (phone) {
      // The Russian layout: Й is the US Q key, Э the quote key; the German
      // one: Z is the US Y key, the key left of Y is the ISO key.
      const language = page
        .getByTestId("touch-virtual-keyboard")
        .locator("select");
      await language.selectOption("ru");
      await sends(
        async () => {
          await tap("Й", "RusQ");
          await tap("Э", "RusQuote");
        },
        [report(0, 0x14), report(0), report(0, 0x34), report(0)],
        "phone: Russian keys",
      );
      await language.selectOption("de");
      await sends(
        async () => {
          await tap("Z", "KeyZ_qwertz");
          await tap("<|>", "IntlBackslash_qwertz");
        },
        [report(0, 0x1c), report(0), report(0, 0x64), report(0)],
        "phone: German keys",
      );
      await language.selectOption("en");
    }
    assert.deepEqual(errors, []);
    await context.close();
  }
  console.log(
    "on-screen keyboard contract: ok (phone and desktop: Ctrl+V, Shift+A and Ctrl+Shift+Esc with the modifiers first and last, Win alone on a second tap, control kept on, a pressed Ctrl readable; the Russian and German keys at their US places)",
  );
} finally {
  await browser.close();
  await new Promise((resolve) => app.server.close(resolve));
}
