import assert from "node:assert/strict";
import { runExistingAccountBrowser } from "./device_existing_account_smoke.mjs";
import { verifyStatusPanel } from "./status_api_acceptance.mjs";

await runExistingAccountBrowser({
  videoContinuityScenario: async ({
    page,
    activeStream,
    createObservedPage,
    progress,
    observe,
    verifyVideoProgress,
  }) => {
    const original = activeStream();
    assert.ok(original);
    let ended = false;
    original.finished().then(
      () => {
        ended = true;
      },
      () => {
        ended = true;
      },
    );
    const verifyStream = async () => {
      observe({ sameResponse: activeStream() === original, finished: ended });
      assert.equal(activeStream(), original, "primary video response replaced");
      await verifyVideoProgress();
    };
    let cold = null;
    try {
      progress("warm-viewports");
      await verifyStatusPanel(page, true);
      await verifyStream();
      progress("cold-document");
      cold = await createObservedPage();
      assert.equal(
        (
          await cold.goto(page.url(), { waitUntil: "domcontentloaded" })
        )?.status(),
        200,
      );
      await cold.bringToFront();
      progress("cold-viewports");
      await verifyStatusPanel(cold, true);
      await verifyStream();
      progress("cold-close");
      await cold.close();
      progress("warm-return");
      await page.bringToFront();
      await verifyStatusPanel(page, true);
      await verifyStream();
      progress("complete");
      return { videoPreserved: true };
    } finally {
      if (cold && !cold.isClosed()) await cold.close();
    }
  },
});
