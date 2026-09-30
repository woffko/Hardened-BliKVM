import assert from "node:assert/strict";
import test from "node:test";

import { commandFailed, commandReason } from "../web/src/lib/command-result.ts";

test("a refused command is a failure even though the call succeeded", () => {
  assert.equal(commandFailed({ code: 0, data: { applied: true } }), false);
  assert.equal(commandFailed({ code: 0, data: null }), false);
  assert.equal(commandFailed({ code: 0 }), false);
  assert.equal(commandFailed({ code: -1, msg: "x" }), true);
  assert.equal(
    commandFailed({ code: 0, data: { applied: false, rejection: "busy" } }),
    true,
  );
});

test("the reason prefers the message, then the owner's rejection", () => {
  assert.equal(commandReason({ code: -1, msg: "denied" }), "denied");
  assert.equal(
    commandReason({ code: 0, data: { applied: false, rejection: "busy" } }),
    "busy",
  );
  assert.equal(commandReason({ code: 0 }), "");
});
