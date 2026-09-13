import assert from "node:assert/strict";

export const statusRoute = "/api/application-update/status";
const profiles = Object.freeze({
  seq5: Object.freeze({
    generation: "22",
    sequence: "5",
    version: "0.1.0-dev4",
    slot: "app-00000000000000000005-41cb7620d36a47c3416e748296fa1512fc59aaf9012f12f04c0f659098629ebc",
  }),
  seq6: Object.freeze({
    generation: "31",
    sequence: "6",
    version: "0.1.0-dev5",
    slot: "app-00000000000000000006-8eedeb5ce21ca08b90336cad27bdd7f16c6502388a1f13bc943a2026d80534ac",
  }),
  seq7: Object.freeze({
    generation: "37",
    sequence: "7",
    version: "0.1.0-dev6",
    slot: "app-00000000000000000007-f2262e8f9929dda83b2baf1005b5ae9262aa075b7ddaa16f4ad6200b5daf0140",
  }),
  seq8: Object.freeze({
    generation: "40",
    sequence: "8",
    version: "0.1.0-dev6",
    slot: "app-00000000000000000008-375f9800a147b8f273073349af1613e3fb0cf9f7b97af914d7a75e9446ea694b",
  }),
  seq11: Object.freeze({
    generation: "49",
    sequence: "11",
    version: "0.1.0-dev7",
    slot: "app-00000000000000000011-d6283a7be390423188d8931bae148d9555305a187f18049796cca6b793621d25",
  }),
  seq10: Object.freeze({
    generation: "46",
    sequence: "10",
    version: "0.1.0-dev7",
    slot: "app-00000000000000000010-8a30162d0b620c2f7a6679061d2c9c4bf863aa8ac1be2d2e6dfa77caacc6ae01",
  }),
  seq9: Object.freeze({
    generation: "43",
    sequence: "9",
    version: "0.1.0-dev6",
    slot: "app-00000000000000000009-5f1aee4802addd6c0c0b444224345624cd8b9b8c46950a2231ad7e2b42de37c2",
  }),
});

export function statusProfile(name = "seq5") {
  assert.equal(typeof name, "string");
  assert.ok(Object.hasOwn(profiles, name), "unknown status acceptance profile");
  return profiles[name];
}

export function validateStatus(value, profileName = "seq5") {
  const profile = statusProfile(profileName);
  assert.equal(value.schemaVersion, 1);
  assert.equal(value.generation, profile.generation);
  assert.equal(value.floorSequence, profile.sequence);
  assert.equal(value.systemSequence, "1");
  assert.equal(value.pending, null);
  assert.equal(value.lastRollback, null);
  assert.deepEqual(value.selected, value.confirmed);
  assert.equal(value.confirmed.slotId, profile.slot);
  assert.equal(value.confirmed.version, profile.version);
  assert.equal(value.confirmed.sequence, profile.sequence);
  assert.equal(value.confirmed.minimumSystemSequence, "1");
}

export async function verifyStatusPanel(
  page,
  enabled,
  progress = () => {},
  profileName = "seq5",
) {
  const profile = statusProfile(profileName);
  let first = null;
  let desktopSettingsOffscreenBeforeReveal = false;
  if (enabled) {
    for (let index = 0; index < 3; index++) {
      progress(`read-${index + 1}`);
      const result = await page.evaluate(async (route) => {
        const response = await fetch(route, { credentials: "include" });
        return { status: response.status, body: await response.json() };
      }, statusRoute);
      assert.equal(result.status, 200);
      assert.equal(result.body.code, 0);
      validateStatus(result.body.data, profileName);
      if (first) assert.deepEqual(result.body.data, first);
      first = result.body.data;
    }
  }
  for (const viewport of [
    { width: 1280, height: 900 },
    { width: 390, height: 844 },
  ]) {
    const size = viewport.width === 1280 ? "desktop" : "mobile";
    progress(`${size}-viewport`);
    await page.setViewportSize(viewport);
    if (size === "desktop") {
      progress("desktop-reveal-toolbar");
      const bounds = await page
        .locator("svg.lucide-settings")
        .first()
        .boundingBox();
      desktopSettingsOffscreenBeforeReveal = Boolean(bounds && bounds.y < 0);
      // The desktop toolbar auto-hides after idle. Use its real hover trigger,
      // not a forced click or a change to the user's menu preferences.
      await page.mouse.move(viewport.width / 2, 1);
    }
    progress(`${size}-open-settings`);
    await page.locator("svg.lucide-settings").first().click();
    const dialog = page.getByRole("dialog");
    progress(`${size}-dialog-visible`);
    await dialog.waitFor({ state: "visible" });
    const update = dialog.getByText("Check for Updates", { exact: true });
    if (enabled) {
      progress(`${size}-open-updates`);
      await update.click();
      const panel = dialog.getByTestId("bli-application-update");
      progress(`${size}-status-visible`);
      await panel
        .getByText("No update awaiting confirmation", { exact: true })
        .waitFor();
      assert.equal(
        await panel
          .getByText(`${profile.version} · #${profile.sequence}`, {
            exact: true,
          })
          .count(),
        2,
      );
      await panel
        .getByText(
          "Status only. Upload and activation are not enabled for this interface yet.",
          { exact: true },
        )
        .waitFor();
      assert.equal(await panel.locator('input[type="file"]').count(), 0);
      assert.equal(
        await panel
          .getByRole("button", { name: /upload|activate|install/i })
          .count(),
        0,
      );
      progress(`${size}-refresh`);
      const response = page.waitForResponse(
        (item) =>
          new URL(item.url()).pathname === statusRoute &&
          item.request().method() === "GET",
      );
      await panel.getByRole("button", { name: "Refresh", exact: true }).click();
      const refreshed = await response;
      assert.equal(refreshed.status(), 200);
      validateStatus((await refreshed.json()).data, profileName);
    } else {
      assert.equal(await update.count(), 0);
    }
    progress(`${size}-close-settings`);
    await dialog.locator("button.ant-modal-close").click();
    await dialog.waitFor({ state: "hidden" });
  }
  progress("restore-desktop");
  await page.setViewportSize({ width: 1280, height: 900 });
  progress("complete");
  return {
    enabled,
    desktop: true,
    mobile: true,
    repeatedReads: enabled ? 3 : 0,
    refreshChecks: enabled ? 2 : 0,
    desktopSettingsOffscreenBeforeReveal,
  };
}
