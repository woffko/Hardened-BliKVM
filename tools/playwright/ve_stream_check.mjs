// A stream from the lab of the hardware H.264 plan, checked on the host:
// Chromium decodes it with WebCodecs, and every decoded frame is compared
// with the picture the lab tool's generator makes for that frame.
//
// Usage: BLI_KVM_VE_LAB=<the host's lab tool> node ve_stream_check.mjs \
//          STREAM SEQUENCE WIDTHxHEIGHT [OUTPUT_JSON]
// The stream is Annex B, one slice per frame, parameter sets before key
// frames. The number of frames is read from the stream.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";

const [streamPath, sequence, size, outputPath] = process.argv.slice(2);
assert.ok(streamPath && sequence && /^\d+x\d+$/.test(size ?? ""), "usage");
const tool = process.env.BLI_KVM_VE_LAB;
assert.ok(tool && fs.existsSync(tool), "BLI_KVM_VE_LAB names the lab tool");
const [width, height] = size.split("x").map(Number);

/// The frames of an Annex B stream: where each begins, and whether it is a
/// key frame. A frame begins at a parameter set, or at a slice that no
/// parameter set of the same frame precedes.
function frames(bytes) {
  const units = [];
  for (let at = 0; at + 4 <= bytes.length; at += 1) {
    if (
      bytes[at] === 0 &&
      bytes[at + 1] === 0 &&
      bytes[at + 2] === 0 &&
      bytes[at + 3] === 1
    ) {
      units.push({ at, type: bytes[at + 4] & 0x1f });
      at += 3;
    }
  }
  const found = [];
  let open = null;
  for (const unit of units) {
    const slice = unit.type === 1 || unit.type === 5;
    if (open === null) open = { at: unit.at, key: false, profile: null };
    if (unit.type === 7 && open.profile === null)
      open.profile = [...bytes.subarray(unit.at + 5, unit.at + 8)];
    if (slice) {
      open.key = unit.type === 5;
      found.push(open);
      open = null;
    }
  }
  return found.map((frame, index) => ({
    ...frame,
    bytes:
      (index + 1 < found.length ? found[index + 1].at : bytes.length) -
      frame.at,
  }));
}

const stream = fs.readFileSync(streamPath);
const list = frames(stream);
assert.ok(list.length > 0, "the stream has no frame");
assert.ok(list[0].key && list[0].profile, "the stream begins with a key frame");
const codec = `avc1.${list[0].profile.map((byte) => byte.toString(16).padStart(2, "0")).join("")}`;

const directory = fs.mkdtempSync(path.join(os.tmpdir(), "ve-stream-"));
const source = path.join(directory, "source.nv12");
execFileSync(tool, ["generate", sequence, size, String(list.length), source]);
fs.copyFileSync(streamPath, path.join(directory, "stream.h264"));

const frameBytes = (width * height * 3) / 2;
const server = http.createServer((request, response) => {
  const name = path.basename(decodeURIComponent(request.url.slice(1)));
  if (name === "") {
    response.writeHead(200, { "Content-Type": "text/html" });
    response.end("<!doctype html><title>stream</title>");
    return;
  }
  // The luma plane of one source picture: the page never holds them all.
  const picture = /^\/luma\/(\d+)$/.exec(request.url);
  if (picture) {
    const luma = Buffer.alloc(width * height);
    const file = fs.openSync(source, "r");
    const read = fs.readSync(file, luma, 0, luma.length, Number(picture[1]) * frameBytes);
    fs.closeSync(file);
    response.writeHead(read === luma.length ? 200 : 404, {
      "Content-Type": "application/octet-stream",
    });
    response.end(read === luma.length ? luma : undefined);
    return;
  }
  const file = path.join(directory, name);
  if (!fs.existsSync(file)) {
    response.writeHead(404);
    response.end();
    return;
  }
  response.writeHead(200, { "Content-Type": "application/octet-stream" });
  fs.createReadStream(file).pipe(response);
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));

const { chromium } = await import(
  pathToFileURL(
    process.env.PLAYWRIGHT_MODULE ??
      path.join(
        os.homedir(),
        ".local/share/playwright/node_modules/playwright/index.mjs",
      ),
  ).href
);
const browser = await chromium.launch({
  headless: true,
  executablePath:
    process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH ??
    path.join(
      os.homedir(),
      ".cache/ms-playwright/chromium-1228/chrome-linux64/chrome",
    ),
});

/// In the page: decode, and give every frame's luma PSNR against the source.
/// A decoded picture is compared and given back before many more are
/// decoded, so a long stream takes no more memory than a short one.
async function decode({ codec, list, width, height }) {
  const streamBytes = new Uint8Array(
    await (await fetch("/stream.h264")).arrayBuffer(),
  );
  const support = await VideoDecoder.isConfigSupported({ codec });
  if (!support.supported) return { error: `${codec} is not supported` };
  const waiting = [];
  let failure = null;
  let wake = () => {};
  const decoder = new VideoDecoder({
    output: (frame) => {
      waiting.push(frame);
      wake();
    },
    error: (error) => {
      failure = String(error);
      wake();
    },
  });
  decoder.configure({ codec, optimizeForLatency: true });
  const results = [];
  async function compare(frame) {
    const index = results.length;
    const buffer = new Uint8Array(frame.allocationSize());
    const layout = await frame.copyTo(buffer);
    const right = frame.displayWidth === width && frame.displayHeight === height;
    let squares = 0;
    if (right) {
      const expected = new Uint8Array(
        await (await fetch(`/luma/${index}`)).arrayBuffer(),
      );
      for (let y = 0; y < height; y += 1) {
        const row = layout[0].offset + y * layout[0].stride;
        for (let x = 0; x < width; x += 1) {
          const difference = buffer[row + x] - expected[y * width + x];
          squares += difference * difference;
        }
      }
    }
    results.push({
      width: frame.displayWidth,
      height: frame.displayHeight,
      psnr:
        squares === 0
          ? null
          : 10 * Math.log10((255 * 255) / (squares / (width * height))),
    });
    frame.close();
  }
  async function drain() {
    while (waiting.length > 0) await compare(waiting.shift());
  }
  for (const [index, frame] of list.entries()) {
    if (failure) break;
    decoder.decode(
      new EncodedVideoChunk({
        type: frame.key ? "key" : "delta",
        timestamp: index * 33333,
        data: streamBytes.subarray(frame.at, frame.at + frame.bytes),
      }),
    );
    while (!failure && (decoder.decodeQueueSize > 4 || waiting.length > 4)) {
      if (waiting.length > 0) await drain();
      else
        await new Promise((resolve) => {
          wake = resolve;
          setTimeout(resolve, 50);
        });
    }
  }
  await decoder.flush().catch((error) => {
    failure = failure ?? String(error);
  });
  await drain();
  if (decoder.state !== "closed") decoder.close();
  return { error: failure, results };
}

let outcome;
try {
  const page = await browser.newPage();
  await page.goto(`http://127.0.0.1:${server.address().port}/`);
  outcome = await page.evaluate(decode, { codec, list, width, height });
} finally {
  await browser.close();
  server.close();
  fs.rmSync(directory, { recursive: true, force: true });
}

const values = outcome.results
  .map((result) => result.psnr)
  .filter((value) => value !== null);
const report = {
  stream: path.basename(streamPath),
  sequence,
  width,
  height,
  codec,
  browser: "Chromium (WebCodecs)",
  frames: list.length,
  keyFrames: list.filter((frame) => frame.key).length,
  bytes: stream.length,
  decoded: outcome.results.length,
  error: outcome.error ?? null,
  sizeRight: outcome.results.every(
    (result) => result.width === width && result.height === height,
  ),
  exact: outcome.results.length - values.length,
  meanDb: values.length
    ? values.reduce((sum, value) => sum + value, 0) / values.length
    : null,
  lowestDb: values.length ? Math.min(...values) : null,
  db: outcome.results.map((result) =>
    result.psnr === null ? "exact" : Math.round(result.psnr * 100) / 100,
  ),
};
if (outputPath)
  fs.writeFileSync(outputPath, `${JSON.stringify(report, null, 1)}\n`);
console.log(JSON.stringify(report));
process.exit(
  report.error === null && report.decoded === report.frames && report.sizeRight
    ? 0
    : 1,
);
