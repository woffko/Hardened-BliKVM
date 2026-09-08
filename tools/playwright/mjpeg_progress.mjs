import assert from "node:assert/strict";

export class MjpegProgress {
  constructor(client, route = "/api/stream/mjpeg") {
    this.current = null;
    this.requests = new Map();
    this.waiters = new Set();
    client.on("Network.requestWillBeSent", ({ requestId, request }) => {
      if (new URL(request.url).pathname !== route) return;
      const value = {
        bytes: 0,
        status: null,
        failed: false,
        firstPartFinished: false,
        completed: false,
      };
      this.requests.set(requestId, value);
      this.current = value;
      if (this.requests.size > 16)
        this.requests.delete(this.requests.keys().next().value);
      this.notify();
    });
    client.on("Network.responseReceived", ({ requestId, response }) => {
      const value = this.requests.get(requestId);
      if (value) {
        value.status = response.status;
        this.notify();
      }
    });
    client.on("Network.dataReceived", ({ requestId, dataLength }) => {
      const value = this.requests.get(requestId);
      if (value && Number.isSafeInteger(dataLength) && dataLength > 0) {
        value.bytes += dataLength;
        this.notify();
      }
    });
    client.on("Network.loadingFailed", ({ requestId }) => {
      const value = this.requests.get(requestId);
      if (value) {
        value.failed = true;
        this.notify();
      }
    });
    client.on("Network.loadingFinished", ({ requestId, encodedDataLength }) => {
      const value = this.requests.get(requestId);
      if (!value) return;
      if (encodedDataLength === 0) value.firstPartFinished = true;
      else if (
        value.firstPartFinished &&
        Number.isSafeInteger(encodedDataLength) &&
        encodedDataLength > 0
      ) {
        value.completed = true;
        this.notify();
      }
    });
    // Chromium emits loadingFinished/Response.finished after the first MJPEG
    // part, while further dataReceived events and decoded frames continue.
    // The first zero-byte completion is not a disconnection. A later positive
    // encoded-length completion represents normal EOF; loadingFailed covers
    // cancellation. Both close paths are exercised by the local fixture.
  }

  notify() {
    for (const check of this.waiters) check();
  }

  checkpoint() {
    assert.ok(this.current, "missing primary video request");
    assert.equal(this.current.status, 200, "primary video did not return 200");
    assert.equal(this.current.failed, false, "primary video request failed");
    assert.equal(
      this.current.completed,
      false,
      "primary video stream completed",
    );
    return { request: this.current, bytes: this.current.bytes };
  }

  wait(check, timeoutMs) {
    return new Promise((resolve, reject) => {
      let timer;
      const evaluate = () => {
        try {
          if (!check()) return;
          clearTimeout(timer);
          this.waiters.delete(evaluate);
          resolve();
        } catch (error) {
          clearTimeout(timer);
          this.waiters.delete(evaluate);
          reject(error);
        }
      };
      timer = setTimeout(() => {
        this.waiters.delete(evaluate);
        reject(new Error("primary video observation timed out"));
      }, timeoutMs);
      this.waiters.add(evaluate);
      evaluate();
    });
  }

  progress(checkpoint, timeoutMs = 5000) {
    return this.wait(() => {
      assert.equal(
        this.current,
        checkpoint.request,
        "primary video request replaced",
      );
      assert.equal(
        checkpoint.request.failed,
        false,
        "primary video request failed",
      );
      assert.equal(
        checkpoint.request.completed,
        false,
        "primary video stream completed",
      );
      return checkpoint.request.bytes > checkpoint.bytes;
    }, timeoutMs);
  }

  disconnected(checkpoint, timeoutMs = 5000) {
    return this.wait(
      () => checkpoint.request.failed || checkpoint.request.completed,
      timeoutMs,
    );
  }
}

export async function observeMjpegProgress(page) {
  const client = await page.context().newCDPSession(page);
  const observer = new MjpegProgress(client);
  await client.send("Network.enable");
  return observer;
}
