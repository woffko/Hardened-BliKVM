// A UDP relay that impairs what passes through it, for measuring the QUIC
// transport on a bad path without touching the network of the host: every
// datagram is delayed by a fixed time each way, and dropped with a given
// probability. Each client address gets a socket of its own towards the
// target, so answers find their way back.
//
// The random numbers come from seeded generators, one for each direction:
// the same seed drops the same datagrams of the same traffic.
//
// Usage:
//   node udp_impair_proxy.mjs --listen HOST:PORT --target HOST:PORT
//        [--delay-ms N] [--jitter-ms N] [--loss P] [--seed N]
// It prints one JSON line when it listens, and its counters as one JSON line
// when it ends (SIGINT or SIGTERM). `startUdpProxy` does the same from a
// script, which may also change the impairment while the relay runs
// (`impair`).
import dgram from "node:dgram";
import { pathToFileURL } from "node:url";

const CLIENT_IDLE_MS = 60_000;

// mulberry32: small, fast, and good enough to choose which datagram is lost.
export function generator(seed) {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let value = state;
    value = Math.imul(value ^ (value >>> 15), value | 1);
    value ^= value + Math.imul(value ^ (value >>> 7), value | 61);
    return ((value ^ (value >>> 14)) >>> 0) / 4294967296;
  };
}

export function endpoint(value) {
  const match = /^(.+):(\d{1,5})$/.exec(value ?? "");
  if (!match || Number(match[2]) > 65535) {
    throw new Error(`not HOST:PORT: ${value}`);
  }
  return { host: match[1].replace(/^\[|\]$/g, ""), port: Number(match[2]) };
}

function checked({ delayMs, jitterMs, loss }) {
  if (!(loss >= 0 && loss < 1)) throw new Error("loss must be in [0, 1)");
  if (!(delayMs >= 0 && jitterMs >= 0)) throw new Error("times must be >= 0");
  return { delayMs, jitterMs, loss };
}

/**
 * Starts the relay. `loss` is a probability from 0 to 1 and applies to each
 * direction; `delayMs` is added each way, `jitterMs` is the width of a
 * uniform spread around it (0 keeps the order of datagrams).
 */
export async function startUdpProxy({
  listen,
  target,
  delayMs = 0,
  jitterMs = 0,
  loss = 0,
  seed = 1,
}) {
  let impairment = checked({ delayMs, jitterMs, loss });
  // One generator for each direction, so what is lost one way does not
  // depend on how the two directions happen to interleave.
  const random = {
    toTarget: generator(seed),
    toClient: generator(seed + 0x9e3779b9),
  };
  const family = target.host.includes(":") ? "udp6" : "udp4";
  const front = dgram.createSocket(listen.host.includes(":") ? "udp6" : "udp4");
  const clients = new Map();
  const counters = {
    toTarget: { received: 0, dropped: 0, forwarded: 0, bytes: 0 },
    toClient: { received: 0, dropped: 0, forwarded: 0, bytes: 0 },
  };
  const timers = new Set();
  let closed = false;

  const pass = (direction, datagram, send) => {
    const counter = counters[direction];
    const { delayMs, jitterMs, loss } = impairment;
    counter.received += 1;
    if (loss > 0 && random[direction]() < loss) {
      counter.dropped += 1;
      return;
    }
    const forward = () => {
      if (closed) return;
      counter.forwarded += 1;
      counter.bytes += datagram.length;
      send(datagram);
    };
    const wait =
      delayMs + (jitterMs > 0 ? (random[direction]() - 0.5) * jitterMs : 0);
    if (wait <= 0) {
      forward();
      return;
    }
    const timer = setTimeout(() => {
      timers.delete(timer);
      forward();
    }, wait);
    timers.add(timer);
  };

  front.on("message", (datagram, from) => {
    const key = `${from.address} ${from.port}`;
    let client = clients.get(key);
    if (!client) {
      const upstream = dgram.createSocket(family);
      client = { upstream, seen: 0 };
      clients.set(key, client);
      upstream.on("message", (answer) =>
        pass("toClient", answer, (bytes) =>
          front.send(bytes, from.port, from.address, () => undefined),
        ),
      );
      upstream.on("error", () => undefined);
    }
    client.seen = performance.now();
    pass("toTarget", datagram, (bytes) =>
      client.upstream.send(bytes, target.port, target.host, () => undefined),
    );
  });
  front.on("error", () => undefined);
  const sweep = setInterval(() => {
    const now = performance.now();
    for (const [key, client] of clients) {
      if (now - client.seen > CLIENT_IDLE_MS) {
        client.upstream.close();
        clients.delete(key);
      }
    }
  }, 10_000);
  sweep.unref();

  await new Promise((resolve, reject) => {
    front.once("error", reject);
    front.bind(listen.port, listen.host, resolve);
  });
  return {
    port: front.address().port,
    counters: () => structuredClone(counters),
    /**
     * Changes the impairment for the datagrams that arrive from now on;
     * what is not named stays as it is.
     */
    impair(changes) {
      impairment = checked({ ...impairment, ...changes });
    },
    async close() {
      closed = true;
      clearInterval(sweep);
      for (const timer of timers) clearTimeout(timer);
      for (const client of clients.values()) client.upstream.close();
      clients.clear();
      await new Promise((resolve) => front.close(resolve));
    },
  };
}

function options(argv) {
  const values = {};
  for (let index = 0; index < argv.length; index += 2) {
    const name = argv[index];
    if (!name.startsWith("--") || index + 1 >= argv.length) {
      throw new Error(`unexpected argument: ${name}`);
    }
    values[name.slice(2)] = argv[index + 1];
  }
  return values;
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const given = options(process.argv.slice(2));
  const proxy = await startUdpProxy({
    listen: endpoint(given.listen),
    target: endpoint(given.target),
    delayMs: Number(given["delay-ms"] ?? 0),
    jitterMs: Number(given["jitter-ms"] ?? 0),
    loss: Number(given.loss ?? 0),
    seed: Number(given.seed ?? 1),
  });
  console.log(JSON.stringify({ listening: proxy.port }));
  const end = async () => {
    console.log(JSON.stringify(proxy.counters()));
    await proxy.close();
    process.exit(0);
  };
  process.on("SIGINT", end);
  process.on("SIGTERM", end);
}
