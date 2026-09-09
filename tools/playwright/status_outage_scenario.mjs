import assert from "node:assert/strict";
import {
  statusProfile,
  statusRoute,
  validateStatus,
} from "./status_api_acceptance.mjs";

const idle = "No update awaiting confirmation";
const stale = "Connection lost. The displayed status may be outdated.";
const unavailable = "Update status is unavailable. Retrying automatically.";
const sizes = [
  { width: 1280, height: 900 },
  { width: 390, height: 844 },
];

async function openPanel(page) {
  await page.bringToFront();
  await page.setViewportSize(sizes[0]);
  await page.mouse.move(640, 1);
  await page.locator("svg.lucide-settings").first().click();
  const dialog = page.getByRole("dialog");
  await dialog.waitFor({ state: "visible" });
  await dialog.getByText("Check for Updates", { exact: true }).click();
  const panel = dialog.getByTestId("bli-application-update");
  await panel.waitFor({ state: "visible" });
  return { dialog, panel };
}

async function refresh(page, panel, status, profileName) {
  const response = page.waitForResponse(
    (item) =>
      new URL(item.url()).pathname === statusRoute &&
      item.request().method() === "GET",
    { timeout: 10_000 },
  );
  // Attach both promises immediately so a click failure cannot leave an
  // unhandled response timeout behind.
  const [result] = await Promise.all([
    response,
    panel.getByRole("button", { name: "Refresh", exact: true }).click(),
  ]);
  assert.equal(result.status(), status);
  if (status === 200) {
    const body = await result.json();
    assert.equal(body.code, 0);
    validateStatus(body.data, profileName);
  }
}

async function verifyViewports(page, panel, state, profileName) {
  const profile = statusProfile(profileName);
  const version = `${profile.version} · #${profile.sequence}`;
  for (const viewport of sizes) {
    await page.setViewportSize(viewport);
    if (state === "ready") {
      await panel.getByText(idle, { exact: true }).waitFor();
      assert.equal(await panel.getByText(stale, { exact: true }).count(), 0);
      assert.equal(
        await panel.getByText(unavailable, { exact: true }).count(),
        0,
      );
    } else {
      await panel
        .getByText(state === "stale" ? stale : unavailable, { exact: true })
        .waitFor();
      assert.equal(await panel.getByText(idle, { exact: true }).count(), 0);
    }
    assert.equal(
      await panel.getByText(version, { exact: true }).count(),
      state === "error" ? 0 : 2,
    );
    assert.equal(await panel.locator('input[type="file"]').count(), 0);
    assert.equal(
      await panel
        .getByRole("button", { name: /upload|activate|install/i })
        .count(),
      0,
    );
  }
}

// The coordinator must resolve stop only after guarded listener/worker drain,
// and restore only after the original socket and serving identity are verified.
// This module neither simulates HTTP failures nor runs privileged commands.
export function createStatusOutageScenario(control) {
  assert.equal(typeof control, "function");
  return async ({
    page,
    beginOutage,
    endOutage,
    activeStream,
    verifyVideoProgress,
    createObservedPage,
    statusProfileName = "seq5",
  }) => {
    statusProfile(statusProfileName);
    const originalStream = activeStream();
    assert.ok(originalStream);
    let cold = null;
    let restoreRequired = false;
    let windowOpen = false;
    try {
      const warm = await openPanel(page);
      await refresh(page, warm.panel, 200, statusProfileName);
      await verifyViewports(page, warm.panel, "ready", statusProfileName);
      await verifyVideoProgress();
      beginOutage();
      windowOpen = true;
      // A failed stop acknowledgement may still mean the listener stopped.
      restoreRequired = true;
      await control("stop");
      await refresh(page, warm.panel, 503, statusProfileName);
      await verifyViewports(page, warm.panel, "stale", statusProfileName);
      await verifyVideoProgress();

      cold = await createObservedPage();
      const document = await cold.goto(page.url(), {
        waitUntil: "domcontentloaded",
      });
      assert.equal(document?.status(), 200);
      const fresh = await openPanel(cold);
      await refresh(cold, fresh.panel, 503, statusProfileName);
      await verifyViewports(cold, fresh.panel, "error", statusProfileName);
      await verifyVideoProgress();

      await control("restore");
      restoreRequired = false;
      endOutage();
      windowOpen = false;
      for (const [target, view] of [
        [cold, fresh],
        [page, warm],
      ]) {
        await target.bringToFront();
        await refresh(target, view.panel, 200, statusProfileName);
        await verifyViewports(target, view.panel, "ready", statusProfileName);
        await view.dialog.locator("button.ant-modal-close").click();
        await view.dialog.waitFor({ state: "hidden" });
      }
      assert.equal(
        activeStream(),
        originalStream,
        "MJPEG response was replaced",
      );
      await verifyVideoProgress();
      assert.equal(
        await page
          .locator("img#screen")
          .evaluate(
            (screen) =>
              screen instanceof HTMLImageElement && screen.naturalWidth > 0,
          ),
        true,
      );
      return {
        staleVerified: true,
        coldErrorVerified: true,
        recoveryVerified: true,
        videoPreserved: true,
      };
    } finally {
      // The host also owns an independent restore timer for process/SSH loss.
      try {
        if (restoreRequired) await control("restore");
      } finally {
        try {
          if (windowOpen) endOutage();
        } finally {
          if (cold) await cold.close();
          await page.setViewportSize(sizes[0]);
        }
      }
    }
  };
}
