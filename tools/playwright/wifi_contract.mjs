// The Wi-Fi block in Settings > System > Network on a mocked API: networks in
// range with signal and security, joining with a password (a refused attempt
// keeps the dialog), an open network, a hidden one, saved networks with
// auto-join and Forget, Disconnect, Refresh and the radio switch; read-only
// changes nothing; desktop and mobile.
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
const HOME = "0123abcd-0123-4567-89ab-0123456789ab";
const PASSWORD = "fixture-only-password";

function board() {
  const state = {
    radio: true,
    ssid: "HomeNet",
    refuseNext: true,
    profiles: [{ id: HOME, ssid: "HomeNet", autoconnect: true, active: true }],
  };
  const networks = () =>
    [
      { ssid: "Corp", signal: 90, security: "enterprise", channel: 1 },
      { ssid: "HomeNet", signal: 81, security: "wpa2", channel: 36 },
      { ssid: "Office", signal: 70, security: "wpa3", channel: 1 },
      { ssid: "Cafe", signal: 55, security: "open", channel: 11 },
    ].map((network) => ({ ...network, inUse: network.ssid === state.ssid }));
  const join = (ssid) => {
    state.ssid = ssid;
    state.profiles = state.profiles
      .filter((profile) => profile.ssid !== ssid)
      .map((profile) => ({ ...profile, active: false }));
    state.profiles.push({
      id: `${ssid.length.toString(16).padStart(8, "0")}-0123-4567-89ab-0123456789ab`,
      ssid,
      autoconnect: true,
      active: true,
    });
  };
  const activate = (ssid) => {
    state.ssid = ssid;
    state.profiles = state.profiles.map((p) => ({
      ...p,
      active: p.ssid === ssid,
    }));
  };
  return { state, networks, join, activate };
}

try {
  for (const [writable, mobile] of [
    [false, false],
    [true, false],
    [true, true],
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
    page.setDefaultTimeout(8000);
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    const access = feature(writable ? "read_write" : "read_only");
    const unavailable = feature("unavailable");
    const manifest = defaultManifest({
      board: {
        features: {
          system_settings: feature("read_only"),
          network_wifi: access,
        },
      },
      network: {
        status: feature("read_only"),
        dns: unavailable,
        ipv6: unavailable,
        wol: unavailable,
      },
    });
    await page.route("**/api/capabilities", (route) =>
      route.fulfill({ json: { code: 0, data: manifest } }),
    );
    const { state, networks, join, activate } = board();
    const writes = [];
    const reads = [];
    await page.route("**/api/network/wifi**", (route) => {
      const request = route.request();
      const url = new URL(request.url());
      const ok = (data) => route.fulfill({ json: { code: 0, data } });
      if (request.method() === "GET") {
        reads.push(url.pathname + url.search);
        if (url.pathname.endsWith("/scan"))
          return ok({ networks: state.radio ? networks() : [] });
        if (url.pathname.endsWith("/saved"))
          return ok({ profiles: state.profiles });
        const current = networks().find((network) => network.inUse);
        return ok({
          available: true,
          connected: state.radio && Boolean(current),
          ssid: state.radio && current ? current.ssid : "",
          interface: "wlan0",
          state: state.radio && current ? "connected" : "disconnected",
          radio: state.radio,
          signal: current ? current.signal : null,
          security: current ? current.security : null,
          address: current ? "192.0.2.40/24" : null,
        });
      }
      const body = request.postDataJSON();
      writes.push({ path: url.pathname, body });
      if (url.pathname.endsWith("/connect")) {
        if (body.ssid === "Office" && state.refuseNext) {
          state.refuseNext = false;
          return ok({ applied: false, rejection: "backend_rejected" });
        }
        join(body.ssid);
      } else if (url.pathname.endsWith("/disconnect")) {
        state.ssid = "";
        state.profiles = state.profiles.map((p) => ({ ...p, active: false }));
      } else if (url.pathname.endsWith("/radio")) {
        state.radio = body.enabled;
      } else if (url.pathname.endsWith("/saved")) {
        const profile = state.profiles.find((p) => p.id === body.id);
        if (body.action === "forget")
          state.profiles = state.profiles.filter((p) => p.id !== body.id);
        else if (body.action === "connect") activate(profile.ssid);
        else profile.autoconnect = body.action === "autoconnect_on";
      }
      return ok({ applied: true });
    });

    await page.goto(app.baseUrl, { waitUntil: "networkidle" });
    await page.locator("svg.lucide-settings").click();
    await page.locator('[data-settings-tab="system"]').click();
    const block = page.getByTestId("wifi-settings");
    await block.waitFor();
    const rows = block.getByTestId("wifi-network");
    await rows.first().waitFor();
    assert.deepEqual(
      (await rows.allInnerTexts()).map((row) => row.replace(/\s+/g, "")),
      ["CorpEnterprise90%", "HomeNetConnected81%", "Office70%", "Cafe55%"],
    );
    await block.getByText("Connected · 192.0.2.40/24 · WPA2").waitFor();
    assert.equal(await block.getByTestId("wifi-profile").count(), 1);
    const radio = block.getByRole("switch", { name: "Wi-Fi radio" });
    assert.equal(await radio.isDisabled(), !writable);
    assert.equal(await rows.nth(2).isDisabled(), !writable);
    assert.equal(writes.length, 0, "opening the section caused a write");
    if (!writable) {
      assert.deepEqual(errors, []);
      await context.close();
      continue;
    }

    // Enterprise networks are not offered.
    await rows.nth(0).click();
    await block.getByText("security the board cannot join").waitFor();
    await block.locator(".ant-alert-close-icon").click();

    // A refused password keeps the dialog; the second attempt joins.
    await rows.nth(2).click();
    const dialog = page.getByRole("dialog", {
      name: /^(Join Wi-Fi|Other network…)$/,
    });
    const joinButton = dialog.getByRole("button", {
      name: "Join",
      exact: true,
    });
    const secret = dialog.getByPlaceholder("Password", { exact: true });
    await secret.fill("short");
    assert.equal(await joinButton.isDisabled(), true);
    await secret.fill(PASSWORD);
    await joinButton.click();
    await block.getByText("Check the password").waitFor();
    assert.equal(await dialog.isVisible(), true);
    await joinButton.click();
    await dialog.waitFor({ state: "hidden" });
    await block.getByText("Connected · 192.0.2.40/24 · WPA3").waitFor();
    assert.deepEqual(writes.slice(0, 2), [
      {
        path: "/api/network/wifi/connect",
        body: { ssid: "Office", password: PASSWORD, hidden: false },
      },
      {
        path: "/api/network/wifi/connect",
        body: { ssid: "Office", password: PASSWORD, hidden: false },
      },
    ]);

    // An open network asks for no password.
    await rows.nth(3).click();
    await dialog.getByText("This network is open").waitFor();
    assert.equal(await dialog.getByPlaceholder("Password").count(), 0);
    await joinButton.click();
    await dialog.waitFor({ state: "hidden" });
    assert.deepEqual(writes.at(-1).body, {
      ssid: "Cafe",
      password: "",
      hidden: false,
    });

    // A saved network rejoins without a dialog.
    await rows.filter({ hasText: "HomeNet" }).click();
    await block.getByText("Connected · 192.0.2.40/24 · WPA2").waitFor();
    assert.deepEqual(writes.at(-1), {
      path: "/api/network/wifi/saved",
      body: { id: HOME, action: "connect" },
    });

    // Disconnect.
    await block.getByRole("button", { name: "Disconnect" }).click();
    await page
      .locator(".ant-popover")
      .getByRole("button", { name: "Ok" })
      .click();
    await block.getByText("Not connected").waitFor();
    assert.equal(writes.at(-1).path, "/api/network/wifi/disconnect");
    // Saved networks: auto-join off, then Forget.
    const home = block
      .getByTestId("wifi-profile")
      .filter({ hasText: "HomeNet" });
    await home.getByRole("switch").click();
    await page.waitForFunction(
      () =>
        [...document.querySelectorAll('[data-testid="wifi-profile"]')]
          .find((row) => row.textContent.includes("HomeNet"))
          ?.querySelector('[role="switch"]')
          ?.getAttribute("aria-checked") === "false",
    );
    assert.deepEqual(writes.at(-1).body, {
      id: HOME,
      action: "autoconnect_off",
    });
    await home.getByRole("button", { name: "Forget" }).click();
    await page
      .locator(".ant-popover")
      .getByRole("button", { name: "Ok" })
      .click();
    await home.waitFor({ state: "detached" });
    assert.deepEqual(writes.at(-1).body, { id: HOME, action: "forget" });

    // A hidden network by name.
    await block.getByRole("button", { name: "Other network…" }).click();
    await dialog.getByPlaceholder("Name", { exact: true }).fill("Hidden Lab");
    await dialog.getByPlaceholder("Password (optional)").fill(PASSWORD);
    await joinButton.click();
    await dialog.waitFor({ state: "hidden" });
    assert.deepEqual(writes.at(-1).body, {
      ssid: "Hidden Lab",
      password: PASSWORD,
      hidden: true,
    });

    // Refresh and the radio.
    const rescan = page.waitForRequest((request) =>
      request.url().includes("/api/network/wifi/scan?rescan=true"),
    );
    await block.getByRole("button", { name: "Refresh" }).click();
    await rescan;
    await radio.click();
    await block
      .getByText("Turn Wi-Fi on to see the networks in range.")
      .waitFor();
    assert.deepEqual(writes.at(-1), {
      path: "/api/network/wifi/radio",
      body: { enabled: false },
    });
    assert.equal(await rows.count(), 0);

    // The password only ever travels in a request body.
    assert.ok(reads.every((read) => !read.includes(PASSWORD)));
    assert.deepEqual(errors, []);
    await context.close();
  }
  console.log(
    "Wi-Fi: networks, join with refusal, open, saved rejoin, hidden, auto-join, Forget, Disconnect, Refresh, radio and read-only passed on desktop and mobile",
  );
} finally {
  await browser.close();
  await new Promise((resolve) => app.server.close(resolve));
}
