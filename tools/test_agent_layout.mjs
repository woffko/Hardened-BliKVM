// The absolute mouse's layout from the target agent's monitors (web/src/lib/agent-layout.ts): the desktop is the box
// around the monitors, the screen the board's monitor in it, whichever monitor is primary and wherever it lies. The
// file is TypeScript, which Node reads by stripping its types; where this Node cannot, the tests are skipped.
import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

const readsTypeScript = Boolean(process.features.typescript);
const skip = readsTypeScript
  ? false
  : "this Node cannot read TypeScript (process.features.typescript)";
const layout = readsTypeScript
  ? await import(
      pathToFileURL(
        path.resolve(import.meta.dirname, "../web/src/lib/agent-layout.ts"),
      ).href
    )
  : null;

const monitor = (x, y, width, height, board = false) => ({
  name: "",
  maker: "DEL",
  product: 1,
  x,
  y,
  width,
  height,
  refresh_millihertz: 60000,
  primary: x === 0 && y === 0,
  scale: 100,
  board,
});

test("the board's monitor on the right of the primary", { skip }, () => {
  assert.deepEqual(
    layout.areaFromMonitors([
      monitor(0, 0, 2560, 1440),
      monitor(2560, 0, 2560, 1440, true),
    ]),
    {
      desktop: { width: 5120, height: 1440 },
      screen: { x: 2560, y: 0, width: 2560, height: 1440 },
    },
  );
});

test(
  "a monitor left of and above the primary moves the origin",
  { skip },
  () => {
    // The board's monitor is primary; another lies left of it and higher.
    assert.deepEqual(
      layout.areaFromMonitors([
        monitor(-1920, -360, 1920, 1080),
        monitor(0, 0, 2560, 1440, true),
      ]),
      {
        desktop: { width: 4480, height: 1800 },
        screen: { x: 1920, y: 360, width: 2560, height: 1440 },
      },
    );
  },
);

test(
  "one monitor, which is the board's, is the whole desktop",
  { skip },
  () => {
    assert.deepEqual(
      layout.areaFromMonitors([monitor(0, 0, 1920, 1080, true)]),
      {
        desktop: { width: 1920, height: 1080 },
        screen: { x: 0, y: 0, width: 1920, height: 1080 },
      },
    );
  },
);

test(
  "no monitor marked, two marked, or none at all: no layout",
  { skip },
  () => {
    assert.equal(
      layout.areaFromMonitors([
        monitor(0, 0, 2560, 1440),
        monitor(2560, 0, 2560, 1440),
      ]),
      null,
    );
    assert.equal(
      layout.areaFromMonitors([
        monitor(0, 0, 2560, 1440, true),
        monitor(2560, 0, 2560, 1440, true),
      ]),
      null,
    );
    assert.equal(layout.areaFromMonitors([]), null);
    // A desktop wider than the range is not a layout.
    assert.equal(
      layout.areaFromMonitors([
        monitor(-20000, 0, 2560, 1440),
        monitor(20000, 0, 2560, 1440, true),
      ]),
      null,
    );
  },
);
