import assert from "node:assert/strict";
import crypto from "node:crypto";
import {
  statusProfile,
  statusRoute,
  validateStatus,
} from "./status_api_acceptance.mjs";

export const uploadRoute = "/api/application-update/upload";
export const activationRoute = "/api/application-update/activate";
export const bundleSha256 =
  "d2acf270fa2aa9cb924c362066ece50f4b0fbd4c4c8b2975648dab5ac0b4f849";
export const bundleBytes = 8724480;

export function validateUploadBody(body, selectedArchiveVerified) {
  assert.equal(selectedArchiveVerified, true);
  // Chromium omits File-backed XHR post data (including Content-Length)
  // from interception. Never replace it with a synthetic request body.
  if (body === null) return false;
  assert.equal(body.length, bundleBytes);
  assert.equal(
    crypto.createHash("sha256").update(body).digest("hex"),
    bundleSha256,
  );
  return true;
}

export async function waitLoginInputs(page) {
  const inputs = page.locator("input");
  // The title effect can precede rendering the lazily loaded form.
  await inputs.nth(1).waitFor({ state: "visible" });
  assert.equal(await inputs.count(), 2);
  return inputs;
}

export function validateStaged(body) {
  const target = statusProfile("seq8");
  assert.equal(body.code, 0);
  assert.deepEqual(body.data, {
    slot: {
      slotId: target.slot,
      version: target.version,
      sequence: "8",
      minimumSystemSequence: "1",
    },
    disposition: "already_present",
    manifestSha256: target.slot.slice(25),
    payloadBytes: "8683648",
  });
}

export function validateActivation(body) {
  assert.equal(body.code, 0);
  assert.deepEqual(Object.keys(body.data).sort(), ["phase", "receipt"]);
  assert.equal(body.data.phase, "awaiting_health");
  assert.deepEqual(Object.keys(body.data.receipt).sort(), [
    "operationId",
    "targetSlotId",
  ]);
  assert.equal(body.data.receipt.targetSlotId, statusProfile("seq8").slot);
  assert.match(body.data.receipt.operationId, /^[a-f0-9]{32}$/);
  assert.notEqual(body.data.receipt.operationId, "0".repeat(32));
}

export function validateActivationRequest(body) {
  assert.deepEqual(body, {
    slotId: statusProfile("seq8").slot,
    expectedGeneration: "37",
  });
}

export async function readStatus(page, profile) {
  const result = await page.evaluate(async (route) => {
    const response = await fetch(route, { credentials: "include" });
    return { status: response.status, body: await response.json() };
  }, statusRoute);
  assert.equal(result.status, 200);
  assert.equal(result.body.code, 0);
  validateStatus(result.body.data, profile);
  return result.body.data;
}

export async function runApplicationWriteUi(
  page,
  archivePath,
  archiveVerified = () => {},
  progress = () => {},
) {
  progress({ step: "status-before" });
  await readStatus(page, "seq7");
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.mouse.move(640, 1);
  await page.locator("svg.lucide-settings").first().click();
  const settings = page.getByRole("dialog");
  await settings.getByText("Check for Updates", { exact: true }).click();
  const panel = settings.getByTestId("bli-application-upload");
  await panel.waitFor({ state: "visible" });
  await panel.getByTestId("application-archive").setInputFiles(archivePath);
  const selected = await panel
    .getByTestId("application-archive")
    .evaluate(async (input) => {
      const file = input.files[0];
      const digest = new Uint8Array(
        await crypto.subtle.digest("SHA-256", await file.arrayBuffer()),
      );
      return {
        bytes: file.size,
        sha256: Array.from(digest, (value) =>
          value.toString(16).padStart(2, "0"),
        ).join(""),
      };
    });
  assert.deepEqual(selected, { bytes: bundleBytes, sha256: bundleSha256 });
  archiveVerified();
  progress({ step: "upload-send" });
  const uploaded = page.waitForResponse(
    (response) =>
      new URL(response.url()).pathname === uploadRoute &&
      response.request().method() === "POST",
    { timeout: 45000 },
  );
  await panel
    .getByRole("button", { name: "Upload and verify", exact: true })
    .click();
  const response = await uploaded;
  progress({ step: "upload-response", httpStatus: response.status() });
  const uploadBody = await response.json();
  const knownMessages = new Set([
    "success",
    "upload body length mismatch",
    "archive verification failed",
    "another upload is in progress",
    "upload staging quota exceeded",
    "service unavailable",
    "upload requires an exact unencoded application/octet-stream body of 1 to 134217728 bytes",
  ]);
  progress({
    step: "upload-receipt",
    httpStatus: response.status(),
    code: Number.isSafeInteger(uploadBody.code) ? uploadBody.code : null,
    messageKind: knownMessages.has(uploadBody.msg)
      ? uploadBody.msg
      : "unclassified",
  });
  assert.equal(response.status(), 200);
  validateStaged(uploadBody);
  progress({ step: "staged-ui" });
  await panel
    .getByText("This verified archive is already staged", { exact: true })
    .waitFor();
  await readStatus(page, "seq7");
  progress({ step: "review" });
  await panel.getByTestId("application-activation-review").click();
  const confirm = page.getByTestId("application-activation-confirm");
  const dialog = page.getByRole("dialog").filter({ has: confirm });
  await dialog
    .getByText("Current confirmed application: 0.1.0-dev6 · #7", {
      exact: true,
    })
    .waitFor();
  await dialog.getByText("Journal generation: 37", { exact: true }).waitFor();
  await dialog.getByText(statusProfile("seq8").slot, { exact: true }).waitFor();
  const activated = page.waitForResponse(
    (item) =>
      new URL(item.url()).pathname === activationRoute &&
      item.request().method() === "POST",
    { timeout: 20000 },
  );
  // No retry, even when receipt delivery is lost. The root guard reconciles
  // authoritative state before restoration; a click is never called success.
  await confirm.click();
  const activation = await activated;
  validateActivationRequest(activation.request().postDataJSON());
  assert.equal(activation.status(), 202);
  validateActivation(await activation.json());
  return {
    uploadedAlreadyPresent: true,
    generationReviewed: "37",
    activationStatus: 202,
    activationReceiptValidated: true,
    confirmedApplication: false,
  };
}
