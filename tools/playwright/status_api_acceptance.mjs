import assert from "node:assert/strict";

export const statusRoute = "/api/application-update/status";
const slot =
  "app-00000000000000000004-768b33ac3a022f4a216ae1db4a08e7926802eabb4842b0de1dc80c74aa70095c";

export function validateStatus(value) {
  assert.equal(value.schemaVersion, 1);
  assert.equal(value.generation, "16");
  assert.equal(value.floorSequence, "4");
  assert.equal(value.systemSequence, "1");
  assert.equal(value.pending, null);
  assert.equal(value.lastRollback, null);
  assert.deepEqual(value.selected, value.confirmed);
  assert.equal(value.confirmed.slotId, slot);
  assert.equal(value.confirmed.version, "0.1.0-dev3");
  assert.equal(value.confirmed.sequence, "4");
  assert.equal(value.confirmed.minimumSystemSequence, "1");
}

export async function verifyStatusPanel(page, enabled) {
  let first = null;
  if (enabled) {
    for (let index = 0; index < 3; index++) {
      const result = await page.evaluate(async (route) => {
        const response = await fetch(route, { credentials: "include" });
        return { status: response.status, body: await response.json() };
      }, statusRoute);
      assert.equal(result.status, 200);
      assert.equal(result.body.code, 0);
      validateStatus(result.body.data);
      if (first) assert.deepEqual(result.body.data, first);
      first = result.body.data;
    }
  }
  for (const viewport of [
    { width: 1280, height: 900 },
    { width: 390, height: 844 },
  ]) {
    await page.setViewportSize(viewport);
    await page.locator("svg.lucide-settings").first().click();
    const dialog = page.getByRole("dialog");
    await dialog.waitFor({ state: "visible" });
    const update = dialog.getByText("Check for Updates", { exact: true });
    if (enabled) {
      await update.click();
      const panel = dialog.getByTestId("bli-application-update");
      await panel
        .getByText("No update awaiting confirmation", { exact: true })
        .waitFor();
      assert.equal(
        await panel.getByText("0.1.0-dev3 · #4", { exact: true }).count(),
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
      const response = page.waitForResponse(
        (item) =>
          new URL(item.url()).pathname === statusRoute &&
          item.request().method() === "GET",
      );
      await panel.getByRole("button", { name: "Refresh", exact: true }).click();
      const refreshed = await response;
      assert.equal(refreshed.status(), 200);
      validateStatus((await refreshed.json()).data);
    } else {
      assert.equal(await update.count(), 0);
    }
    await dialog.locator("button.ant-modal-close").click();
    await dialog.waitFor({ state: "hidden" });
  }
  await page.setViewportSize({ width: 1280, height: 900 });
  return {
    enabled,
    desktop: true,
    mobile: true,
    repeatedReads: enabled ? 3 : 0,
    refreshChecks: enabled ? 2 : 0,
  };
}
