// The share of the bitrate a session's path carries
// (docs/design/quic-transport-v1.md, "The bitrate"), through the built page,
// the host's build of the gateway, a played server (quic_fixture.mjs) that
// does with the share what the board's server does, and the encoder's own
// x264 engine behind a frame socket. A UDP relay
// (tools/net/udp_impair_proxy.mjs) stands between the page and the gateway,
// and narrows the path when told to.
//
// On a path that carries the picture, the session asks for no less than the
// whole bitrate. Narrowed to half the picture's rate, the page falls behind
// and the session asks for less, a quarter at a time, until the path
// carries what the encoder makes: the page then keeps up, and the picture
// goes on whole at the lower bitrate. Wide again, the path gets more.
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { startUdpProxy } from "../net/udp_impair_proxy.mjs";
import { startApp } from "./app_harness.mjs";
import {
  Board,
  launch,
  now,
  openPage,
  openSettings,
  sleep,
  startFrameSource,
  until,
} from "./quic_fixture.mjs";
import { freePort } from "./quic_stack_rig.mjs";

/** The source: its rate, size and chosen bitrate. */
const FPS = 30;
const WHOLE_FPS = 24;
const KBPS = 800;
/** How long the narrow path's queue may hold a datagram, as a mobile one. */
const QUEUE_MS = 1000;
/** A picture that keeps up: its oldest frame younger than this. */
const KEPT_UP_MS = 1000;

const browser = await launch();
const app = await startApp();
const PORT = Number(new URL(app.baseUrl).port);
const directory = fs.mkdtempSync(path.join(os.tmpdir(), "quic-share-"));
const frameSocket = path.join(directory, "frames.sock");
const source = await startFrameSource(frameSocket, {
  width: 1280,
  height: 720,
  fps: FPS,
  kbps: KBPS,
});
const gatewayPort = await freePort();
const relay = await startUdpProxy({
  listen: { host: "127.0.0.1", port: PORT },
  target: { host: "127.0.0.1", port: gatewayPort },
  queueMs: QUEUE_MS,
  seed: 20261009,
});
const board = new Board(PORT, { gatewayPort, frameSocket });
// As the board's server does: the encoder runs at the chosen bitrate times
// the least share of the sessions that watch (here, one).
board.onShare = (_session, percent) => source.setKbps((KBPS * percent) / 100);
await board.listen();
board.enabled = true;
board.startGateway();
await board.gatewayOnTheLink();

/** The picture's counters as its canvas carries them. */
const stats = (page) =>
  page.evaluate(() => {
    const canvas = document.querySelector(
      'canvas#screen[data-video-transport="quic"]',
    );
    if (!canvas || canvas.dataset.quicFrames === undefined) return null;
    return {
      frames: Number(canvas.dataset.quicFrames),
      fps: Number(canvas.dataset.quicFps),
      ageP95: canvas.dataset.quicAgeP95
        ? Number(canvas.dataset.quicAgeP95)
        : null,
    };
  });
/** The share the gateway's counters give, as Settings has it. */
const counted = (page) =>
  page
    .locator('[data-testid="transport-status"]')
    .getAttribute("data-bitrate-share")
    .then(Number);
const asked = () => board.shares.map((share) => share.percent);
/** The frames drawn a second over a span, and the oldest frame's age. */
async function watch(page, ms) {
  const before = await stats(page);
  const started = now();
  let worstAge = 0;
  while (now() - started < ms) {
    await sleep(500);
    worstAge = Math.max(worstAge, (await stats(page)).ageP95 ?? 0);
  }
  const after = await stats(page);
  return {
    fps: (after.frames - before.frames) / ((now() - started) / 1000),
    worstAge,
  };
}

try {
  const { context, page, errors } = await openPage(browser, app, board, {
    choice: "quic",
    videoMode: "h264",
  });
  await until("the picture comes over the session", async () => {
    const seen = await stats(page);
    return seen && seen.frames >= 60 ? seen : null;
  });
  await openSettings(page);

  // A path that carries the picture: the whole bitrate. What x264 makes of
  // the simple test picture is below its target; the path's own count says
  // how much.
  const bytesBefore = relay.counters().toClient.bytes;
  const wideStarted = now();
  const wide = await watch(page, 12_000);
  const wideKbps =
    ((relay.counters().toClient.bytes - bytesBefore) * 8) /
    (now() - wideStarted);
  assert.deepEqual(asked(), [], "less was asked of a wide path");
  assert.equal(await counted(page), 100);
  assert.ok(wide.fps >= WHOLE_FPS, `${wide.fps.toFixed(1)} frames a second`);

  // Narrowed to half the picture's rate: less is asked for, a quarter at a
  // time, until the page keeps up with what the encoder makes.
  const narrowKbps = Math.round(wideKbps / 2);
  relay.impair({ rateKbps: narrowKbps });
  const narrowed = now();
  await until(
    "the session asks for less",
    () => (asked().length ? true : null),
    20_000,
  );
  let kept;
  await until(
    "the page keeps up on the narrow path",
    async () => {
      kept = await watch(page, 6000);
      return kept.fps >= WHOLE_FPS && kept.worstAge < KEPT_UP_MS ? true : null;
    },
    90_000,
  );
  const keptAfter = (now() - narrowed) / 1000;
  // A quarter less at a time, and once the page has kept up for ten seconds
  // a fifth more; never below a quarter of the whole.
  const steps = asked();
  for (const [index, percent] of steps.entries()) {
    const before = index === 0 ? 100 : steps[index - 1];
    assert.ok(
      percent === Math.max(25, Math.floor((before * 3) / 4)) ||
        percent === Math.min(100, Math.floor((before * 6) / 5)),
      `the session asked for ${steps}`,
    );
  }
  const lowest = Math.min(...steps);
  assert.ok(lowest <= 56, `the session asked for ${steps}`);
  await until("the counters give the share", async () =>
    (await counted(page)) === asked().at(-1) ? true : null,
  );
  const overflowed = relay.counters().toClient.overflowed;

  // Wide again: within half a minute the session asks for more.
  const before = asked().at(-1);
  relay.impair({ rateKbps: 0 });
  await until(
    "the session asks for more",
    () => (asked().at(-1) > before ? true : null),
    30_000,
  );
  assert.deepEqual(errors, []);
  console.log(
    `QUIC share: the whole bitrate on a wide path (${wideKbps.toFixed(0)} kbit/s, ${wide.fps.toFixed(1)} frames a second); ` +
      `narrowed to ${narrowKbps} kbit/s (${overflowed} datagrams overflowed its queue), ` +
      `the session asked for ${steps.join(", ")} % and the page kept up after ${keptAfter.toFixed(1)} s, ` +
      `at ${kept.fps.toFixed(1)} frames a second, the oldest ${kept.worstAge} ms old; ` +
      `wide again, it asked for ${asked().at(-1)} %`,
  );
  await context.close();
  console.log("quic share contract: ok");
} catch (error) {
  if (board.stderr) console.error(`gateway:\n${board.stderr}`);
  throw error;
} finally {
  await browser.close();
  await board.close();
  await relay.close();
  await source.stop();
  app.server.close();
  fs.rmSync(directory, { recursive: true, force: true });
}
