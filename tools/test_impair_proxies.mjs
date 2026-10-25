import assert from "node:assert/strict";
import crypto from "node:crypto";
import dgram from "node:dgram";
import net from "node:net";
import test from "node:test";

import { startTcpProxy } from "./net/tcp_impair_proxy.mjs";
import { endpoint, generator, startUdpProxy } from "./net/udp_impair_proxy.mjs";

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function udpEcho() {
  const socket = dgram.createSocket("udp4");
  socket.on("message", (datagram, from) =>
    socket.send(datagram, from.port, from.address),
  );
  await new Promise((resolve) => socket.bind(0, "127.0.0.1", resolve));
  return socket;
}

// Sends `count` numbered datagrams and collects what comes back, with the
// time each took.
async function exchange(port, count, settle) {
  const socket = dgram.createSocket("udp4");
  const sent = new Map();
  const back = [];
  socket.on("message", (datagram) => {
    const number = datagram.readUInt32BE(0);
    back.push({ number, ms: performance.now() - sent.get(number) });
  });
  await new Promise((resolve) => socket.bind(0, "127.0.0.1", resolve));
  for (let number = 0; number < count; number += 1) {
    const datagram = Buffer.alloc(64);
    datagram.writeUInt32BE(number, 0);
    sent.set(number, performance.now());
    socket.send(datagram, port, "127.0.0.1");
    // One process plays every part here: bursts are kept small enough for
    // its sockets to be read in time.
    if (number % 10 === 9) await sleep(1);
  }
  await sleep(settle);
  socket.close();
  return back;
}

test("an endpoint is a host and a port", () => {
  assert.deepEqual(endpoint("127.0.0.1:443"), { host: "127.0.0.1", port: 443 });
  assert.deepEqual(endpoint("[::1]:8443"), { host: "::1", port: 8443 });
  for (const bad of ["", "host", "host:99999", "host:port"]) {
    assert.throws(() => endpoint(bad), /HOST:PORT/, bad);
  }
});

test("the generator repeats itself for a seed and spreads evenly", () => {
  const first = generator(7);
  const second = generator(7);
  const other = generator(8);
  const values = Array.from({ length: 1000 }, () => first());
  assert.deepEqual(
    values,
    Array.from({ length: 1000 }, () => second()),
  );
  assert.notDeepEqual(values.slice(0, 5), [
    other(),
    other(),
    other(),
    other(),
    other(),
  ]);
  assert.ok(values.every((value) => value >= 0 && value < 1));
  const mean = values.reduce((sum, value) => sum + value, 0) / values.length;
  assert.ok(mean > 0.45 && mean < 0.55, `mean ${mean}`);
});

test("a clean relay passes every datagram, in order, both ways", async (t) => {
  const echo = await udpEcho();
  const proxy = await startUdpProxy({
    listen: { host: "127.0.0.1", port: 0 },
    target: { host: "127.0.0.1", port: echo.address().port },
  });
  t.after(async () => {
    await proxy.close();
    echo.close();
  });
  const back = await exchange(proxy.port, 500, 200);
  assert.deepEqual(
    back.map((entry) => entry.number),
    Array.from({ length: 500 }, (_, number) => number),
  );
  const counters = proxy.counters();
  assert.equal(counters.toTarget.forwarded, 500);
  assert.equal(counters.toClient.forwarded, 500);
  assert.equal(counters.toTarget.dropped + counters.toClient.dropped, 0);
});

test("delay is added each way and loss takes its share each way", async (t) => {
  const echo = await udpEcho();
  t.after(() => echo.close());
  const options = {
    listen: { host: "127.0.0.1", port: 0 },
    target: { host: "127.0.0.1", port: echo.address().port },
    delayMs: 30,
    loss: 0.1,
    seed: 42,
  };
  const proxy = await startUdpProxy(options);
  t.after(() => proxy.close());
  const back = await exchange(proxy.port, 2000, 400);
  const counters = proxy.counters();
  // Every datagram is counted, and about a tenth is dropped each way.
  assert.equal(counters.toTarget.received, 2000);
  assert.equal(counters.toTarget.forwarded + counters.toTarget.dropped, 2000);
  assert.equal(counters.toClient.received, counters.toTarget.forwarded);
  for (const direction of [counters.toTarget, counters.toClient]) {
    const share = direction.dropped / direction.received;
    assert.ok(share > 0.07 && share < 0.13, `lost ${share}`);
  }
  assert.equal(back.length, counters.toClient.forwarded);
  // Nothing is faster than the two delays; order is kept without jitter.
  assert.ok(
    back.every((entry) => entry.ms >= 59),
    "an echo came back early",
  );
  assert.deepEqual(
    back.map((entry) => entry.number),
    back.map((entry) => entry.number).sort((a, b) => a - b),
  );

  // The same seed loses the same datagrams of the same traffic.
  const again = await startUdpProxy(options);
  t.after(() => again.close());
  await exchange(again.port, 2000, 400);
  assert.deepEqual(again.counters().toTarget, counters.toTarget);
});

test("the impairment can change while the relay runs", async (t) => {
  const echo = await udpEcho();
  t.after(() => echo.close());
  const proxy = await startUdpProxy({
    listen: { host: "127.0.0.1", port: 0 },
    target: { host: "127.0.0.1", port: echo.address().port },
    seed: 7,
  });
  t.after(() => proxy.close());
  assert.equal((await exchange(proxy.port, 300, 300)).length, 300);
  proxy.impair({ loss: 0.2 });
  const lossy = await exchange(proxy.port, 1000, 300);
  const dropped = () =>
    proxy.counters().toTarget.dropped + proxy.counters().toClient.dropped;
  const lost = dropped();
  assert.equal(lossy.length, 1000 - lost);
  assert.ok(lost > 250 && lost < 470, `lost ${lost}`);
  proxy.impair({ loss: 0, delayMs: 20 });
  const late = await exchange(proxy.port, 300, 300);
  assert.equal(late.length, 300);
  assert.equal(dropped(), lost);
  assert.ok(
    late.every((entry) => entry.ms >= 39),
    "an echo came back early",
  );
  assert.throws(() => proxy.impair({ loss: 1 }), /loss must be/);
});

async function tcpEcho() {
  const server = net.createServer((socket) => socket.pipe(socket));
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  return server;
}

function roundTrip(port, payload) {
  return new Promise((resolve, reject) => {
    const started = performance.now();
    const chunks = [];
    let received = 0;
    const socket = net.connect(port, "127.0.0.1", () => socket.write(payload));
    socket.setNoDelay(true);
    socket.on("data", (chunk) => {
      chunks.push(chunk);
      received += chunk.length;
      if (received >= payload.length) {
        socket.destroy();
        resolve({
          bytes: Buffer.concat(chunks),
          ms: performance.now() - started,
        });
      }
    });
    socket.on("error", reject);
  });
}

test("the TCP relay delays each way and keeps the bytes", async (t) => {
  const echo = await tcpEcho();
  const proxy = await startTcpProxy({
    listen: { host: "127.0.0.1", port: 0 },
    target: { host: "127.0.0.1", port: echo.address().port },
    delayMs: 25,
  });
  t.after(async () => {
    await proxy.close();
    echo.close();
  });
  const small = await roundTrip(proxy.port, Buffer.from("report"));
  assert.equal(small.bytes.toString(), "report");
  assert.ok(small.ms >= 49 && small.ms < 400, `took ${small.ms} ms`);
  const payload = crypto.randomBytes(300_000);
  const large = await roundTrip(proxy.port, payload);
  assert.ok(large.bytes.equals(payload), "the bytes changed on the way");
  assert.equal(proxy.counters().toTarget.stalled, 0);
});

test("a lost segment stalls its direction for the retransmission", async (t) => {
  const echo = await tcpEcho();
  const proxy = await startTcpProxy({
    listen: { host: "127.0.0.1", port: 0 },
    target: { host: "127.0.0.1", port: echo.address().port },
    delayMs: 5,
    loss: 0.5,
    stallMs: 120,
    seed: 3,
  });
  t.after(async () => {
    await proxy.close();
    echo.close();
  });
  const times = [];
  for (let index = 0; index < 12; index += 1) {
    const answer = await roundTrip(proxy.port, Buffer.from(`report ${index}`));
    assert.equal(answer.bytes.toString(), `report ${index}`);
    times.push(answer.ms);
  }
  const counters = proxy.counters();
  const stalled = counters.toTarget.stalled + counters.toClient.stalled;
  assert.ok(stalled >= 4 && stalled <= 20, `${stalled} stalls`);
  // A round trip without a stall takes the two delays; one with a stall at
  // least the stall longer.
  assert.ok(Math.min(...times) < 100, `fastest ${Math.min(...times)} ms`);
  assert.ok(Math.max(...times) >= 129, `slowest ${Math.max(...times)} ms`);
});
