// The TCP counterpart of udp_impair_proxy.mjs, for the standard transport:
// it relays connections and delays each direction by a fixed time.
//
// Loss cannot be applied to TCP segments from user space: the kernel's TCP
// would have to lose them. What a lost segment costs the application is
// modelled instead: with probability `loss` a segment, and everything behind
// it in its direction, waits one retransmission longer. The stall defaults to
// what Linux takes to repair the loss of the only segment in flight, which is
// the case of keyboard and mouse reports: twice the round trip plus 200 ms
// (the tail-loss probe's timeout). Numbers taken through this relay with
// loss are therefore a model of TCP, not a measurement of it; without loss
// the relay only delays.
//
// Usage:
//   node tcp_impair_proxy.mjs --listen HOST:PORT --target HOST:PORT
//        [--delay-ms N] [--loss P] [--stall-ms N] [--seed N]
// It prints one JSON line when it listens, and its counters as one JSON line
// when it ends. `startTcpProxy` does the same from a script.
import net from "node:net";
import { pathToFileURL } from "node:url";

import { endpoint, generator } from "./udp_impair_proxy.mjs";

/** The largest piece that counts as one segment. */
const SEGMENT_BYTES = 1400;

export async function startTcpProxy({
  listen,
  target,
  delayMs = 0,
  loss = 0,
  stallMs,
  seed = 1,
}) {
  if (!(loss >= 0 && loss < 1)) throw new Error("loss must be in [0, 1)");
  if (!(delayMs >= 0)) throw new Error("the delay must be >= 0");
  const stall = stallMs ?? 4 * delayMs + 200;
  const random = {
    toTarget: generator(seed),
    toClient: generator(seed + 0x9e3779b9),
  };
  const counters = {
    connections: 0,
    toTarget: { segments: 0, stalled: 0, bytes: 0 },
    toClient: { segments: 0, stalled: 0, bytes: 0 },
  };
  const sockets = new Set();

  // Relays one direction: every chunk leaves no earlier than its delay after
  // it arrived, and never before the chunk in front of it.
  const relay = (from, to, direction) => {
    const counter = counters[direction];
    let release = 0;
    let waiting = 0;
    let ended = false;
    const finish = () => {
      if (ended && waiting === 0) to.end();
    };
    from.on("data", (chunk) => {
      let wait = delayMs;
      for (let offset = 0; offset < chunk.length; offset += SEGMENT_BYTES) {
        counter.segments += 1;
        if (loss > 0 && random[direction]() < loss) {
          counter.stalled += 1;
          wait += stall;
        }
      }
      counter.bytes += chunk.length;
      release = Math.max(release, performance.now() + wait);
      waiting += 1;
      setTimeout(
        () => {
          waiting -= 1;
          if (!to.destroyed) to.write(chunk);
          finish();
        },
        Math.max(0, release - performance.now()),
      );
    });
    from.on("end", () => {
      ended = true;
      finish();
    });
    from.on("error", () => to.destroy());
    from.on("close", () => {
      if (!ended) to.destroy();
    });
  };

  const server = net.createServer((client) => {
    counters.connections += 1;
    const upstream = net.connect(target.port, target.host);
    client.setNoDelay(true);
    upstream.setNoDelay(true);
    for (const socket of [client, upstream]) {
      sockets.add(socket);
      socket.on("close", () => sockets.delete(socket));
    }
    relay(client, upstream, "toTarget");
    relay(upstream, client, "toClient");
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(listen.port, listen.host, resolve);
  });
  return {
    port: server.address().port,
    counters: () => structuredClone(counters),
    async close() {
      for (const socket of sockets) socket.destroy();
      await new Promise((resolve) => server.close(resolve));
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
  const proxy = await startTcpProxy({
    listen: endpoint(given.listen),
    target: endpoint(given.target),
    delayMs: Number(given["delay-ms"] ?? 0),
    loss: Number(given.loss ?? 0),
    stallMs:
      given["stall-ms"] === undefined ? undefined : Number(given["stall-ms"]),
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
