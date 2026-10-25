// How long a keyboard report takes to be answered over the standard path
// and over QUIC, measured in a page against the real web server, the real
// gateway and a stand-in for the board owner (quic_stack_rig.mjs). Reports go
// one after the other, each waiting for its answer, as the page sends keys.
//
// The path is the loopback interface, directly and through the relays of
// tools/net: without impairment, with 50 ms each way, and with 50 ms each way
// and 2% loss. The UDP relay really loses datagrams, so the QUIC numbers
// under loss are measured. The TCP relay cannot lose segments; it stalls a
// direction for one retransmission instead (see tcp_impair_proxy.mjs), so
// the standard path's numbers under loss are that model's.
//
// The standard path here is HTTP/1.1 to the server, without the TLS and the
// nginx that stand before it on a board.
//
// Usage: node quic_input_latency.mjs OUTPUT.json [REPORTS]
//   BLI_KVM_STACK_PROFILE=target/release uses the optimized builds.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

import { startTcpProxy } from "../net/tcp_impair_proxy.mjs";
import { startUdpProxy } from "../net/udp_impair_proxy.mjs";
import { launch } from "./quic_fixture.mjs";
import { freePort, installClient, startStack } from "./quic_stack_rig.mjs";

const [outputValue, reportsValue] = process.argv.slice(2);
assert.ok(outputValue, "usage: quic_input_latency.mjs OUTPUT.json [REPORTS]");
const output = path.resolve(outputValue);
assert.ok(!fs.existsSync(output), `${output} exists`);
const REPORTS = Number(reportsValue ?? 300);
assert.ok(REPORTS >= 20 && REPORTS <= 5000);
/** Reports at the start of a run that are not counted. */
const WARM_UP = 10;

function hostClocks() {
  try {
    const [monotonic, raw] = execFileSync(
      "python3",
      [
        "-c",
        "import time;print(time.monotonic(), time.clock_gettime(time.CLOCK_MONOTONIC_RAW))",
      ],
      { encoding: "utf8" },
    )
      .trim()
      .split(" ")
      .map(Number);
    return { monotonic, raw };
  } catch {
    return null;
  }
}

function summary(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const at = (share) =>
    sorted[Math.min(sorted.length - 1, Math.floor(share * sorted.length))];
  const round = (value) => Math.round(value * 100) / 100;
  return {
    count: sorted.length,
    median: round(at(0.5)),
    p95: round(at(0.95)),
    p99: round(at(0.99)),
    max: round(sorted[sorted.length - 1]),
    mean: round(sorted.reduce((sum, value) => sum + value, 0) / sorted.length),
  };
}

// In the page: the lease over one transport, `count` key reports one after
// the other, and the time each took.
async function measure([transport, count]) {
  const kvm = window.kvm;
  if (!(await kvm.confirm())) throw new Error("the target was not confirmed");
  const session = transport === "quic" ? await kvm.open() : null;
  const ask = (request) =>
    session ? session.ask(request) : kvm.https(request);
  const acquired = await ask({
    action: "acquire",
    operationId: kvm.operation(),
    ttlSeconds: 30,
  });
  const lease = acquired.body?.data?.leaseId;
  if (!lease) throw new Error(`no lease: ${JSON.stringify(acquired)}`);
  const times = [];
  for (let index = 0; index < count; index += 1) {
    const request = {
      action: "report",
      operationId: kvm.operation(),
      leaseId: lease,
      report: {
        mode: "keyboard",
        data: [0, 0, index % 2 === 0 ? 4 : 0, 0, 0, 0, 0, 0],
      },
    };
    const started = performance.now();
    const answer = await ask(request);
    times.push(performance.now() - started);
    if (answer.body?.data?.applied !== true) {
      throw new Error(`report ${index}: ${JSON.stringify(answer)}`);
    }
  }
  await ask({
    action: "release",
    operationId: kvm.operation(),
    leaseId: lease,
  });
  session?.close();
  return times;
}

const browser = await launch();
const stack = await startStack();
const receipt = {
  schemaVersion: 1,
  probe: "quic-input-latency",
  browser: browser.version(),
  profile: process.env.BLI_KVM_STACK_PROFILE ?? "target/debug",
  reports: REPORTS,
  warmUp: WARM_UP,
  paths: [],
};
const clocksBefore = hostClocks();
let result = "failed";
try {
  for (const condition of [
    { name: "direct", relay: null },
    { name: "relay", relay: { delayMs: 0, loss: 0 } },
    { name: "100 ms round trip", relay: { delayMs: 50, loss: 0 } },
    { name: "100 ms round trip, 2% loss", relay: { delayMs: 50, loss: 0.02 } },
  ]) {
    let port = stack.port;
    let tcp = null;
    let udp = null;
    if (condition.relay) {
      port = await freePort();
      const target = { host: "127.0.0.1", port: stack.port };
      const listen = { host: "127.0.0.1", port };
      tcp = await startTcpProxy({
        listen,
        target,
        ...condition.relay,
        seed: 11,
      });
      udp = await startUdpProxy({
        listen,
        target,
        ...condition.relay,
        seed: 12,
      });
    }
    const context = await browser.newContext();
    const page = await context.newPage();
    await page.goto(`http://127.0.0.1:${port}/`, { waitUntil: "load" });
    await page.evaluate(installClient);
    assert.equal(await page.evaluate(() => window.kvm.login()), 200);
    const entry = { path: condition.name, relay: condition.relay };
    for (const transport of ["https", "quic"]) {
      const times = await page.evaluate(measure, [
        transport,
        REPORTS + WARM_UP,
      ]);
      entry[transport] = summary(times.slice(WARM_UP));
    }
    if (tcp) entry.tcpRelay = tcp.counters();
    if (udp) entry.udpRelay = udp.counters();
    receipt.paths.push(entry);
    console.log(
      `${condition.name}: standard median ${entry.https.median} p95 ${entry.https.p95} max ${entry.https.max} ms; ` +
        `QUIC median ${entry.quic.median} p95 ${entry.quic.p95} max ${entry.quic.max} ms`,
    );
    await context.close();
    await tcp?.close();
    await udp?.close();
  }
  // Every report reached the owner.
  const reported = stack.owner
    .leases()
    .filter((request) => request.op === "report").length;
  assert.equal(reported, receipt.paths.length * 2 * (REPORTS + WARM_UP));
  receipt.reportsAtTheOwner = reported;
  result = "passed";
} catch (error) {
  receipt.error = String(error?.message ?? error).slice(0, 400);
  console.error(stack.logs());
} finally {
  const clocksAfter = hostClocks();
  if (clocksBefore && clocksAfter) {
    receipt.hostClockRate =
      Math.round(
        ((clocksAfter.monotonic - clocksBefore.monotonic) /
          (clocksAfter.raw - clocksBefore.raw)) *
          10000,
      ) / 10000;
  }
  receipt.result = result;
  fs.writeFileSync(output, JSON.stringify(receipt, null, 1));
  await browser.close();
  await stack.close();
}
console.log(`result: ${result}`);
process.exit(result === "passed" ? 0 : 1);
