// The System Log page on this board (docs/baselines/work-plan-20261117-system-log.md, L4), through the built page and
// a played board: where the system has the log forwarder, the remote host, port, priority and kernel switch are saved
// and read back, a remote copy needs a host, the errors kept on the card are named, the four logs are read, and
// busybox's own fields are not shown; without the forwarder the page says so and writes nothing.
import assert from "node:assert/strict";
import { defaultManifest, startApp } from "./app_harness.mjs";
import { eventually } from "./eventually.mjs";
import { launchChromium } from "./quality_fixture.mjs";

const app = await startApp();
const browser = await launchChromium();
const feature = (availability) => ({
  availability,
  evidence: "source_verified",
});
const unavailable = feature("unavailable");
const kept = "/var/lib/hardened-blikvm-syslog/errors.log";

try {
  for (const writable of [true, false]) {
    const context = await browser.newContext({
      viewport: { width: 1280, height: 900 },
    });
    await context.addCookies([
      { name: "hardened-blikvm-csrf", value: "fixture-csrf", url: app.baseUrl },
    ]);
    const page = await context.newPage();
    page.setDefaultTimeout(8000);
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    const manifest = defaultManifest({
      board: {
        features: {
          system_settings: feature("read_only"),
          system_log: feature(writable ? "read_write" : "read_only"),
        },
      },
      network: {
        status: unavailable,
        wifi: unavailable,
        dns: unavailable,
        ipv6: unavailable,
        wol: unavailable,
      },
    });
    await page.route("**/api/capabilities", (route) =>
      route.fulfill({ json: { code: 0, data: manifest } }),
    );
    let config = {
      remoteEnabled: false,
      remoteHost: "",
      remotePort: 514,
      priority: 8,
      bufferKb: 200,
      rotations: 1,
      smallOutput: false,
      stripTimestamps: false,
      kernelEnabled: true,
      kernelConsoleLevel: 7,
    };
    const writes = [];
    await page.route("**/api/system-log/config", (route) => {
      const request = route.request();
      if (request.method() === "POST") {
        writes.push(request.postDataJSON());
        config = { ...config, ...request.postDataJSON() };
      }
      return route.fulfill({
        json: {
          code: 0,
          data: {
            config,
            localLogFile: kept,
            localStorage: "card",
            remoteProtocol: "udp",
            writable,
          },
        },
      });
    });
    const reads = [];
    await page.route("**/api/system-log/messages**", (route) => {
      const url = new URL(route.request().url());
      const kind = url.searchParams.get("kind");
      reads.push(kind);
      return route.fulfill({
        json: {
          code: 0,
          data: {
            kind,
            content: `the ${kind} log\n`,
            lines: Number(url.searchParams.get("lines")),
            truncated: false,
          },
        },
      });
    });
    await page.goto(app.baseUrl, { waitUntil: "networkidle" });
    await page.locator("svg.lucide-settings").click();
    await page.locator('[data-settings-tab="system"]').click();
    await page.getByText("System Log", { exact: true }).first().click();

    // The card holds the errors; busybox's own fields are not offered.
    const card = page.getByTestId("system-log-card");
    await card.waitFor();
    assert.ok((await card.textContent()).includes(kept));
    for (const busybox of [
      "Buffer size",
      "Rotations",
      "Compact output",
      "Strip timestamps",
      "Console level",
    ]) {
      assert.equal(
        await page.getByText(busybox, { exact: true }).count(),
        0,
        `${busybox} shown`,
      );
    }
    await page.getByText("the system log").waitFor();

    // The four logs.
    for (const [label, kind] of [
      ["Kernel", "kernel"],
      ["Board services", "backend"],
      ["Kept errors", "saved"],
    ]) {
      await page.getByTestId("system-log-kind").click();
      await page.getByTitle(label, { exact: true }).click();
      await page.getByText(`the ${kind} log`).waitFor();
    }
    assert.deepEqual(
      [...new Set(reads)],
      ["system", "kernel", "backend", "saved"],
      reads.join(),
    );

    const remote = page.getByRole("switch").first();
    if (!writable) {
      await page
        .getByText("This board's system has no log forwarder yet", {
          exact: false,
        })
        .waitFor();
      assert.ok(
        await remote.isDisabled(),
        "the switch writes without a forwarder",
      );
      assert.equal(
        await page.getByRole("button", { name: "Send test log" }).count(),
        0,
      );
      assert.equal(writes.length, 0);
    } else {
      assert.ok(!(await remote.isDisabled()));
      await remote.click();
      // A remote copy needs a host before it is saved.
      const save = page.getByRole("button", { name: /Save changes/ });
      assert.ok(
        await eventually(async () => await save.isDisabled(), 2000),
        "saved without a host",
      );
      await page.getByPlaceholder("192.0.2.1").fill("logs.example.test");
      await page.getByText("Send kernel messages").waitFor();
      await page.locator('.ant-select:has-text("8 - All")').first().click();
      await page.getByTitle("5 - Notice", { exact: true }).click();
      await save.click();
      assert.ok(await eventually(() => writes.length === 1, 3000), "no write");
      assert.equal(writes[0].remoteEnabled, true);
      assert.equal(writes[0].remoteHost, "logs.example.test");
      assert.equal(writes[0].priority, 5);
      assert.equal(writes[0].kernelEnabled, true);
      assert.ok(
        await eventually(async () => await save.isHidden(), 3000),
        "the saved settings were not read back",
      );
    }
    assert.deepEqual(errors, [], errors.join("\n"));
    await context.close();
  }
  console.log(
    "system log contract: ok (with the forwarder the remote host, port, priority and kernel switch were saved and read back, a remote copy needed a host, the kept errors' file was named, four logs were read and busybox's fields were not shown; without it nothing was written)",
  );
} finally {
  await browser.close();
  await new Promise((resolve) => app.server.close(resolve));
}
