// The whole input path on the host, for the contracts and measurements that
// need more than a played server: the real web server and the real gateway
// (the host's builds), and a stand-in for the board owner that applies what
// the server asks of it and grants a lease. A page of the server's own origin
// talks to them the way the application does: the HID lease API over HTTP,
// and the same requests over the QUIC session's input stream.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import dgram from "node:dgram";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";

const PATIENCE_MS = 15000;
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

export function binary(variable, name) {
  const file =
    process.env[variable] ??
    path.resolve(process.env.BLI_KVM_STACK_PROFILE ?? "target/debug", name);
  assert.ok(fs.existsSync(file), `${name} is not built: ${file}`);
  return file;
}

/**
 * A port number that is free for TCP and for UDP on the loopback address: a
 * page reaches the server and the gateway under one number, its origin's.
 */
export async function freePort() {
  for (let attempt = 0; attempt < 20; attempt += 1) {
    const server = net.createServer();
    await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
    const { port } = server.address();
    await new Promise((resolve) => server.close(resolve));
    const socket = dgram.createSocket("udp4");
    const free = await new Promise((resolve) => {
      socket.once("error", () => resolve(false));
      socket.bind(port, "127.0.0.1", () => resolve(true));
    });
    await new Promise((resolve) => socket.close(resolve));
    if (free) return port;
  }
  throw new Error("no port is free for both TCP and UDP");
}

/**
 * The board owner's control socket as the lease path meets it. Every request
 * is applied; an acquire grants the lease `55…55`. The owner's messages carry
 * 64-bit numbers, so the request's number and operation are copied as text.
 * `hidMs` is how long a report takes to be written.
 */
export function startOwner(socketPath, { hidMs = 0 } = {}) {
  const requests = [];
  const sockets = new Set();
  // The server closes its side once it has asked; the answer may follow
  // later, so the socket stays open for it.
  const server = net.createServer({ allowHalfOpen: true }, (socket) => {
    sockets.add(socket);
    socket.on("close", () => sockets.delete(socket));
    socket.on("error", () => undefined);
    let waiting = 0;
    let ended = false;
    const finish = () => {
      if (ended && waiting === 0) socket.end();
    };
    socket.on("end", () => {
      ended = true;
      finish();
    });
    let buffer = Buffer.alloc(0);
    socket.on("data", (chunk) => {
      buffer = Buffer.concat([buffer, chunk]);
      while (buffer.length >= 4) {
        const length = buffer.readUInt32BE(0);
        if (buffer.length < 4 + length) return;
        const text = buffer.subarray(4, 4 + length).toString("utf8");
        buffer = buffer.subarray(4 + length);
        const requestId = /"requestId":(\d+)/.exec(text)?.[1];
        const operationId = /"operationId":"([0-9a-f]+)"/.exec(text)?.[1];
        const type = /"request":\{"type":"([a-z_]+)"/.exec(text)?.[1];
        const op = /"lease":\{"op":"([a-z_]+)"/.exec(text)?.[1] ?? null;
        const data = /"report":\{"mode":"([a-z_]+)","data":\[([0-9,]+)\]/.exec(
          text,
        );
        requests.push({
          type,
          op,
          report: data
            ? { mode: data[1], data: data[2].split(",").map(Number) }
            : null,
        });
        const response =
          op === "acquire"
            ? `{"type":"lease_granted","lease_id":[${Array(16).fill(85).join(",")}]}`
            : '{"type":"applied"}';
        const body = Buffer.from(
          `{"schemaVersion":2,"requestId":${requestId},"operationId":"${operationId}","response":${response}}`,
        );
        const prefix = Buffer.alloc(4);
        prefix.writeUInt32BE(body.length);
        waiting += 1;
        const answer = () => {
          socket.write(Buffer.concat([prefix, body]));
          waiting -= 1;
          finish();
        };
        if (op === "report" && hidMs > 0) setTimeout(answer, hidMs);
        else answer();
      }
    });
  });
  return new Promise((resolve) =>
    server.listen(socketPath, () =>
      resolve({
        requests,
        /** The lease requests seen, by what they ask. */
        leases: () =>
          requests.filter((request) => request.type === "hid_lease"),
        close: () =>
          new Promise((done) => {
            for (const socket of sockets) socket.destroy();
            server.close(done);
          }),
      }),
    ),
  );
}

function run(file, env, name) {
  const child = spawn(file, [], { env, stdio: ["ignore", "ignore", "pipe"] });
  const state = { child, log: "", exited: false };
  child.stderr.on("data", (chunk) => (state.log += chunk));
  child.once("exit", () => (state.exited = true));
  state.said = async (pattern) => {
    const deadline = performance.now() + PATIENCE_MS;
    while (!pattern.test(state.log)) {
      assert.ok(!state.exited, `${name} ended:\n${state.log}`);
      assert.ok(
        performance.now() < deadline,
        `${name} did not start:\n${state.log}`,
      );
      await sleep(20);
    }
  };
  state.stop = () =>
    new Promise((resolve) => {
      if (state.exited) return resolve();
      child.once("exit", resolve);
      child.kill("SIGTERM");
    });
  return state;
}

/**
 * Starts the owner, the server and the gateway. The server listens on the
 * TCP port and the gateway on the UDP port of one number, `port`, so a page
 * loaded from the server may open a session.
 */
export async function startStack({ hidMs = 0 } = {}) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "quic-stack-"));
  const owner = await startOwner(path.join(directory, "control.sock"), {
    hidMs,
  });
  const port = await freePort();
  const link = path.join(directory, "link.sock");
  const server = run(
    binary("BLI_KVM_SERVER", "bli-kvm-server"),
    {
      BLI_KVM_BIND: `127.0.0.1:${port}`,
      BLI_KVM_WEB_ROOT: path.resolve("web/dist"),
      BLI_KVM_STATE_DIR: path.join(directory, "state"),
      BLI_KVM_CONTROL_SOCKET: path.join(directory, "control.sock"),
      BLI_KVM_QUIC_LINK_SOCKET: link,
      BLI_KVM_QUIC_LINK_USER: os.userInfo().username,
    },
    "the server",
  );
  await server.said(/listening on/);
  const gateway = run(
    binary("BLI_KVM_QUIC_GATEWAY", "bli-kvm-quic-gateway"),
    {
      BLI_KVM_QUIC_BIND: `127.0.0.1:${port}`,
      BLI_KVM_QUIC_LINK_SOCKET: link,
    },
    "the gateway",
  );
  await gateway.said(/the server link is up/);
  return {
    port,
    owner,
    /** Stops the gateway the way its unit is stopped. */
    stopGateway: () => gateway.stop(),
    logs: () => `server:\n${server.log}\ngateway:\n${gateway.log}`,
    async close() {
      await gateway.stop();
      await server.stop();
      await owner.close();
      fs.rmSync(directory, { recursive: true, force: true });
    },
  };
}

/**
 * Installs `window.kvm` in a page of the server's origin: the requests the
 * application makes, without its interface. Passed to `page.evaluate`.
 */
export function installClient() {
  const json = async (method, route, body, csrf) => {
    const response = await fetch(route, {
      method,
      credentials: "include",
      headers: {
        "Content-Type": "application/json",
        ...(csrf ? { "x-csrf-token": csrf } : {}),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    let parsed = null;
    try {
      parsed = await response.json();
    } catch {
      // No JSON body.
    }
    return { status: response.status, body: parsed };
  };
  const operation = () =>
    Array.from(crypto.getRandomValues(new Uint8Array(16)), (byte) =>
      byte.toString(16).padStart(2, "0"),
    ).join("");
  const frame = (message) => {
    const body = new TextEncoder().encode(JSON.stringify(message));
    const out = new Uint8Array(2 + body.length);
    out[0] = body.length >> 8;
    out[1] = body.length & 0xff;
    out.set(body, 2);
    return out;
  };
  // Reads the length-prefixed messages of a stream one by one.
  const reading = (readable) => {
    const reader = readable.getReader();
    let buffer = new Uint8Array(0);
    return async () => {
      for (;;) {
        if (buffer.length >= 2) {
          const length = (buffer[0] << 8) | buffer[1];
          if (buffer.length >= 2 + length) {
            const message = JSON.parse(
              new TextDecoder().decode(buffer.subarray(2, 2 + length)),
            );
            buffer = buffer.slice(2 + length);
            return message;
          }
        }
        const { value, done } = await reader.read();
        if (done || !value) return null;
        const joined = new Uint8Array(buffer.length + value.length);
        joined.set(buffer);
        joined.set(value, buffer.length);
        buffer = joined;
      }
    };
  };

  window.kvm = {
    csrf: null,
    operation,
    /** Creates the account on a new server, and logs in. */
    async login() {
      const account = { username: "operator", password: "initial password" };
      await json("POST", "/api/auth/setup", account);
      const login = await json("POST", "/api/auth/login", account);
      this.csrf = login.body.data.csrfToken;
      return login.status;
    },
    /** The operator's confirmation of the target, as before a lease. */
    async confirm() {
      const challenge = await json("GET", "/api/hid/focus");
      const confirmed = await json(
        "POST",
        "/api/hid/focus",
        {
          operationId: operation(),
          challenge: challenge.body.data.challenge,
          targetId: "11".repeat(16),
          operatorConfirmed: true,
        },
        this.csrf,
      );
      return confirmed.body.data.attested;
    },
    /** A request of the lease API over HTTP: its status and body. */
    https(request) {
      return json("POST", "/api/hid/lease", request, this.csrf);
    },
    /** Opens a QUIC session of this login on the page's own port. */
    async open() {
      const grant = (
        await json(
          "POST",
          "/api/transport/quic/session",
          { pageTime: Date.now() },
          this.csrf,
        )
      ).body.data;
      if (!grant.granted) throw new Error(`no token: ${grant.reason}`);
      const hash = Uint8Array.from(
        grant.certificateSha256.match(/../g),
        (pair) => Number.parseInt(pair, 16),
      );
      const port = location.port || 443;
      const session = new WebTransport(
        `https://${location.hostname}:${port}/kvm`,
        { serverCertificateHashes: [{ algorithm: "sha-256", value: hash }] },
      );
      session.closed.catch(() => undefined);
      await session.ready;
      const control = await session.createBidirectionalStream();
      const controlWriter = control.writable.getWriter();
      const hearControl = reading(control.readable);
      await controlWriter.write(
        frame({ t: "hello", protocol: 1, token: grant.token }),
      );
      const welcome = await hearControl();
      if (welcome?.t !== "welcome")
        throw new Error(`no welcome: ${welcome?.t}`);
      const input = await session.createBidirectionalStream();
      const writer = input.writable.getWriter();
      const hear = reading(input.readable);
      let seq = 0;
      return {
        session,
        /** One request and its answer: the lease API's status and body. */
        async ask(request) {
          seq += 1;
          await writer.write(frame({ seq, request }));
          const answer = await hear();
          if (answer?.seq !== seq) throw new Error("the answer is another's");
          return answer.response;
        },
        /** Requests written at once, answered in their order. */
        async askAll(requests) {
          const first = seq + 1;
          const bytes = requests.map((request) => {
            seq += 1;
            return frame({ seq, request });
          });
          const all = new Uint8Array(
            bytes.reduce((sum, one) => sum + one.length, 0),
          );
          let offset = 0;
          for (const one of bytes) {
            all.set(one, offset);
            offset += one.length;
          }
          await writer.write(all);
          const answers = [];
          for (let index = 0; index < requests.length; index += 1) {
            const answer = await hear();
            if (answer?.seq !== first + index) {
              throw new Error("the answers are out of order");
            }
            answers.push(answer.response);
          }
          return answers;
        },
        close: () => session.close(),
      };
    },
  };
}
