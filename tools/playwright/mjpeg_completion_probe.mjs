import assert from "node:assert/strict";
import http from "node:http";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { MjpegProgress } from "./mjpeg_progress.mjs";

const [modulePath, executablePath, closeMode = "navigation"] =
  process.argv.slice(2);
assert.ok(["navigation", "graceful", "swap", "contention"].includes(closeMode));
assert.ok(
  modulePath &&
    executablePath &&
    path.isAbsolute(modulePath) &&
    path.isAbsolute(executablePath),
);
const { chromium } = await import(pathToFileURL(modulePath).href);

const browser = await chromium.launch({
  executablePath,
  headless: true,
  args: ["--no-sandbox"],
});
let server;
try {
  const context = await browser.newContext();
  const page = await context.newPage();
  const devtools = await page.context().newCDPSession(page);
  const observer = new MjpegProgress(devtools, "/stream");
  await devtools.send("Network.enable");
  let streamRequest = null;
  let streamBytes = 0;
  let postFinishedBytes = 0;
  let finished = false;
  let failed = false;
  let stage = "streaming";
  const completions = [];
  devtools.on("Network.requestWillBeSent", (event) => {
    if (new URL(event.request.url).pathname === "/stream")
      streamRequest = event.requestId;
  });
  devtools.on("Network.dataReceived", (event) => {
    if (event.requestId === streamRequest) {
      streamBytes += event.dataLength;
      if (finished) postFinishedBytes += event.dataLength;
    }
  });
  devtools.on("Network.loadingFailed", (event) => {
    if (event.requestId === streamRequest) failed = true;
  });
  devtools.on("Network.loadingFinished", (event) => {
    if (event.requestId === streamRequest)
      completions.push({ stage, bytes: event.encodedDataLength });
  });
  const encoded = await page.evaluate(() => {
    const canvas = document.createElement("canvas");
    canvas.width = canvas.height = 2;
    const context = canvas.getContext("2d");
    return ["red", "blue"].map((color) => {
      context.fillStyle = color;
      context.fillRect(0, 0, 2, 2);
      return canvas.toDataURL("image/jpeg").split(",")[1];
    });
  });
  const frames = encoded.map((frame) => Buffer.from(frame, "base64"));
  let sent = 0;
  let closed = false;
  let streamResponse = null;
  const connections = [];
  let rejectedActiveStreams = null;
  const serveStream = (response) => {
    response.writeHead(200, {
      "Content-Type": "multipart/x-mixed-replace; boundary=frame",
      "Cache-Control": "no-store",
    });
    streamResponse ??= response;
    const connection = { frames: 0, closed: false };
    connections.push(connection);
    const timer = setInterval(() => {
      const frame = frames[sent++ % 2];
      connection.frames += 1;
      response.write(
        `--frame\r\nContent-Type: image/jpeg\r\nContent-Length: ${frame.length}\r\n\r\n`,
      );
      response.write(frame);
      response.write("\r\n");
    }, 100);
    response.on("close", () => {
      connection.closed = true;
      if (response === streamResponse) closed = true;
      clearInterval(timer);
    });
  };
  server = http.createServer((request, response) => {
    if (new URL(request.url, "http://synthetic").pathname !== "/stream") {
      response.writeHead(200, { "Content-Type": "text/html" });
      response.end(
        `<img id="screen" src="${request.url === "/?cold" ? "/stream?client=cold&nonce=0" : "/stream"}">`,
      );
      return;
    }
    if (
      closeMode === "contention" &&
      connections.filter((item) => !item.closed).length >= 2
    ) {
      const deadline = performance.now() + 1000;
      const waiting = setInterval(() => {
        const active = connections.filter((item) => !item.closed).length;
        if (active < 2) {
          clearInterval(waiting);
          serveStream(response);
        } else if (performance.now() >= deadline) {
          clearInterval(waiting);
          rejectedActiveStreams = active;
          response.writeHead(503);
          response.end();
        }
      }, 25);
      response.on("close", () => clearInterval(waiting));
    } else serveStream(response);
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  assert.ok(address && typeof address === "object");
  const responsePromise = page.waitForResponse(
    (response) => new URL(response.url()).pathname === "/stream",
  );
  await page.goto(`http://127.0.0.1:${address.port}`, {
    waitUntil: "domcontentloaded",
  });
  const response = await responsePromise;
  response.finished().then(
    () => {
      finished = true;
    },
    () => {
      finished = true;
    },
  );
  const colors = [];
  for (let index = 0; index < 8; index++) {
    await page.waitForTimeout(150);
    colors.push(
      await page.evaluate(() => {
        const image = document.querySelector("#screen");
        const canvas = document.createElement("canvas");
        canvas.width = canvas.height = 2;
        const context = canvas.getContext("2d");
        context.drawImage(image, 0, 0);
        const pixel = context.getImageData(0, 0, 1, 1).data;
        return pixel[0] > pixel[2] ? "red" : "blue";
      }),
    );
  }
  assert.equal(closed, false);
  assert.ok(sent >= 8);
  assert.equal(new Set(colors).size, 2);
  const checkpoint = observer.checkpoint();
  await observer.progress(checkpoint);
  const streamClosedBeforeNavigation = closed;
  stage = closeMode;
  if (closeMode === "navigation") {
    await page.goto("about:blank");
    await observer.disconnected(checkpoint);
  } else if (closeMode === "graceful") {
    assert.ok(streamResponse);
    streamResponse.end("--frame--\r\n");
    await observer.disconnected(checkpoint);
  } else if (closeMode === "swap") {
    await page.evaluate(() => {
      document.querySelector("#screen").src = "/stream?nonce=1";
    });
  } else {
    const cold = await page.context().newPage();
    await cold.goto(`http://127.0.0.1:${address.port}/?cold`, {
      waitUntil: "domcontentloaded",
    });
    await cold.waitForFunction(
      () => document.querySelector("#screen").naturalWidth > 0,
    );
    const replaced = cold.waitForResponse(
      (item) => new URL(item.url()).searchParams.get("nonce") === "1",
    );
    await cold.evaluate(() => {
      document.querySelector("#screen").src = "/stream?client=cold&nonce=1";
    });
    assert.equal((await replaced).status(), 503);
    assert.equal(rejectedActiveStreams, 2);
    await observer.progress(observer.checkpoint());
    await cold.close();
  }
  await page.waitForTimeout(300);
  console.log(
    JSON.stringify({
      responseFinished: finished,
      streamClosedBeforeNavigation,
      framesSent: sent,
      streamBytes,
      postFinishedBytes,
      decodedColors: [...new Set(colors)],
      streamClosedAfterNavigation: closed,
      loadingFailedAfterNavigation: failed,
      closeMode,
      completions,
      connections,
      rejectedActiveStreams,
    }),
  );
} finally {
  await browser.close();
  if (server) {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
}
