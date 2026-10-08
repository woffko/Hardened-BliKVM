// The upscaling setting and the rules it is applied by
// (web/src/lib/upscale/settings.ts, docs/design/screen-upscaling-v1.md): what
// the browser keeps, the canvas buffer of a picture shown at a size, and
// when a frame is enlarged sharply or drawn at its own size. The file is
// TypeScript, which Node reads by stripping its types; where this Node
// cannot, the tests are skipped and say so.
import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

const readsTypeScript = Boolean(process.features.typescript);
const skip = readsTypeScript
  ? false
  : "this Node cannot read TypeScript (process.features.typescript)";
const settings = readsTypeScript
  ? await import(
      pathToFileURL(
        path.resolve(import.meta.dirname, "../web/src/lib/upscale/settings.ts"),
      ).href
    )
  : null;

const memory = (value) => {
  const kept = new Map(
    value === undefined ? [] : [["hardened-blikvm-upscale", value]],
  );
  return {
    getItem: (key) => kept.get(key) ?? null,
    setItem: (key, item) => kept.set(key, item),
  };
};

test(
  "the browser keeps the setting, and anything else reads as the default",
  { skip },
  () => {
    const storage = memory();
    assert.deepEqual(settings.readUpscale(storage), settings.DEFAULT_UPSCALE);
    assert.deepEqual(settings.DEFAULT_UPSCALE, {
      mode: "off",
      sharpness: "medium",
    });
    settings.writeUpscale(storage, { mode: "auto", sharpness: "high" });
    assert.deepEqual(settings.readUpscale(storage), {
      mode: "auto",
      sharpness: "high",
    });
    for (const stored of [
      "not json",
      "null",
      '{"mode":"on"}',
      "[]",
      '"auto"',
    ]) {
      assert.deepEqual(
        settings.readUpscale(memory(stored)),
        settings.DEFAULT_UPSCALE,
        stored,
      );
    }
    assert.deepEqual(
      settings.readUpscale(memory('{"mode":"auto","sharpness":"extreme"}')),
      {
        mode: "auto",
        sharpness: "medium",
      },
    );
    const refusing = {
      getItem: () => {
        throw new Error("blocked");
      },
      setItem: () => {
        throw new Error("blocked");
      },
    };
    assert.deepEqual(settings.readUpscale(refusing), settings.DEFAULT_UPSCALE);
    settings.writeUpscale(refusing, { mode: "auto", sharpness: "low" });
  },
);

test(
  "the buffer is the shown device pixels, at most 3840x2160, of the same shape",
  { skip },
  () => {
    assert.deepEqual(settings.bufferSize(1280, 720, 1), {
      width: 1280,
      height: 720,
    });
    assert.deepEqual(settings.bufferSize(1280, 720, 2), {
      width: 2560,
      height: 1440,
    });
    assert.deepEqual(settings.bufferSize(2560, 1440, 2), {
      width: 3840,
      height: 2160,
    });
    // A 16:10 picture capped keeps 16:10.
    assert.deepEqual(settings.bufferSize(1920, 1200, 2), {
      width: 3456,
      height: 2160,
    });
    assert.deepEqual(settings.bufferSize(0.2, 0.2, 1), { width: 1, height: 1 });
  },
);

test(
  "a frame is enlarged sharply only when it is shown more than 1.1 times its size",
  { skip },
  () => {
    // 1920x1080 shown at 2560x1440 CSS pixels on a display of ratio 1: 4/3.
    assert.deepEqual(settings.drawing(1920, 1080, 2560, 1440, 1, "medium"), {
      mode: "medium",
      width: 2560,
      height: 1440,
    });
    // Shown at its size on a display of ratio 2: enlarged twice by the display.
    assert.deepEqual(settings.drawing(1920, 1080, 1920, 1080, 2, "low"), {
      mode: "low",
      width: 3840,
      height: 2160,
    });
    // At most 1.1 times, or smaller: the frame at its own size, plainly.
    for (const [width, ratio] of [
      [2112, 1],
      [1920, 1],
      [960, 1],
      [1056, 2],
    ]) {
      assert.deepEqual(
        settings.drawing(1920, 1080, width, (width * 9) / 16, ratio, "high"),
        { mode: "plain", width: 1920, height: 1080 },
        `${width} at ${ratio}`,
      );
    }
  },
);
