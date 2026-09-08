import assert from "node:assert/strict";

// A dedicated inherited Unix socket carries only public phase messages.
// Password stdin and browser output are never multiplexed onto this channel.
export class StatusOutageControl {
  constructor(socket, timeoutMs = 75_000) {
    assert.ok(
      Number.isInteger(timeoutMs) && timeoutMs > 0 && timeoutMs <= 75_000,
    );
    this.socket = socket;
    this.timeoutMs = timeoutMs;
    this.nextId = 1;
    this.restores = 0;
    this.pending = null;
    this.buffer = "";
    this.closed = false;
    socket.setEncoding("utf8");
    socket.on("data", (data) => this.receive(data));
    socket.on("end", () => this.fail());
    socket.on("error", () => this.fail());
    socket.on("close", () => this.fail());
  }

  fail() {
    if (this.closed) return;
    this.closed = true;
    if (this.pending) {
      clearTimeout(this.pending.timer);
      this.pending.reject(new Error("status outage control channel failed"));
      this.pending = null;
    }
    this.socket.destroy();
  }

  receive(data) {
    if (this.closed) return;
    this.buffer += data;
    if (Buffer.byteLength(this.buffer) > 256) return this.fail();
    if (!this.buffer.includes("\n")) return;
    const pending = this.pending;
    if (
      !pending ||
      !this.buffer.endsWith("\n") ||
      this.buffer.indexOf("\n") !== this.buffer.length - 1
    ) {
      return this.fail();
    }
    const good =
      JSON.stringify({ v: 1, id: pending.id, phase: pending.phase, ok: true }) +
      "\n";
    const bad =
      JSON.stringify({
        v: 1,
        id: pending.id,
        phase: pending.phase,
        ok: false,
      }) + "\n";
    if (this.buffer !== good && this.buffer !== bad) return this.fail();
    const accepted = this.buffer === good;
    this.buffer = "";
    this.pending = null;
    clearTimeout(pending.timer);
    if (accepted) pending.resolve();
    else pending.reject(new Error("status outage phase was not verified"));
  }

  request(phase) {
    assert.equal(this.closed, false, "status outage control channel is closed");
    assert.equal(this.pending, null, "status outage command already pending");
    assert.ok(phase === "stop" || phase === "restore");
    if (phase === "stop")
      assert.equal(this.nextId, 1, "stop may run only once");
    else {
      assert.ok(
        this.nextId > 1 && this.restores < 2,
        "restore order/count mismatch",
      );
      this.restores += 1;
    }
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending = {
        id,
        phase,
        resolve,
        reject,
        timer: setTimeout(() => this.fail(), this.timeoutMs),
      };
      this.socket.write(JSON.stringify({ v: 1, id, phase }) + "\n", (error) => {
        if (error) this.fail();
      });
    });
  }

  close() {
    this.fail();
  }
}
