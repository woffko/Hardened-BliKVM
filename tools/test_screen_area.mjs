// Where the captured screen lies in the target's desktop (web/src/lib/screen-area.ts,
// docs/design/screen-area-v1.md): a layout is checked, a position of the picture becomes the
// share of the desktop the absolute report carries, and the target, which reads the range as
// steps over its whole desktop, lands on the captured screen's own pixel. The files are
// TypeScript, which Node reads by stripping its types; where this Node cannot, the tests are
// skipped and say so.
import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

const readsTypeScript = Boolean(process.features.typescript);
const skip = readsTypeScript
  ? false
  : "this Node cannot read TypeScript (process.features.typescript)";
const load = (file) =>
  readsTypeScript
    ? import(
        pathToFileURL(path.resolve(import.meta.dirname, "../web/src/lib", file))
          .href
      )
    : null;
const area = await load("screen-area.ts");
const mouse = await load("mouse.ts");
const MAX = 0x7fff;

/** The target's pixel for a report's value: the range is MAX + 1 steps over the desktop. */
const pixelOf = (value, total) => Math.floor((value * total) / (MAX + 1));

/** Two monitors side by side, the captured one on the left. */
const LEFT = {
  desktop: { width: 5120, height: 1440 },
  screen: { x: 0, y: 0, width: 2560, height: 1440 },
};

test(
  "a layout is whole pixels with the screen inside the desktop",
  { skip },
  () => {
    assert.equal(area.isScreenArea(LEFT), true);
    assert.equal(
      area.isScreenArea({
        desktop: { width: 4480, height: 1440 },
        screen: { x: 2560, y: 360, width: 1920, height: 1080 },
      }),
      true,
    );
    for (const bad of [
      null,
      {},
      { desktop: { width: 5120, height: 1440 } },
      { ...LEFT, screen: { ...LEFT.screen, x: 2561 } },
      { ...LEFT, screen: { ...LEFT.screen, width: 5121 } },
      { ...LEFT, screen: { ...LEFT.screen, y: -1 } },
      { ...LEFT, screen: { ...LEFT.screen, width: 2560.5 } },
      { ...LEFT, desktop: { width: 0, height: 1440 } },
      { ...LEFT, desktop: { width: 40000, height: 1440 } },
      { ...LEFT, screen: { ...LEFT.screen, height: "1440" } },
    ]) {
      assert.equal(area.isScreenArea(bad), false, JSON.stringify(bad));
    }
  },
);

test("without a layout the position is the picture's", { skip }, () => {
  assert.deepEqual(area.mapToDesktop(null, 0, MAX, MAX), { x: 0, y: MAX });
  assert.deepEqual(area.mapToDesktop(null, 12345, 678, MAX), {
    x: 12345,
    y: 678,
  });
});

test(
  "every point of the picture lands on its own pixel of the captured screen",
  { skip },
  () => {
    for (const layout of [
      LEFT,
      // The captured screen on the right, the other one left of it.
      {
        desktop: { width: 5120, height: 1440 },
        screen: { x: 2560, y: 0, width: 2560, height: 1440 },
      },
      // A smaller screen beside a larger one, lower down.
      {
        desktop: { width: 4480, height: 1440 },
        screen: { x: 2560, y: 360, width: 1920, height: 1080 },
      },
    ]) {
      const { screen, desktop } = layout;
      for (let pictureX = 0; pictureX < screen.width; pictureX += 7) {
        // The middle of the picture's pixel, as the page reads the pointer.
        const value = Math.round((MAX * (pictureX + 0.5)) / screen.width);
        const { x } = area.mapToDesktop(layout, value, value, MAX);
        const pixel = pixelOf(x, desktop.width);
        assert.ok(
          Math.abs(pixel - (screen.x + pictureX)) <= 1,
          `${JSON.stringify(layout)}: the picture's pixel ${pictureX} landed on ${pixel}`,
        );
      }
      // The edges stay on the screen, not on its neighbour.
      for (const [value, first, last] of [
        [0, screen.x, screen.y],
        [MAX, screen.x + screen.width - 1, screen.y + screen.height - 1],
      ]) {
        // `first` and `last`: the column and the row the corner lands on.
        const mapped = area.mapToDesktop(layout, value, value, MAX);
        assert.equal(pixelOf(mapped.x, desktop.width), first);
        assert.equal(pixelOf(mapped.y, desktop.height), last);
      }
    }
  },
);

test("the absolute report carries the desktop's position", { skip }, () => {
  let layout = null;
  const report = new mouse.MouseReportAbsolute(() => layout);
  const position = (bytes) => [
    bytes[1] | (bytes[2] << 8),
    bytes[3] | (bytes[4] << 8),
  ];
  assert.deepEqual(position(report.buildReport(MAX, 16383)), [MAX, 16383]);
  layout = LEFT;
  const [x, y] = position(report.buildReport(MAX, Math.round(MAX / 2)));
  // The right edge of the left monitor, half way down.
  assert.equal(pixelOf(x, 5120), 2559);
  assert.equal(pixelOf(y, 1440), 720);
  // The buttons and the wheel are the report's own.
  report.buttonDown(0);
  const bytes = report.buildReport(0, 0, 3);
  assert.equal(bytes[0], 1);
  assert.equal(bytes[5], 3);
  assert.equal(pixelOf(bytes[1] | (bytes[2] << 8), 5120), 0);
});
