// Device test of virtual media through the GUI, as an operator uses it: a
// small ISO uploaded from the Download menu, mounted as a CD-ROM from the
// image dialog, checked through the API, unmounted and deleted. Every
// virtual-media and storage answer is recorded. The GUI password is read from
// stdin; a receipt is written under evidence/private. No screenshot is taken.
//
// Usage: node media_gui_device_test.mjs BASE_URL USERNAME OUTPUT ISO_PATH VIEWPORT
//   VIEWPORT: desktop | mobile
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const [baseUrl, username, outputValue, isoPath, viewport] =
  process.argv.slice(2);
assert.equal(new URL(baseUrl).protocol, "https:");
const output = path.resolve(outputValue);
assert.equal(path.dirname(output), path.resolve("evidence/private"));
assert.ok(!fs.existsSync(output));
assert.ok(fs.existsSync(isoPath) && isoPath.toLowerCase().endsWith(".iso"));
assert.ok(["desktop", "mobile"].includes(viewport));
const imageName = path.basename(isoPath);
let password = "";
for await (const chunk of process.stdin) password += chunk.toString("utf8");
password = password.replace(/[\r\n]+$/, "");
assert.ok(password.length >= 8);

const { chromium } = await import(
  pathToFileURL(
    process.env.PLAYWRIGHT_MODULE ??
      "/home/w0w/.local/share/playwright/node_modules/playwright/index.mjs",
  ).href
);
const browser = await chromium.launch({
  headless: true,
  executablePath:
    process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH ??
    "/home/w0w/.cache/ms-playwright/chromium-1228/chrome-linux64/chrome",
});
const mobile = viewport === "mobile";
const context = await browser.newContext({
  ignoreHTTPSErrors: true,
  locale: "en-US",
  ...(mobile
    ? { viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true }
    : { viewport: { width: 1440, height: 1000 } }),
});
const receipt = {
  schemaVersion: 1,
  probe: "media-gui-device",
  viewport,
  imageName,
  steps: [],
};
const record = (step, value) => {
  receipt.steps.push({ step, at: new Date().toISOString(), ...value });
  console.log(JSON.stringify({ step, ...value }).slice(0, 500));
};

let result = "failed";
try {
  const login = await context.request.post(`${baseUrl}/api/auth/login`, {
    data: { username, password },
    headers: { Origin: baseUrl },
  });
  password = "";
  const csrf = (await login.json()).data?.csrfToken;
  assert.ok(csrf, "login failed");
  const api = async (route) =>
    (
      await context.request.get(`${baseUrl}${route}`, {
        headers: { Origin: baseUrl, "X-CSRF-Token": csrf },
      })
    ).json();
  const detach = async () =>
    (
      await context.request.post(`${baseUrl}/api/virtual-media`, {
        headers: { Origin: baseUrl, "X-CSRF-Token": csrf },
        data: {
          operationId: randomBytes(16).toString("hex"),
          command: { action: "set_connected", connected: false },
        },
      })
    ).json();
  // A previous run may have left the drive attached.
  if ((await api("/api/storage/image/mounted")).data?.file) {
    record("detach_before", { answer: (await detach()).data });
    await new Promise((resolve) => setTimeout(resolve, 3000));
  }
  const page = await context.newPage();
  page.setDefaultTimeout(15000);
  const answers = [];
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("response", async (response) => {
    const url = new URL(response.url());
    if (!/\/api\/(download\/file|virtual-media|storage)/.test(url.pathname))
      return;
    let body = null;
    try {
      body = await response.json();
    } catch {
      body = null;
    }
    answers.push({
      method: response.request().method(),
      path: url.pathname,
      status: response.status(),
      code: body?.code,
      msg: body?.code === 0 ? undefined : body?.msg,
      data:
        body?.data && typeof body.data === "object" && "applied" in body.data
          ? body.data
          : undefined,
    });
  });
  const take = () => answers.splice(0, answers.length);

  await page.goto(`${baseUrl}/`, { waitUntil: "domcontentloaded" });
  await page.locator("svg.lucide-download").first().waitFor({ timeout: 30000 });

  // Upload from the Download menu.
  await page.locator("svg.lucide-download").first().click();
  await page.locator("#file-upload").setInputFiles(isoPath);
  // The upload button sits beside the file drop area; the URL form above it
  // has its own.
  await page
    .locator("#file-upload")
    .locator("xpath=ancestor::div[.//button][1]")
    .locator("button.ant-btn-primary")
    .click();
  const outcome = page
    .locator(
      ".ant-popover:visible .text-green-500, .ant-popover:visible .text-red-500",
    )
    .first();
  await outcome.waitFor({ timeout: 120000 });
  await page.waitForTimeout(1000);
  const uploadText = (await outcome.innerText()).trim();
  record("upload", { shown: uploadText, answers: take() });
  await page.keyboard.press("Escape");
  await page.mouse.click(mobile ? 380 : 1400, mobile ? 830 : 980);
  const catalog = (await api("/api/virtual-media/catalog")).data?.images ?? [];
  const uploaded = catalog.find((image) => image.imageName === imageName);
  record("catalog_after_upload", {
    present: Boolean(uploaded),
    count: catalog.length,
  });
  assert.ok(uploaded, "the uploaded image is not in the catalog");

  // Mount as a CD-ROM from the image dialog.
  await page.locator("svg.lucide-disc").first().click();
  const dialog = page.locator(".ant-modal-wrap:visible").first();
  await dialog.getByText("CD ROM", { exact: true }).click();
  const row = dialog
    .locator("div.group")
    .filter({ hasText: imageName })
    .first();
  await row.waitFor();
  await row.click();
  await page
    .waitForFunction(
      (name) =>
        [...document.querySelectorAll(".ant-modal-wrap div.group")].some(
          (element) =>
            element.textContent.includes(name) &&
            element.className.includes("text-blue-500"),
        ),
      imageName,
      { timeout: 30000 },
    )
    .catch(() => {});
  const mounted = (await api("/api/storage/image/mounted")).data;
  const cdrom = (await api("/api/storage/cdrom")).data;
  record("mount", { mounted, cdrom, answers: take() });
  assert.equal(mounted?.file, imageName, "the image is not mounted");

  // Unmount, after the owner's cooldown between media commands.
  await page.waitForTimeout(3000);
  await row.click();
  await page.waitForTimeout(3000);
  const afterUnmount = (await api("/api/storage/image/mounted")).data;
  record("unmount", { mounted: afterUnmount, answers: take() });
  assert.ok(!afterUnmount?.file, "the image is still mounted");

  // Delete.
  await row
    .getByRole("button", { name: new RegExp(imageName.replace(".", "\\.")) })
    .click();
  await page.locator(".ant-modal-confirm .ant-btn-dangerous").click();
  await page.waitForTimeout(2000);
  const finalCatalog =
    (await api("/api/virtual-media/catalog")).data?.images ?? [];
  record("delete", {
    present: finalCatalog.some((image) => image.imageName === imageName),
    answers: take(),
  });
  assert.ok(
    !finalCatalog.some((image) => image.imageName === imageName),
    "the image was not deleted",
  );
  record("page_errors", { errors });
  assert.deepEqual(errors, []);
  result = "passed";
} catch (error) {
  record("error", { message: String(error?.message ?? error).slice(0, 400) });
} finally {
  receipt.result = result;
  fs.writeFileSync(output, JSON.stringify(receipt, null, 1), { mode: 0o600 });
  await browser.close();
}
console.log(`result: ${result}`);
process.exit(result === "passed" ? 0 : 1);
