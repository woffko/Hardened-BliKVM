// The lab tool of the hardware H.264 plan, without an engine: the streams
// its software stand-in makes through the register plans of both open maps,
// and of the form the vendor's library uses for the newer generation, must
// decode in Chromium, at every size the lab uses, to the very samples
// of the pictures they were made from. That proves the stream's headers,
// the plans' use of the bit writer, and the picture generator, before any
// of them meets hardware.
//
// The stand-in codes a key frame as I_PCM macroblocks (the samples as they
// are) and any other frame as skipped macroblocks (the frame before it), so
// every decoded frame must equal the last key frame's picture exactly.
//
// Usage: BLI_KVM_VE_LAB=<the tool> node ve_standin_contract.mjs
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";

const tool = process.env.BLI_KVM_VE_LAB;
assert.ok(tool && fs.existsSync(tool), "BLI_KVM_VE_LAB names the lab tool");

const { chromium } = await import(
  pathToFileURL(
    process.env.PLAYWRIGHT_MODULE ??
      path.join(
        os.homedir(),
        ".local/share/playwright/node_modules/playwright/index.mjs",
      ),
  ).href
);

const SIZES = ["320x240", "1280x720", "1366x768", "1920x1080"];
const MAPS = ["older", "v3s", "h6"];
const FRAMES = 5;
const KEY_EVERY = 3;

const directory = fs.mkdtempSync(path.join(os.tmpdir(), "ve-standin-"));
const cases = [];
for (const [index, size] of SIZES.entries()) {
  // A different sequence at every size, and every map at each.
  const sequence = ["scroll", "drag", "motion", "pointer"][index];
  const source = `${sequence}-${size}.nv12`;
  execFileSync(tool, [
    "generate",
    sequence,
    size,
    String(FRAMES),
    path.join(directory, source),
  ]);
  for (const map of MAPS) {
    const stream = `${map}-${sequence}-${size}.h264`;
    const summary = JSON.parse(
      execFileSync(tool, [
        "standin",
        map,
        sequence,
        size,
        String(FRAMES),
        String(KEY_EVERY),
        path.join(directory, stream),
      ]).toString(),
    );
    assert.equal(summary.frames.length, FRAMES);
    assert.deepEqual(
      summary.frames.map((frame) => frame.key),
      [true, false, false, true, false],
    );
    cases.push({ map, sequence, size, source, stream, summary });
  }
}
// The two maps write the same bytes through different register traffic.
// The newer generation's stream differs in one field of its PPS: it has no
// chroma QP offset.
for (const size of SIZES) {
  const [older, v3s, h6] = MAPS.map((map) =>
    fs.readFileSync(
      path.join(
        directory,
        cases.find((entry) => entry.map === map && entry.size === size).stream,
      ),
    ),
  );
  assert.ok(older.equals(v3s), `${size}: both maps make the same stream`);
  assert.ok(!older.equals(h6), `${size}: the newer form has its own PPS`);
  assert.equal(h6.length, older.length);
}

const server = http.createServer((request, response) => {
  const name = decodeURIComponent(request.url.slice(1));
  if (name === "") {
    response.writeHead(200, { "Content-Type": "text/html" });
    response.end("<!doctype html><title>stand-in</title>");
    return;
  }
  const file = path.join(directory, path.basename(name));
  if (!fs.existsSync(file)) {
    response.writeHead(404);
    response.end();
    return;
  }
  response.writeHead(200, { "Content-Type": "application/octet-stream" });
  fs.createReadStream(file).pipe(response);
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const origin = `http://127.0.0.1:${server.address().port}`;

const browser = await chromium.launch({
  headless: true,
  executablePath:
    process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH ??
    path.join(
      os.homedir(),
      ".cache/ms-playwright/chromium-1228/chrome-linux64/chrome",
    ),
});

/// In the page: decode the stream and compare every frame with the picture
/// of the last key frame before it.
async function decodeAndCompare({ stream, source, summary }) {
  const [streamBytes, sourceBytes] = await Promise.all(
    [stream, source].map(
      async (name) =>
        new Uint8Array(await (await fetch(`/${name}`)).arrayBuffer()),
    ),
  );
  const { width, height, codec, frames } = summary;
  const frameBytes = (width * height * 3) / 2;
  const support = await VideoDecoder.isConfigSupported({ codec });
  if (!support.supported) return { error: `${codec} is not supported` };
  const decoded = [];
  let failure = null;
  const decoder = new VideoDecoder({
    output: (frame) => decoded.push(frame),
    error: (error) => {
      failure = String(error);
    },
  });
  decoder.configure({ codec, optimizeForLatency: true });
  let offset = 0;
  for (const [index, frame] of frames.entries()) {
    decoder.decode(
      new EncodedVideoChunk({
        type: frame.key ? "key" : "delta",
        timestamp: index * 33333,
        data: streamBytes.subarray(offset, offset + frame.bytes),
      }),
    );
    offset += frame.bytes;
  }
  if (offset !== streamBytes.length)
    return { error: "the sizes do not add up" };
  await decoder.flush().catch((error) => {
    failure = failure ?? String(error);
  });
  if (failure) return { error: failure };
  const results = [];
  let key = 0;
  for (const [index, frame] of decoded.entries()) {
    if (frames[index].key) key = index;
    const expected = sourceBytes.subarray(
      key * frameBytes,
      (key + 1) * frameBytes,
    );
    const buffer = new Uint8Array(frame.allocationSize());
    const layout = await frame.copyTo(buffer);
    const result = {
      format: frame.format,
      width: frame.displayWidth,
      height: frame.displayHeight,
      lumaDifferences: 0,
      chromaDifferences: 0,
    };
    for (let y = 0; y < height; y += 1)
      for (let x = 0; x < width; x += 1)
        if (
          buffer[layout[0].offset + y * layout[0].stride + x] !==
          expected[y * width + x]
        )
          result.lumaDifferences += 1;
    // The source is NV12; the decoder gives planar or NV12.
    const chroma = (plane, x, y) =>
      frame.format === "NV12"
        ? buffer[layout[1].offset + y * layout[1].stride + 2 * x + plane]
        : buffer[layout[1 + plane].offset + y * layout[1 + plane].stride + x];
    for (let y = 0; y < height / 2; y += 1)
      for (let x = 0; x < width / 2; x += 1)
        for (const plane of [0, 1])
          if (
            chroma(plane, x, y) !==
            expected[width * height + y * width + 2 * x + plane]
          )
            result.chromaDifferences += 1;
    results.push(result);
    frame.close();
  }
  decoder.close();
  return { results };
}

try {
  const page = await browser.newPage();
  await page.goto(`${origin}/`);
  for (const entry of cases) {
    const label = `${entry.map} ${entry.sequence} ${entry.size}`;
    const outcome = await page.evaluate(decodeAndCompare, entry);
    assert.equal(outcome.error, undefined, `${label}: ${outcome.error}`);
    assert.equal(outcome.results.length, FRAMES, `${label}: every frame`);
    const [width, height] = entry.size.split("x").map(Number);
    for (const [index, result] of outcome.results.entries()) {
      assert.equal(result.width, width, `${label}: frame ${index} width`);
      assert.equal(result.height, height, `${label}: frame ${index} height`);
      assert.equal(
        result.lumaDifferences + result.chromaDifferences,
        0,
        `${label}: frame ${index} differs (${JSON.stringify(result)})`,
      );
    }
  }
  console.log(
    `ve stand-in contract: ok (${cases.length} streams, ${FRAMES} frames each)`,
  );
} finally {
  await browser.close();
  server.close();
  fs.rmSync(directory, { recursive: true, force: true });
}
