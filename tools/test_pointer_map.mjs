// Where the pointer falls in the target's picture (web/src/lib/pointer-map.ts,
// docs/design/screen-upscaling-v1.md): the views state the picture's size on
// the screen element, and the mapping uses it before what the element holds,
// so that an upscaled canvas, whose buffer has the drawn size and is capped,
// maps the pointer as the picture is laid out. The file is TypeScript, which
// Node reads by stripping its types; where this Node cannot, the tests are
// skipped and say so.
import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

const readsTypeScript = Boolean(process.features.typescript);
const skip = readsTypeScript
  ? false
  : "this Node cannot read TypeScript (process.features.typescript)";
const map = readsTypeScript
  ? await import(
      pathToFileURL(
        path.resolve(import.meta.dirname, "../web/src/lib/pointer-map.ts"),
      ).href
    )
  : null;

/** A canvas as an upscaled view makes it: stated picture, buffer of the drawn size times the pixel ratio, capped. */
function upscaledCanvas(picture, zoom, ratio, cap = [3840, 2160]) {
  const css = { width: picture.width * zoom, height: picture.height * zoom };
  return {
    css,
    element: {
      tagName: "CANVAS",
      dataset: {
        pictureWidth: String(picture.width),
        pictureHeight: String(picture.height),
      },
      width: Math.min(cap[0], Math.round(css.width * ratio)),
      height: Math.min(cap[1], Math.round(css.height * ratio)),
    },
  };
}

test(
  "the pointer lands on the same picture pixel at 100, 150 and 200 % with pixel ratios 1 and 2",
  { skip },
  () => {
    const picture = { width: 1920, height: 1080 };
    for (const zoom of [1, 1.5, 2]) {
      for (const ratio of [1, 2]) {
        const { css, element } = upscaledCanvas(picture, zoom, ratio);
        const rect = { left: 37, top: 12, ...css };
        for (const [px, py] of [
          [0, 0],
          [100, 200],
          [960, 540],
          [1919, 1079],
        ]) {
          const clientX = rect.left + ((px + 0.5) / picture.width) * css.width;
          const clientY = rect.top + ((py + 0.5) / picture.height) * css.height;
          const at = map.pointInPicture(
            clientX,
            clientY,
            rect,
            map.pictureSize(element),
          );
          assert.ok(
            Math.abs(at.x * picture.width - (px + 0.5)) < 1e-9 &&
              Math.abs(at.y * picture.height - (py + 0.5)) < 1e-9,
            `zoom ${zoom}, ratio ${ratio}: pixel ${px},${py} mapped to ${at.x * picture.width},${at.y * picture.height}`,
          );
        }
      }
    }
  },
);

test(
  "a capped buffer of another shape does not move the pointer",
  { skip },
  () => {
    // A 16:10 picture at 200 % and ratio 2: the buffer is capped to 3840x2160,
    // 16:9, but the picture is laid out 16:10 and the pointer maps by that.
    const picture = { width: 1920, height: 1200 };
    const { css, element } = upscaledCanvas(picture, 2, 2);
    assert.deepEqual([element.width, element.height], [3840, 2160]);
    const rect = { left: 0, top: 0, ...css };
    const bottom = map.pointInPicture(
      css.width / 2,
      css.height - 1,
      rect,
      map.pictureSize(element),
    );
    assert.ok(Math.abs(bottom.y - (css.height - 1) / css.height) < 1e-9);
    // By the buffer's own shape it would have been taken as letterboxed and off.
    const byBuffer = map.pointInPicture(css.width / 2, css.height - 1, rect, {
      width: element.width,
      height: element.height,
    });
    assert.ok(byBuffer.y > 1.05, String(byBuffer.y));
  },
);

test(
  "the stated size comes first, then the video's, the image's, the canvas's",
  { skip },
  () => {
    assert.deepEqual(
      map.pictureSize({
        dataset: { pictureWidth: "1920", pictureHeight: "1080" },
        videoWidth: 640,
        videoHeight: 480,
      }),
      { width: 1920, height: 1080 },
    );
    assert.deepEqual(
      map.pictureSize({ dataset: {}, videoWidth: 640, videoHeight: 480 }),
      { width: 640, height: 480 },
    );
    assert.deepEqual(
      map.pictureSize({ dataset: {}, naturalWidth: 800, naturalHeight: 600 }),
      { width: 800, height: 600 },
    );
    assert.deepEqual(
      map.pictureSize({
        dataset: {},
        tagName: "CANVAS",
        width: 320,
        height: 200,
      }),
      { width: 320, height: 200 },
    );
    assert.equal(map.pictureSize({ dataset: {}, tagName: "DIV" }), null);
    assert.deepEqual(map.pictureAttributes({ width: 1920, height: 1080 }), {
      "data-picture-width": 1920,
      "data-picture-height": 1080,
    });
    assert.deepEqual(map.pictureAttributes(null), {
      "data-picture-width": undefined,
      "data-picture-height": undefined,
    });
  },
);

test("without a picture the element's own extent is used", { skip }, () => {
  const at = map.pointInPicture(
    150,
    75,
    { left: 100, top: 50, width: 200, height: 100 },
    null,
  );
  assert.deepEqual(at, { x: 0.25, y: 0.25 });
});
