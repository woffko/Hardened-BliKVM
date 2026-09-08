import assert from "node:assert/strict";
import http from "node:http";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { MjpegProgress } from "./mjpeg_progress.mjs";

const [modulePath, executablePath] = process.argv.slice(2);
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
  const page = await browser.newPage();
  const devtools = await page.context().newCDPSession(page);
  const observer = new MjpegProgress(devtools, "/stream");
  await devtools.send("Network.enable");
  let streamRequest = null;
  let streamBytes = 0;
  let postFinishedBytes = 0;
  let finished = false;
  let failed = false;
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
  server = http.createServer((request, response) => {
    if (request.url !== "/stream") {
      response.writeHead(200, { "Content-Type": "text/html" });
      response.end('<img id="screen" src="/stream">');
      return;
    }
    response.writeHead(200, {
      "Content-Type": "multipart/x-mixed-replace; boundary=frame",
      "Cache-Control": "no-store",
    });
    const timer = setInterval(() => {
      const frame = frames[sent++ % 2];
      response.write(
        `--frame\r\nContent-Type: image/jpeg\r\nContent-Length: ${frame.length}\r\n\r\n`,
      );
      response.write(frame);
      response.write("\r\n");
    }, 100);
    response.on("close", () => {
      closed = true;
      clearInterval(timer);
    });
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
  await page.goto("about:blank");
  await observer.disconnected(checkpoint);
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
    }),
  );
} finally {
  await browser.close();
  if (server) {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
}
