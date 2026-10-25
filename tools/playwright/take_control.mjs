// Remote control is taken by the first press on the screen (or a key), which
// asks for a confirmation and does not reach the target itself; it is given
// back from the Mouse menu (2026-10-24: no button on the toolbar).

/**
 * Asks for control with a key (Shift: the press itself is not sent), or with
 * a tap on the screen's middle when `tap` (the screen needs a picture), then
 * confirms.
 */
export async function takeControl(page, { tap = false } = {}) {
  const confirm = page.getByRole("button", { name: "Confirm and control" });
  // A page that has not loaded what it offers yet hears no press: it is
  // pressed again until the confirmation opens.
  const deadline = Date.now() + 15000;
  for (;;) {
    if (tap) {
      const screen = page.locator("#screen").first();
      await screen.waitFor({ state: "visible" });
      const box = await screen.boundingBox();
      await page.touchscreen.tap(box.x + box.width / 2, box.y + box.height / 2);
    } else {
      await page.keyboard.press("Shift");
    }
    const opened = await confirm
      .waitFor({ state: "visible", timeout: 1000 })
      .then(() => true)
      .catch(() => false);
    if (opened) break;
    if (Date.now() > deadline)
      throw new Error("no confirmation of remote control opened");
  }
  if (tap) await confirm.tap();
  else await confirm.click();
  await confirm.waitFor({ state: "hidden" });
  await page.waitForFunction(
    () => document.documentElement.dataset.remoteControl === "on",
  );
}

/** Gives control back through the Mouse menu. */
export async function releaseControl(page) {
  const mouseMenu = page.locator("svg.lucide-mouse").first();
  await mouseMenu.click();
  await page.getByRole("button", { name: "Release remote control" }).click();
  await page.waitForFunction(
    () => document.documentElement.dataset.remoteControl === "off",
  );
  // A phone's menu closes after a choice, a desktop's stays open: it is
  // closed from its own icon, since a click on the screen would ask for
  // control.
  await page.waitForTimeout(100);
  if (await page.locator(".ant-popover:not(.ant-popover-hidden)").count()) {
    await mouseMenu.click();
  }
}

/** Whether the page holds the operator's confirmation. */
export async function controlIsOn(page) {
  return page.evaluate(
    () => document.documentElement.dataset.remoteControl === "on",
  );
}

/** A one-frame MJPEG stream, so that the screen shows a picture to press. */
export async function serveStillPicture(page) {
  const jpeg = await page.evaluate(async () => {
    const canvas = new OffscreenCanvas(64, 36);
    const context = canvas.getContext("2d");
    context.fillStyle = "#606060";
    context.fillRect(0, 0, 64, 36);
    const blob = await canvas.convertToBlob({
      type: "image/jpeg",
      quality: 0.7,
    });
    return Array.from(new Uint8Array(await blob.arrayBuffer()));
  });
  const body = Buffer.concat([
    Buffer.from(
      `--frame\r\nContent-Type: image/jpeg\r\nContent-Length: ${jpeg.length}\r\n\r\n`,
    ),
    Buffer.from(jpeg),
    Buffer.from("\r\n"),
  ]);
  await page.route("**/api/stream/mjpeg**", (route) =>
    route.fulfill({
      status: 200,
      headers: { "content-type": "multipart/x-mixed-replace; boundary=frame" },
      body,
    }),
  );
}
