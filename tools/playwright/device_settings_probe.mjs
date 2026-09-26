// Verify the plan phase B settings on the device through the API: About
// addresses and image version, the Diagnostics component states, the SSH
// switch (off closes port 22, on opens it again), IPv4 manual/DHCP with the
// revert confirmation (keeping the current address), the session lock
// duration, USB gadget reconnect, frame detect and the Tailscale view. Every
// setting is restored at the end. The GUI password is read from stdin; a
// receipt is written under evidence/private.
import assert from "node:assert/strict";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { pathToFileURL } from "node:url";

const [baseUrl, username, outputValue] = process.argv.slice(2);
const url = new URL(baseUrl);
assert.equal(url.protocol, "https:");
const output = path.resolve(outputValue);
assert.equal(path.dirname(output), path.resolve("evidence/private"));
assert.ok(!fs.existsSync(output));
let password = "";
for await (const chunk of process.stdin) password += chunk.toString("utf8");
password = password.replace(/[\r\n]+$/, "");
assert.ok(password.length >= 8);

const { request } = await import(
  pathToFileURL(
    process.env.PLAYWRIGHT_MODULE ??
      "/home/w0w/.local/share/playwright/node_modules/playwright/index.mjs",
  ).href
);
const context = await request.newContext({ ignoreHTTPSErrors: true });
const receipt = { schemaVersion: 1, probe: "device-settings", steps: [] };
const step = (name, value) => receipt.steps.push({ step: name, ...value });
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const port22 = () =>
  new Promise((resolve) => {
    const socket = net.connect({ host: url.hostname, port: 22 });
    const done = (open) => {
      socket.destroy();
      resolve(open);
    };
    socket.setTimeout(3000, () => done(false));
    socket.on("connect", () => done(true));
    socket.on("error", () => done(false));
  });
try {
  const login = await context.post(`${url.origin}/api/auth/login`, {
    data: { username, password },
    headers: { Origin: url.origin },
  });
  password = "";
  const csrf = (await login.json()).data.csrfToken;
  const headers = { Origin: url.origin, "X-CSRF-Token": csrf };
  const api = async (method, route, data) => {
    const response = await context.fetch(`${url.origin}${route}`, {
      method,
      data,
      headers,
      timeout: 120000,
    });
    let body = null;
    try {
      body = await response.json();
    } catch {
      body = null;
    }
    return {
      http: response.status(),
      code: body?.code,
      msg: body?.msg,
      data: body?.data,
    };
  };

  // About and Diagnostics.
  const info = await api("GET", "/api/vm/info");
  step("about", {
    http: info.http,
    ips: info.data?.ips,
    image: info.data?.image,
  });
  const diagnostics = await api("GET", "/api/diagnostics");
  step("diagnostics", {
    http: diagnostics.http,
    components: diagnostics.data?.current?.components,
  });

  // SSH switch.
  step("ssh_initial", {
    ...(await api("GET", "/api/vm/ssh")),
    port22: await port22(),
  });
  step("ssh_off", await api("POST", "/api/vm/ssh/disable"));
  await sleep(1500);
  step("ssh_after_off", { port22: await port22() });
  step("ssh_on", await api("POST", "/api/vm/ssh/enable"));
  await sleep(1500);
  step("ssh_after_on", { port22: await port22() });

  // IPv4: manual with the current address, confirm, back to DHCP, confirm.
  const dns = await api("GET", "/api/network/dns");
  step("dns_initial", { http: dns.http, data: dns.data });
  const current = dns.data?.info ?? {};
  const manual = await api("POST", "/api/network/dns", {
    mode: "manual",
    servers: dns.data?.dhcp?.length ? dns.data.dhcp : dns.data?.effective,
    interface: current.interface,
    address: current.address,
    subnetMask: current.subnetMask,
    gateway: current.gateway,
  });
  step("dns_manual", {
    http: manual.http,
    code: manual.code,
    msg: manual.msg,
    mode: manual.data?.mode,
    pending: manual.data?.pending,
  });
  await sleep(3000);
  step("dns_manual_confirm", await api("POST", "/api/network/dns/confirm"));
  const afterManual = await api("GET", "/api/network/dns");
  step("dns_after_manual", {
    mode: afterManual.data?.mode,
    pending: afterManual.data?.pending,
    config: afterManual.data?.config,
  });
  const dhcp = await api("POST", "/api/network/dns", {
    mode: "dhcp",
    servers: [],
  });
  step("dns_dhcp", {
    code: dhcp.code,
    msg: dhcp.msg,
    mode: dhcp.data?.mode,
    pending: dhcp.data?.pending,
  });
  await sleep(5000);
  step("dns_dhcp_confirm", await api("POST", "/api/network/dns/confirm"));
  const afterDhcp = await api("GET", "/api/network/dns");
  step("dns_after_dhcp", {
    mode: afterDhcp.data?.mode,
    pending: afterDhcp.data?.pending,
    address: afterDhcp.data?.info?.address,
  });

  // Session lock: set, read back, restore.
  const lock = await api("GET", "/api/vm/session-lock");
  step(
    "lock_set",
    await api("POST", "/api/vm/session-lock", { duration: 900 }),
  );
  step("lock_read", await api("GET", "/api/vm/session-lock"));
  step(
    "lock_restore",
    await api("POST", "/api/vm/session-lock", {
      duration: lock.data?.duration ?? 3600,
    }),
  );

  // USB gadget reconnect, then the controller is enumerated again.
  step("usb_reconnect", await api("POST", "/api/storage/usb/reconnect"));
  await sleep(4000);
  const afterReconnect = await api("GET", "/api/diagnostics");
  step("usb_after_reconnect", {
    hid: afterReconnect.data?.current?.components?.hid,
  });

  // Frame detect off and on again.
  step(
    "frame_detect_off",
    await api("POST", "/api/stream/mjpeg/detect", { enabled: false }),
  );
  step(
    "frame_detect_stop",
    await api("POST", "/api/stream/mjpeg/detect/stop", { duration: 10 }),
  );
  step(
    "frame_detect_on",
    await api("POST", "/api/stream/mjpeg/detect", { enabled: true }),
  );

  // Tailscale view.
  step("tailscale", await api("GET", "/api/extensions/tailscale/status"));

  const by = Object.fromEntries(
    receipt.steps.map((entry) => [entry.step, entry]),
  );
  const components = by.diagnostics.components ?? {};
  receipt.result =
    by.about.ips?.some((ip) => ip.addr === url.hostname) &&
    Boolean(by.about.image) &&
    components.hid?.online === "active" &&
    components.power?.online === "active" &&
    components.virtualMedia?.online === "active" &&
    by.ssh_initial.data?.enabled === true &&
    by.ssh_initial.port22 === true &&
    by.ssh_off.data?.enabled === false &&
    by.ssh_after_off.port22 === false &&
    by.ssh_on.data?.enabled === true &&
    by.ssh_after_on.port22 === true &&
    by.dns_initial.data?.info?.address === url.hostname &&
    by.dns_manual.mode === "manual" &&
    by.dns_manual.pending > 0 &&
    by.dns_manual_confirm.code === 0 &&
    by.dns_after_manual.pending === 0 &&
    by.dns_after_manual.config?.address === url.hostname &&
    by.dns_dhcp.mode === "dhcp" &&
    by.dns_dhcp_confirm.code === 0 &&
    by.dns_after_dhcp.mode === "dhcp" &&
    by.dns_after_dhcp.address === url.hostname &&
    by.lock_read.data?.duration === 900 &&
    by.lock_restore.code === 0 &&
    by.usb_reconnect.data?.applied === true &&
    by.usb_after_reconnect.hid?.connected === "active" &&
    by.frame_detect_off.code === 0 &&
    by.frame_detect_stop.code === 0 &&
    by.frame_detect_on.code === 0 &&
    typeof by.tailscale.data?.state === "string"
      ? "passed"
      : "failed";
} finally {
  await context.dispose();
  fs.writeFileSync(output, JSON.stringify(receipt, null, 1), { mode: 0o600 });
  console.log(JSON.stringify(receipt));
}
