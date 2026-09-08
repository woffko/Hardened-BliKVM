import assert from "node:assert/strict";
import test from "node:test";
import { EventEmitter } from "node:events";
import { MjpegProgress } from "./playwright/mjpeg_progress.mjs";
import { StatusOutageControl } from "./playwright/status_outage_control.mjs";
import {
  createStatusOutageWindow,
  validateOutageResult,
} from "./playwright/status_outage_audit.mjs";
import { statusRoute } from "./playwright/status_api_acceptance.mjs";
import { runExistingAccountBrowser } from "./playwright/device_existing_account_smoke.mjs";
import { createStatusOutageScenario } from "./playwright/status_outage_scenario.mjs";

test("browser workflow import has no CLI or stdin side effects", () => {
  assert.equal(typeof runExistingAccountBrowser, "function");
});

test("only exact status GET 503 during the bounded window is accepted", () => {
  let now = 100;
  const window = createStatusOutageWindow(() => now);
  const expected = { method: "GET", route: statusRoute, status: 503 };
  assert.equal(window.allows(expected), false);
  window.begin();
  const token = window.captureRequest();
  for (const wrong of [
    { ...expected, method: "POST" },
    { ...expected, status: 500 },
    { ...expected, route: "/api/inventory" },
    { ...expected, route: "/api/stream/mjpeg" },
  ]) {
    assert.equal(window.allows(wrong, token), false);
  }
  assert.equal(window.allows(expected), false);
  assert.equal(window.allows(expected, token), true);
  window.end();
  window.assertCompleted();
  assert.equal(window.allows(expected), false);
  assert.throws(() => window.begin());
  assert.throws(() => window.end());
  now += 1;
});

test("expired, reversed or never-observed windows cannot pass", () => {
  for (const elapsed of [-1, 90_001]) {
    let now = 100;
    const window = createStatusOutageWindow(() => now);
    window.begin();
    const token = window.captureRequest();
    now += elapsed;
    assert.equal(
      window.allows({ method: "GET", route: statusRoute, status: 503 }, token),
      false,
    );
    assert.throws(() => window.end());
    assert.throws(() => window.assertCompleted());
  }
  const window = createStatusOutageWindow(() => 100);
  window.begin();
  window.end();
  assert.throws(() => window.assertCompleted());
});

test("late responses retain only their bounded in-window request allowance", () => {
  let now = 0;
  const window = createStatusOutageWindow(() => now);
  const response = { method: "GET", route: statusRoute, status: 503 };
  const before = window.captureRequest();
  window.begin();
  const during = window.captureRequest();
  assert.equal(window.allows(response, before), false);
  assert.equal(window.allows(response, Symbol("forged")), false);
  window.end();
  assert.equal(window.allows(response, before), false);
  assert.equal(window.allows(response), false);
  assert.equal(window.allows(response, during), true);
  window.assertCompleted();
  now = 90_001;
  assert.equal(window.allows(response, during), false);
});

test("only closed boolean acceptance facts reach the receipt", () => {
  const result = {
    staleVerified: true,
    coldErrorVerified: true,
    recoveryVerified: true,
    videoPreserved: true,
  };
  validateOutageResult(result);
  assert.throws(() =>
    validateOutageResult({ ...result, extra: "not accepted" }),
  );
  assert.throws(() => validateOutageResult({ ...result, videoPreserved: 1 }));
  assert.throws(() => validateOutageResult(null));
});

function scenarioFixture({
  failStop = false,
  failCold = false,
  falseIdle = false,
  failRestore = false,
  endedStream = false,
} = {}) {
  let outage = false;
  const events = [];
  const stream = {
    finished: () =>
      endedStream ? Promise.resolve(null) : new Promise(() => {}),
  };
  function makePage(cold = false) {
    let hasData = !cold;
    let closed = false;
    let state = cold ? "error" : "ready";
    const count = (text) => {
      if (text === "0.1.0-dev4 · #5") return hasData ? 2 : 0;
      if (text === "No update awaiting confirmation")
        return state === "ready" || falseIdle ? 1 : 0;
      if (text === "Connection lost. The displayed status may be outdated.")
        return state === "stale" ? 1 : 0;
      if (text === "Update status is unavailable. Retrying automatically.")
        return state === "error" ? 1 : 0;
      return 0;
    };
    const locator = (text = "") => ({
      first() {
        return this;
      },
      async click() {
        if (text === "Refresh") {
          state = outage ? (hasData ? "stale" : "error") : "ready";
          if (!outage) hasData = true;
        }
      },
      async waitFor() {
        if (
          [
            "No update awaiting confirmation",
            "Connection lost. The displayed status may be outdated.",
            "Update status is unavailable. Retrying automatically.",
          ].includes(text)
        ) {
          assert.equal(count(text), 1);
        }
      },
      async count() {
        return count(text);
      },
      async evaluate() {
        return true;
      },
      locator,
      getByText: locator,
      getByTestId: locator,
      getByRole: (_role, options) => locator(options?.name),
    });
    return {
      locator,
      getByRole: () => locator(),
      mouse: { async move() {} },
      async bringToFront() {},
      async setViewportSize() {},
      url: () => "https://device.invalid/#/",
      async goto() {
        if (failCold) throw new Error("cold document failed");
        return { status: () => 200 };
      },
      async close() {
        closed = true;
        events.push("close");
      },
      isClosed: () => closed,
      async waitForResponse() {
        const slot = {
          slotId:
            "app-00000000000000000005-41cb7620d36a47c3416e748296fa1512fc59aaf9012f12f04c0f659098629ebc",
          version: "0.1.0-dev4",
          sequence: "5",
          minimumSystemSequence: "1",
        };
        return {
          status: () => (outage ? 503 : 200),
          json: async () => ({
            code: 0,
            data: {
              schemaVersion: 1,
              generation: "22",
              floorSequence: "5",
              systemSequence: "1",
              pending: null,
              lastRollback: null,
              confirmed: slot,
              selected: slot,
            },
          }),
        };
      },
    };
  }
  const cold = makePage(true);
  return {
    events,
    cold,
    scenario: createStatusOutageScenario(async (command) => {
      events.push(command);
      if (failRestore && command === "restore")
        throw new Error("restore failed");
      outage = command === "stop";
      if (failStop && outage) throw new Error("stop acknowledgement lost");
    }),
    args: {
      page: makePage(),
      beginOutage: () => events.push("begin"),
      endOutage: () => events.push("end"),
      activeStream: () => stream,
      verifyVideoProgress: async () => {
        if (endedStream) throw new Error("MJPEG progress failed");
      },
      createObservedPage: async () => cold,
    },
  };
}

test("outage scenario verifies warm/cold recovery and closes the extra page", async () => {
  const fixture = scenarioFixture();
  validateOutageResult(await fixture.scenario(fixture.args));
  assert.deepEqual(fixture.events, [
    "begin",
    "stop",
    "restore",
    "end",
    "close",
  ]);
  assert.equal(fixture.cold.isClosed(), true);
});

test("lost stop acknowledgement still requests restoration", async () => {
  const fixture = scenarioFixture({ failStop: true });
  await assert.rejects(fixture.scenario(fixture.args), /acknowledgement lost/);
  assert.deepEqual(fixture.events, ["begin", "stop", "restore", "end"]);
});

test("cold page failure restores the socket and closes that page", async () => {
  const fixture = scenarioFixture({ failCold: true });
  await assert.rejects(fixture.scenario(fixture.args), /cold document failed/);
  assert.deepEqual(fixture.events, [
    "begin",
    "stop",
    "restore",
    "end",
    "close",
  ]);
});

test("stale data may not retain the healthy idle claim", async () => {
  const fixture = scenarioFixture({ falseIdle: true });
  await assert.rejects(fixture.scenario(fixture.args));
  assert.deepEqual(fixture.events, ["begin", "stop", "restore", "end"]);
});

test("failed restoration cannot pass and still closes the cold page", async () => {
  const fixture = scenarioFixture({ failRestore: true });
  await assert.rejects(fixture.scenario(fixture.args), /restore failed/);
  assert.deepEqual(fixture.events, [
    "begin",
    "stop",
    "restore",
    "restore",
    "end",
    "close",
  ]);
  assert.equal(fixture.cold.isClosed(), true);
});

test("missing MJPEG byte progress fails preservation even with a decoded image", async () => {
  const fixture = scenarioFixture({ endedStream: true });
  await assert.rejects(fixture.scenario(fixture.args), /MJPEG progress failed/);
  assert.equal(fixture.events.includes("stop"), false);
});

test("multipart completion is not disconnect; later bytes prove progress", async () => {
  const client = new EventEmitter();
  const observer = new MjpegProgress(client);
  client.emit("Network.requestWillBeSent", {
    requestId: "one",
    request: { url: "https://synthetic/api/stream/mjpeg?v=1" },
  });
  client.emit("Network.responseReceived", {
    requestId: "one",
    response: { status: 200 },
  });
  const checkpoint = observer.checkpoint();
  const progress = observer.progress(checkpoint, 100);
  client.emit("Network.loadingFinished", { requestId: "one" });
  client.emit("Network.dataReceived", { requestId: "one", dataLength: 100 });
  await progress;
  const disconnected = observer.disconnected(checkpoint, 100);
  client.emit("Network.loadingFailed", { requestId: "one" });
  await disconnected;
  assert.throws(() => observer.checkpoint());
});

test("MJPEG progress rejects request replacement and stalled streams", async () => {
  const client = new EventEmitter();
  const observer = new MjpegProgress(client);
  const start = (id) => {
    client.emit("Network.requestWillBeSent", {
      requestId: id,
      request: { url: "https://synthetic/api/stream/mjpeg" },
    });
    client.emit("Network.responseReceived", {
      requestId: id,
      response: { status: 200 },
    });
  };
  start("one");
  const progress = observer.progress(observer.checkpoint(), 100);
  start("two");
  await assert.rejects(progress, /request replaced/);
  await assert.rejects(
    observer.progress(observer.checkpoint(), 5),
    /timed out/,
  );
});

test("MJPEG normal EOF differs from the first zero-byte completion", async () => {
  const client = new EventEmitter();
  const observer = new MjpegProgress(client);
  client.emit("Network.requestWillBeSent", {
    requestId: "one",
    request: { url: "https://synthetic/api/stream/mjpeg" },
  });
  client.emit("Network.responseReceived", {
    requestId: "one",
    response: { status: 200 },
  });
  client.emit("Network.loadingFinished", {
    requestId: "one",
    encodedDataLength: 0,
  });
  const checkpoint = observer.checkpoint();
  const progress = observer.progress(checkpoint, 100);
  client.emit("Network.dataReceived", { requestId: "one", dataLength: 200 });
  await progress;
  const closed = observer.disconnected(checkpoint, 100);
  client.emit("Network.loadingFinished", {
    requestId: "one",
    encodedDataLength: 250,
  });
  await closed;
  assert.throws(() => observer.checkpoint(), /stream completed/);
  await assert.rejects(observer.progress(checkpoint, 100), /stream completed/);
});

function controlFixture(timeout = 1000) {
  const socket = new EventEmitter();
  socket.writes = [];
  socket.setEncoding = () => {};
  socket.write = (value, callback) => {
    socket.writes.push(value);
    callback(null);
  };
  socket.destroy = () => socket.emit("close");
  return { socket, control: new StatusOutageControl(socket, timeout) };
}

test("phase channel accepts split canonical replies and one stop only", async () => {
  const { socket, control } = controlFixture();
  const stop = control.request("stop");
  assert.equal(socket.writes[0], '{"v":1,"id":1,"phase":"stop"}\n');
  socket.emit("data", '{"v":1,"id":1,');
  socket.emit("data", '"phase":"stop","ok":true}\n');
  await stop;
  assert.throws(() => control.request("stop"));
  const restore = control.request("restore");
  socket.emit("data", '{"v":1,"id":2,"phase":"restore","ok":true}\n');
  await restore;
  control.close();
});

test("phase channel rejects malformed, duplicate, oversize and mismatched replies", async () => {
  for (const reply of [
    '{"v":1,"id":2,"phase":"stop","ok":true}\n',
    '{"v":1,"id":1,"phase":"restore","ok":true}\n',
    '{"v":1,"id":1,"id":1,"phase":"stop","ok":true}\n',
    '{"v":1,"id":1,"phase":"stop","ok":1}\n',
    "{}\n{}\n",
    "x".repeat(257),
  ]) {
    const { socket, control } = controlFixture();
    const request = control.request("stop");
    socket.emit("data", reply);
    await assert.rejects(request, /channel failed/);
    assert.equal(control.closed, true);
  }
});

test("phase failure permits bounded restoration but EOF and timeout fail closed", async () => {
  const { socket, control } = controlFixture();
  assert.throws(() => control.request("restore"));
  const stop = control.request("stop");
  socket.emit("data", '{"v":1,"id":1,"phase":"stop","ok":false}\n');
  await assert.rejects(stop, /not verified/);
  const restore = control.request("restore");
  socket.emit("end");
  await assert.rejects(restore, /channel failed/);
  const expiring = controlFixture(5).control;
  await assert.rejects(expiring.request("stop"), /channel failed/);
});
