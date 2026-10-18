// The terminal page's serial port (web/src/pages/terminal/validater.ts): the
// port is a /dev path of letters, digits, dot, underscore and hyphen only, and
// the picocom line carries only the options that were given (security review
// of 2026-10-18, LOW-05). The file is TypeScript, which Node reads by
// stripping its types; where this Node cannot, the tests are skipped.
import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

const readsTypeScript = Boolean(process.features.typescript);
const skip = readsTypeScript
  ? false
  : "this Node cannot read TypeScript (process.features.typescript)";
const serial = readsTypeScript
  ? await import(
      pathToFileURL(
        path.resolve(
          import.meta.dirname,
          "../web/src/pages/terminal/validater.ts",
        ),
      ).href
    )
  : null;

const port = (value) => ({
  port: value,
  baud: "115200",
  parity: null,
  flowControl: null,
  dataBits: null,
  stopBits: null,
});

test("a serial device path is accepted", { skip }, () => {
  for (const value of ["/dev/ttyUSB0", "/dev/ttyS1", "/dev/tty.usb-serial_1"]) {
    assert.equal(serial.validatePicocomParameters(port(value)), true, value);
  }
});

test("characters between dot and underscore are refused", { skip }, () => {
  for (const value of [
    "/dev/tty:0",
    "/dev/tty[0]",
    "/dev/tty=x",
    "/dev/tty@a",
    "/dev/a/b",
  ]) {
    assert.equal(serial.validatePicocomParameters(port(value)), false, value);
  }
});

test("the picocom line has only the options given", { skip }, () => {
  assert.equal(
    serial.picocomCommand(port("/dev/ttyUSB0")),
    "picocom /dev/ttyUSB0 --baud 115200",
  );
  assert.equal(
    serial.picocomCommand({
      port: "/dev/ttyUSB0",
      baud: "9600",
      parity: "e",
      flowControl: "n",
      dataBits: "7",
      stopBits: "2",
    }),
    "picocom /dev/ttyUSB0 --baud 9600 --parity e --flow n --databits 7 --stopbits 2",
  );
  assert.doesNotMatch(serial.picocomCommand(port("/dev/ttyUSB0")), /null/);
});
