import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";
import { defaultManifest, startApp } from "./app_harness.mjs";

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
const app = await startApp();
const feature = (availability) => ({
  availability,
  evidence: "source_verified",
});
try {
  for (const { writable, applied, mobile } of [
    { writable: false, applied: false, mobile: false },
    { writable: true, applied: false, mobile: false },
    { writable: true, applied: true, mobile: false },
    { writable: true, applied: true, mobile: true },
  ]) {
    const context = await browser.newContext(
      mobile
        ? {
            viewport: { width: 390, height: 844 },
            isMobile: true,
            hasTouch: true,
          }
        : {},
    );
    await context.addCookies([
      { name: "hardened-blikvm-csrf", value: "fixture-csrf", url: app.baseUrl },
    ]);
    const page = await context.newPage();
    await page.addInitScript(() => {
      const original = window.setTimeout.bind(window);
      window.__tlsRedirectTimers = 0;
      window.setTimeout = (callback, delay, ...args) => {
        if (delay === 30000) window.__tlsRedirectTimers++;
        return original(callback, delay, ...args);
      };
    });
    page.setDefaultTimeout(5000);
    const errors = [];
    const requests = [];
    page.on("pageerror", (error) => errors.push(error.message));
    page.on("console", (message) => {
      if (
        message.type() === "error" &&
        /ErrorBoundary|during render/.test(message.text())
      )
        errors.push(message.text());
    });
    page.on("request", (request) => {
      if (new URL(request.url()).pathname.startsWith("/api/"))
        requests.push(request);
    });
    const access = feature(writable ? "read_write" : "read_only");
    const manifest = defaultManifest({
      board: {
        features: {
          device_settings: feature("read_only"),
          device_mdns: access,
          device_hid_only: access,
          device_mouse_jiggler: access,
          device_session_lock: access,
          network_tls: access,
          network_wifi: access,
          tailscale: feature("read_only"),
          system_firewall: access,
          system_settings: feature("read_only"),
        },
      },
      network: {
        status: access,
        dns: access,
        ipv6: access,
        wol: feature("unavailable"),
      },
    });
    await page.route("**/api/capabilities", (route) =>
      route.fulfill({ json: { code: 0, data: manifest } }),
    );
    let mdnsEnabled = false;
    let firewallActive = false;
    await page.route("**/api/system/firewall", (route) => {
      if (route.request().method() === "POST") {
        if (applied)
          firewallActive = route.request().postDataJSON().mode === "baseline";
        return route.fulfill({
          json: {
            code: 0,
            data: applied
              ? { applied: true }
              : { applied: false, rejection: "firewall_refused" },
          },
        });
      }
      return route.fulfill({
        json: {
          code: 0,
          data: {
            backendAvailable: true,
            baselineActive: firewallActive,
            mode: firewallActive ? "baseline" : "disabled",
            message: "",
            nftInstalled: true,
            nftDetail: "fixture nft",
            iptablesInstalled: false,
            ip6tablesInstalled: false,
            rulesNft: "table inet fixture {}",
          },
        },
      });
    });
    await page.route("**/api/extensions/tailscale/status", (route) =>
      route.fulfill({
        json: {
          code: 0,
          data: { state: "notInstall", ip: "", name: "", account: "" },
        },
      }),
    );
    await page.route("**/api/vm/tls", (route) =>
      route.fulfill({
        json: {
          code: 0,
          data:
            route.request().method() === "GET"
              ? { enabled: true }
              : applied
                ? { applied: true }
                : { applied: false, rejection: "tls_refused" },
        },
      }),
    );
    let lockDuration = 900;
    await page.route("**/api/vm/session-lock", (route) => {
      if (route.request().method() === "POST" && applied)
        lockDuration = route.request().postDataJSON().duration;
      return route.fulfill({
        json: { code: 0, data: { duration: lockDuration } },
      });
    });
    let jigglerEnabled = false;
    await page.route("**/api/vm/mouse-jiggler", (route) => {
      if (route.request().method() === "GET")
        return route.fulfill({
          json: {
            code: 0,
            data: { enabled: jigglerEnabled, mode: "relative" },
          },
        });
      if (applied) jigglerEnabled = route.request().postDataJSON().enabled;
      return route.fulfill({
        json: {
          code: 0,
          data: applied
            ? { applied: true }
            : { applied: false, rejection: "jiggler_refused" },
        },
      });
    });
    let hidOnlyEnabled = false;
    await page.route("**/api/vm/hid-only", (route) => {
      if (route.request().method() === "GET")
        return route.fulfill({
          json: { code: 0, data: { enabled: hidOnlyEnabled } },
        });
      if (applied) hidOnlyEnabled = route.request().postDataJSON().enabled;
      return route.fulfill({
        json: {
          code: 0,
          data: applied
            ? { applied: true }
            : { applied: false, rejection: "hid_only_refused" },
        },
      });
    });
    await page.route("**/api/vm/mdns**", (route) => {
      if (route.request().method() === "GET")
        return route.fulfill({
          json: { code: 0, data: { enabled: mdnsEnabled } },
        });
      if (applied) mdnsEnabled = route.request().url().endsWith("/enable");
      return route.fulfill({
        json: {
          code: 0,
          data: applied
            ? { applied: true }
            : { applied: false, rejection: "mdns_refused" },
        },
      });
    });
    await page.route("**/api/inventory", (route) =>
      route.fulfill({
        json: {
          code: 0,
          data: {
            schemaVersion: 3,
            backend: "native",
            versions: {},
            video: { online: false },
            thermal: {},
            rtc: {},
          },
        },
      }),
    );
    let servers = ["1.1.1.1"];
    let ipv6Mode = "slaac";
    await page.route("**/api/network/**", (route) => {
      const request = route.request();
      // Wi-Fi has its own contract (wifi_contract.mjs); here the board has
      // no Wi-Fi device and the section stays hidden.
      if (new URL(request.url()).pathname.startsWith("/api/network/wifi"))
        return route.fulfill({
          json: {
            code: 0,
            data: {
              available: false,
              connected: false,
              ssid: "",
              state: "unavailable",
              radio: false,
              signal: null,
              security: null,
              address: null,
            },
          },
        });
      const dnsState = () => ({
        mode: "manual",
        servers,
        effective: servers,
        dhcp: [],
        info: {
          interface: "enp1s0",
          type: "Ethernet",
          address: "192.0.2.10",
          subnetMask: "255.255.255.0",
          gateway: "192.0.2.1",
          searchDomains: [],
        },
        config: {
          address: "192.0.2.10",
          subnetMask: "255.255.255.0",
          gateway: "192.0.2.1",
        },
        pending: 0,
      });
      if (request.method() === "POST" && request.url().endsWith("/dns")) {
        if (!applied)
          return route.fulfill({
            json: { code: -1, msg: "fixture_refused", data: null },
          });
        servers = request.postDataJSON().servers;
        return route.fulfill({ json: { code: 0, data: dnsState() } });
      }
      const ipv6State = () => ({
        interface: "enp1s0",
        mode: ipv6Mode,
        enabled: ipv6Mode !== "disabled",
        active: false,
        clientAvailable: true,
        clientPath: "NetworkManager",
        addresses: [],
        gateway: "",
        status: "",
        message: "",
      });
      if (request.method() === "POST" && request.url().endsWith("/ipv6")) {
        if (!applied)
          return route.fulfill({
            json: { code: -1, msg: "fixture_refused", data: null },
          });
        ipv6Mode = request.postDataJSON().mode;
        return route.fulfill({ json: { code: 0, data: ipv6State() } });
      }
      const data = request.url().endsWith("/dns") ? dnsState() : ipv6State();
      return route.fulfill({ json: { code: 0, data } });
    });
    await page.goto(app.baseUrl, { waitUntil: "networkidle" });
    await page.locator("svg.lucide-settings").click();
    await page.getByText("native", { exact: true }).waitFor();
    assert.equal(await page.getByText("KVMD", { exact: true }).count(), 0);
    await page.locator('[data-settings-tab="device"]').click();
    await page.locator('[data-settings-tab="system"]').click();
    await page.getByText("enp1s0", { exact: true }).waitFor();
    const input = page.locator('input[value="1.1.1.1"]');
    await input.waitFor();
    assert.equal(await input.isDisabled(), !writable);
    assert.equal(
      requests.some((request) => request.method() !== "GET"),
      false,
      "opening settings caused a write",
    );
    for (const request of requests) {
      const route = new URL(request.url()).pathname;
      assert.equal(
        /\/api\/hid\/shortcut|\/api\/network\/tls|\/api\/.*(oled|ssh|swap|virtual-device)/.test(
          route,
        ),
        false,
        `unsupported request: ${route}`,
      );
    }
    if (writable) {
      await input.fill("9.9.9.9");
      await page
        .getByRole("button", { name: "Save changes", exact: true })
        .click();
      await page
        .getByText(applied ? "DNS settings saved" : "fixture_refused", {
          exact: true,
        })
        .waitFor();
      const writes = requests.filter((request) => request.method() === "POST");
      assert.equal(writes.length, 1);
      // The reference body; the address is unchanged, so no redirect.
      assert.deepEqual(writes[0].postDataJSON(), {
        mode: "manual",
        servers: ["9.9.9.9"],
        interface: "enp1s0",
        address: "192.0.2.10",
        subnetMask: "255.255.255.0",
        gateway: "192.0.2.1",
      });
      assert.equal(
        await page.getByText("DNS settings saved", { exact: true }).count(),
        applied ? 1 : 0,
      );
      if (!applied)
        await page.locator('input[value="9.9.9.9"]').fill("1.1.1.1");
      await page.getByText("Disabled", { exact: true }).click();
      const refused = page.waitForResponse(
        (response) =>
          response.url().endsWith("/api/network/ipv6") &&
          response.request().method() === "POST",
      );
      await page
        .getByRole("button", { name: "Save changes", exact: true })
        .click();
      await refused;
      await page
        .getByText(applied ? "IPv6 settings saved" : "fixture_refused")
        .waitFor();
      const ipv6Writes = requests.filter(
        (request) =>
          request.method() === "POST" && request.url().endsWith("/ipv6"),
      );
      assert.equal(ipv6Writes.length, 1);
      assert.deepEqual(ipv6Writes[0].postDataJSON(), { mode: "disabled" });
      assert.equal(
        await page.getByText("IPv6 settings saved").count(),
        applied ? 1 : 0,
      );
      assert.equal(
        requests.filter((request) => request.method() === "POST").length,
        2,
      );
    }
    const tls = page.getByRole("switch", { name: "HTTPS", exact: true });
    assert.equal(await tls.isDisabled(), !writable);
    // A read-only HTTPS row says why (it is always on), a writable one does not.
    assert.equal(
      await page.getByTestId("tls-always-on").count(),
      writable ? 0 : 1,
    );
    assert.equal(
      await tls.getAttribute("aria-checked"),
      "true",
      "TLS must use API state rather than browser protocol",
    );
    const timersBefore = await page.evaluate(() => window.__tlsRedirectTimers);
    if (writable) {
      await tls.click();
      await page
        .getByRole("button", { name: "Apply and reboot", exact: true })
        .click();
      if (applied)
        await page.waitForFunction(
          () =>
            document
              .querySelector('[aria-label="HTTPS"]')
              ?.getAttribute("aria-checked") === "false",
        );
      else
        await page
          .getByRole("alert")
          .filter({ hasText: "tls_refused" })
          .waitFor();
      assert.equal(
        await page.evaluate(() => window.__tlsRedirectTimers),
        timersBefore + (applied ? 1 : 0),
      );
    }
    const mdnsRead = page.waitForResponse((response) =>
      response.url().endsWith("/vm/mdns"),
    );
    await page.locator('[data-settings-tab="device"]').click();
    await mdnsRead;
    const mdns = page.getByRole("switch", { name: "mDNS", exact: true });
    await mdns.waitFor();
    assert.equal(await mdns.isDisabled(), !writable);
    if (writable) {
      await mdns.click();
      if (applied)
        await page.waitForFunction(
          () =>
            document
              .querySelector('[aria-label="mDNS"]')
              ?.getAttribute("aria-checked") === "true",
        );
      else
        await page
          .getByRole("alert")
          .filter({ hasText: "mdns_refused" })
          .waitFor();
      assert.equal(
        await mdns.getAttribute("aria-checked"),
        applied ? "true" : "false",
      );
    }
    const hidOnly = page.getByRole("switch", { name: "HID-only", exact: true });
    assert.equal(await hidOnly.isDisabled(), !writable);
    if (writable) {
      await hidOnly.click();
      if (applied)
        await page.waitForFunction(
          () =>
            document
              .querySelector('[aria-label="HID-only"]')
              ?.getAttribute("aria-checked") === "true",
        );
      else
        await page
          .getByRole("alert")
          .filter({ hasText: "hid_only_refused" })
          .waitFor();
      assert.equal(
        await hidOnly.getAttribute("aria-checked"),
        applied ? "true" : "false",
      );
      const writes = requests.filter(
        (request) =>
          request.method() === "POST" && request.url().endsWith("/hid-only"),
      );
      assert.equal(writes.length, 1);
      assert.deepEqual(writes[0].postDataJSON(), { enabled: true });
    }
    const jiggler = page.getByRole("switch", {
      name: "Mouse jiggler",
      exact: true,
    });
    assert.equal(await jiggler.isDisabled(), !writable);
    if (writable) {
      await jiggler.click();
      if (applied)
        await page
          .getByRole("combobox", { name: "Mouse jiggler mode", exact: true })
          .waitFor();
      else {
        await page
          .getByRole("alert")
          .filter({ hasText: "jiggler_refused" })
          .waitFor();
        assert.equal(await jiggler.getAttribute("aria-checked"), "false");
      }
      const writes = requests.filter(
        (request) =>
          request.method() === "POST" &&
          request.url().endsWith("/mouse-jiggler"),
      );
      assert.equal(writes.length, 1);
      assert.deepEqual(writes[0].postDataJSON(), {
        enabled: true,
        mode: "relative",
      });
    }
    const sessionLock = page.getByRole("combobox", {
      name: "Session lock duration",
      exact: true,
    });
    assert.equal(await sessionLock.isDisabled(), !writable);
    if (writable) {
      await page
        .locator(".ant-select-selection-item")
        .filter({ hasText: /^15 min$/ })
        .click();
      await page
        .locator(".ant-select-item-option")
        .filter({ hasText: /^5 min$/ })
        .click();
      if (applied)
        await page
          .locator(".ant-select-selection-item")
          .filter({ hasText: /^5 min$/ })
          .waitFor();
      else
        await page
          .getByRole("alert")
          .filter({ hasText: "Session lock read-back did not match" })
          .waitFor();
      const writes = requests.filter(
        (request) =>
          request.method() === "POST" &&
          request.url().endsWith("/session-lock"),
      );
      assert.equal(writes.length, 1);
      assert.deepEqual(writes[0].postDataJSON(), { duration: 300 });
    }
    const beforeTailscale = requests.length;
    await page.locator('[data-settings-tab="tailscale"]').click();
    await page
      .getByText("Tailscale not found! Please install.", { exact: true })
      .waitFor();
    assert.deepEqual(
      requests
        .slice(beforeTailscale)
        .filter((request) => request.url().includes("/tailscale/"))
        .map(
          (request) => `${request.method()} ${new URL(request.url()).pathname}`,
        ),
      ["GET /api/extensions/tailscale/status"],
    );
    // The reference view: installing is offered, nothing else yet.
    assert.equal(
      await page.getByRole("button", { name: "Install", exact: true }).count(),
      1,
    );
    assert.equal(
      await page
        .getByRole("button", {
          name: /Start|Login|Logout|Uninstall/,
          exact: true,
        })
        .count(),
      0,
    );
    await page.locator('[data-settings-tab="system"]').click();
    await page
      .locator(".ant-segmented-item-label")
      .filter({ hasText: /^Firewall$/ })
      .click();
    await page.getByText("Managed rules: inactive", { exact: true }).waitFor();
    const enableFirewall = page.getByRole("button", {
      name: "Enable managed rules",
      exact: true,
    });
    assert.equal(await enableFirewall.isDisabled(), !writable);
    assert.equal(
      requests.filter(
        (request) =>
          request.method() === "POST" &&
          request.url().endsWith("/system/firewall"),
      ).length,
      0,
    );
    if (writable) {
      await enableFirewall.click();
      await page.getByRole("button", { name: "Apply", exact: true }).click();
      await page
        .getByText(applied ? "Managed rules: active" : "firewall_refused", {
          exact: true,
        })
        .waitFor();
      const writes = requests.filter(
        (request) =>
          request.method() === "POST" &&
          request.url().endsWith("/system/firewall"),
      );
      assert.equal(writes.length, 1);
      assert.deepEqual(writes[0].postDataJSON(), { mode: "baseline" });
      if (!applied)
        assert.equal(
          await page
            .getByText("Managed rules: inactive", { exact: true })
            .count(),
          1,
        );
    }
    assert.deepEqual(errors, []);
    await context.close();
  }
  console.log(
    "Network contract: read-only, page-open, unsupported requests, DNS/IPv6 refusal and desktop/mobile successful writes passed",
  );
} finally {
  await browser.close();
  await new Promise((resolve) => app.server.close(resolve));
}
