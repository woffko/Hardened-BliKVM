// Device probe of the four firewall modes (docs/design/firewall-reference-contract.md).
// For each mode it applies the mode through the API and checks, from this
// host over IPv4 and (through the Windows host) IPv6, HTTPS and SSH, and on
// the board through one web-terminal session the owned table's DHCP and ICMPv6
// rules, the IPv6 router, the Tailscale control server and, through
// vp8_webrtc_probe.mjs, WebRTC media. A restrictive mode is confirmed only
// after its checks; `timeout` leaves one unconfirmed and expects the owner to
// roll it back. The terminal is switched off at the end. The GUI password is
// read from stdin; a receipt is written under evidence/private.
//
// Usage: node firewall_modes_device_probe.mjs BASE_URL USERNAME OUTPUT IPV6 STEP...
//   STEP: baseline | moderate | restricted | paranoid | timeout:MODE | status
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { pathToFileURL } from "node:url";

const [baseUrl, username, outputValue, ipv6, ...steps] = process.argv.slice(2);
const url = new URL(baseUrl);
assert.equal(url.protocol, "https:");
const output = path.resolve(outputValue);
assert.equal(path.dirname(output), path.resolve("evidence/private"));
assert.ok(!fs.existsSync(output));
assert.ok(/^[0-9a-f:]+$/i.test(ipv6), "the board's IPv6 address is required");
assert.ok(steps.length > 0);
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
const receipt = { schemaVersion: 1, probe: "firewall-modes-device", steps: [] };
const record = (name, value) => {
  receipt.steps.push({ step: name, at: new Date().toISOString(), ...value });
  console.log(JSON.stringify({ step: name, ...value }).slice(0, 600));
};
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
let api = null;

// Host-side checks.
function tcpBanner(host, port) {
  return new Promise((resolve) => {
    const socket = net.connect({ host, port, timeout: 5000 });
    let data = "";
    socket.on("data", (chunk) => {
      data += chunk.toString();
      socket.destroy();
    });
    socket.on("timeout", () => socket.destroy());
    socket.on("error", () => {});
    socket.on("close", () =>
      resolve(data.startsWith("SSH-") ? "open" : "closed"),
    );
  });
}
function windows(command) {
  const result = spawnSync(
    "powershell.exe",
    ["-NoProfile", "-Command", command],
    {
      encoding: "utf8",
      timeout: 40000,
    },
  );
  return (result.stdout ?? "").replace(/\r/g, "").trim();
}
async function hostChecks() {
  let https4 = "closed";
  try {
    const response = await fetch(`${url.origin}/api/health`, {
      signal: AbortSignal.timeout(6000),
    });
    https4 = response.status === 200 ? "open" : `http_${response.status}`;
  } catch {
    https4 = "closed";
  }
  // The Windows host has an address on the board's IPv6 network; WSL does not.
  const https6 = windows(
    `$code = curl.exe -6 -k -s -o NUL -w '%{http_code}' --max-time 6 'https://[${ipv6}]/api/health'; $code`,
  );
  const ssh6 = windows(
    `$c = New-Object Net.Sockets.TcpClient; try { $t = $c.ConnectAsync('${ipv6}', 22); if ($t.Wait(5000) -and $c.Connected) { 'open' } else { 'closed' } } catch { 'closed' } finally { $c.Close() }`,
  );
  return {
    https4,
    ssh4: await tcpBanner(url.hostname, 22),
    https6: https6 === "200" ? "open" : "closed",
    ssh6,
  };
}

try {
  const context = await browser.newContext({ ignoreHTTPSErrors: true });
  const page = await context.newPage();
  const login = await context.request.post(`${url.origin}/api/auth/login`, {
    data: { username, password },
    headers: { Origin: url.origin },
  });
  const csrf = (await login.json()).data.csrfToken;
  await page.goto(`${url.origin}/`);
  api = async (method, route, data) => {
    const response = await context.request.fetch(`${url.origin}${route}`, {
      method,
      data,
      headers: { Origin: url.origin, "X-CSRF-Token": csrf },
      timeout: 60000,
    });
    const body = await response.json().catch(() => null);
    return {
      http: response.status(),
      code: body?.code,
      msg: body?.msg,
      data: body?.data,
    };
  };

  // One root shell for the whole probe.
  assert.equal(
    (await api("POST", "/api/vm/terminal/enabled", { enabled: true })).data
      ?.enabled,
    true,
  );
  const unlocked = await api("POST", "/api/vm/terminal/unlock", {
    username,
    password,
  });
  const ticket = unlocked.data?.ticket;
  assert.ok(ticket, "the terminal did not unlock");
  await page.evaluate(async (ticket) => {
    await new Promise((resolve, reject) => {
      const socket = new WebSocket(
        `wss://${location.host}/api/vm/terminal?ticket=${encodeURIComponent(ticket)}`,
      );
      socket.binaryType = "arraybuffer";
      window.__text = "";
      socket.onmessage = (event) => {
        window.__text += new TextDecoder().decode(event.data);
      };
      socket.onopen = () => {
        socket.send(new Blob([JSON.stringify({ rows: 50, cols: 250 })]));
        socket.send(
          "stty -echo; export TERM=dumb PAGER=cat SYSTEMD_PAGER=cat\r",
        );
        window.__socket = socket;
        setTimeout(resolve, 1500);
      };
      socket.onerror = () => reject(new Error("terminal socket failed"));
    });
  }, ticket);
  let serial = 0;
  const shell = async (command, timeoutMs = 60000) => {
    serial += 1;
    const tag = `${serial}`;
    const encoded = Buffer.from(command).toString("base64");
    return await page.evaluate(
      async ({ encoded, tag, timeoutMs }) => {
        const start = window.__text.length;
        window.__socket.send(
          `printf 'B%s-%s\\n' ${tag} $((6*7)); echo ${encoded} | base64 -d | sh 2>&1; printf 'E%s-%s\\n' ${tag} $((6*7))\r`,
        );
        const begin = `B${tag}-42`;
        const end = `E${tag}-42`;
        const deadline = Date.now() + timeoutMs;
        while (Date.now() < deadline) {
          const text = window.__text.slice(start);
          const from = text.indexOf(begin);
          const to = text.indexOf(end);
          if (from >= 0 && to > from)
            return text
              .slice(from + begin.length, to)
              .replace(/\r/g, "")
              .trim();
          await new Promise((resolve) => setTimeout(resolve, 200));
        }
        return "TIMEOUT";
      },
      { encoded, tag, timeoutMs },
    );
  };
  // The password stays in memory only for the WebRTC child probe below.

  async function boardChecks() {
    const table = await shell(
      "nft list table inet hardened-blikvm 2>/dev/null || echo NO-TABLE",
    );
    const router = await shell(
      "r=$(ip -6 route show default | awk '{print $3; exit}'); echo router=$r; [ -n \"$r\" ] && ip -6 neigh show $r dev eth0 | awk '{print $NF}'",
    );
    const tailscale = await shell(
      "printf 'control=%s ' $(curl -s -o /dev/null -w '%{http_code}' --max-time 6 https://controlplane.tailscale.com/key?v=71); tailscale status --json 2>/dev/null | sed -n 's/.*\"BackendState\": *\"\\([A-Za-z]*\\)\".*/state=\\1/p' | head -1",
    );
    const legacy = await shell(
      "printf 'xtables=%s nft_filter_drops=%s' $(iptables-legacy-save 2>/dev/null | grep -c '^-A') $(nft list ruleset 2>/dev/null | grep -v 'hardened-blikvm' | grep -c 'drop')",
    );
    return {
      table: table.includes("NO-TABLE")
        ? "absent"
        : (table.match(/hardened-blikvm-owner-v2:([a-z]+)/)?.[1] ?? "unknown"),
      dhcp:
        /udp sport 67 udp dport 68/.test(table) &&
        /udp sport 547 udp dport 546/.test(table),
      icmpv6:
        /nd-neighbor-solicit/.test(table) && /nd-router-advert/.test(table),
      router: router.replace(/\n/g, " "),
      tailscale,
      legacy,
    };
  }
  function webrtc(label) {
    const receiptPath = output.replace(/\.json$/, `-webrtc-${label}.json`);
    const child = spawnSync(
      "node",
      [
        "tools/playwright/vp8_webrtc_probe.mjs",
        baseUrl,
        username,
        receiptPath,
        "10",
      ],
      { input: `${password}\n`, encoding: "utf8", timeout: 120000 },
    );
    try {
      return (
        JSON.parse(fs.readFileSync(receiptPath, "utf8")).result ?? "unknown"
      );
    } catch {
      return `error:${(child.stderr ?? "").slice(-120)}`;
    }
  }
  async function modeStep(mode, confirm) {
    const change = await api("POST", "/api/system/firewall/mode", {
      mode,
      confirm: true,
    });
    record(`change_${mode}`, change);
    await sleep(3000);
    const status = await api("GET", "/api/system/firewall/mode");
    const host = await hostChecks();
    const board = await boardChecks();
    const media = webrtc(mode);
    record(`checks_${mode}`, {
      status: status.data,
      host,
      board,
      webrtc: media,
    });
    if (confirm && status.data?.pendingOperationId) {
      record(
        `confirm_${mode}`,
        await api("POST", "/api/system/firewall/mode/confirm", {
          operationId: status.data.pendingOperationId,
        }),
      );
    }
    record(`final_${mode}`, await api("GET", "/api/system/firewall/mode"));
  }

  record("initial", await api("GET", "/api/system/firewall/mode"));
  for (const step of steps) {
    if (step === "status") {
      record("status", {
        status: (await api("GET", "/api/system/firewall/mode")).data,
        host: await hostChecks(),
        board: await boardChecks(),
      });
    } else if (step.startsWith("timeout:")) {
      const mode = step.slice("timeout:".length);
      const before = await api("GET", "/api/system/firewall/mode");
      record(
        `timeout_change_${mode}`,
        await api("POST", "/api/system/firewall/mode", { mode, confirm: true }),
      );
      record(
        `timeout_pending_${mode}`,
        await api("GET", "/api/system/firewall/mode"),
      );
      await sleep(80000);
      const after = await api("GET", "/api/system/firewall/mode");
      record(`timeout_after_${mode}`, {
        status: after.data,
        rolledBackTo: after.data?.mode,
        expected: before.data?.mode,
        board: await boardChecks(),
      });
    } else {
      await modeStep(step, true);
    }
  }
  receipt.result = "recorded";
} catch (error) {
  record("error", { message: String(error?.message ?? error) });
  receipt.result = "error";
} finally {
  password = "";
  if (api) {
    const off = await api("POST", "/api/vm/terminal/enabled", {
      enabled: false,
    }).catch(() => null);
    record("terminal_off", { enabled: off?.data?.enabled });
  }
  await browser.close();
  fs.writeFileSync(output, JSON.stringify(receipt, null, 1), { mode: 0o600 });
}
