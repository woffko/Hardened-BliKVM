// What a page that only watches may ask under /api/hid (tools/playwright/app_harness.mjs): the read of the desktop
// layout the board keeps (docs/design/screen-area-v1.md), which sends nothing to the target, and nothing else; and
// every device probe that fails on a HID request asks that one predicate, so that the page's own read of the layout
// does not fail a probe that only watches.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

const harness = path.resolve(import.meta.dirname, "playwright/app_harness.mjs");
const { watchingMayAsk } = await import(pathToFileURL(harness).href);

// The probes that watch the board and fail on a request under /api/hid.
const PROBES = [
  "engine_busy_device_probe",
  "engine_menu_device_probe",
  "quality_monitor_device_probe",
  "quic_firewall_device_probe",
  "quic_input_device_probe",
  "quic_transport_device_probe",
  "quic_video_device_probe",
  "touch_reach_device_probe",
  "upscale_colour_device_probe",
];

test("a watching page may only read the desktop layout under /api/hid", () => {
  assert.equal(watchingMayAsk("GET", "/api/hid/screen-area"), true);
  for (const [method, route] of [
    ["PUT", "/api/hid/screen-area"],
    ["DELETE", "/api/hid/screen-area"],
    ["POST", "/api/hid/lease"],
    ["GET", "/api/hid/focus"],
    ["POST", "/api/hid/focus"],
    ["GET", "/api/hid/screen-area/x"],
    ["GET", "/api/hid"],
  ])
    assert.equal(watchingMayAsk(method, route), false, `${method} ${route}`);
});

test("every watching probe asks the predicate where it looks for a HID request", () => {
  const asked =
    'route.startsWith("/api/hid") && !watchingMayAsk(request.method(), route)';
  for (const name of PROBES) {
    // The source with its line breaks and indents as single spaces, so that a
    // condition written over two lines reads as one.
    const source = fs
      .readFileSync(
        path.resolve(import.meta.dirname, "playwright", `${name}.mjs`),
        "utf8",
      )
      .replace(/\s+/g, " ");
    const looks = source.split('route.startsWith("/api/hid")').length - 1;
    assert.ok(looks > 0, `${name} looks for no HID request`);
    assert.equal(
      source.split(asked).length - 1,
      looks,
      `${name}: a look for a HID request without the predicate`,
    );
  }
});
