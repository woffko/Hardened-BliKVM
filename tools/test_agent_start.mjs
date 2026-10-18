// "Start the agent" (web/src/lib/agent-start.ts): Win+R, then a PowerShell line typed at the US layout's positions.
// Every character of the line has a key (none is skipped), the line finds the drive by its label and names the agent,
// and it ends with Enter. The file is TypeScript, which Node reads by stripping its types; where this Node cannot, the
// tests are skipped.
import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

const readsTypeScript = Boolean(process.features.typescript);
const skip = readsTypeScript
  ? false
  : "this Node cannot read TypeScript (process.features.typescript)";
const start = readsTypeScript
  ? await import(
      pathToFileURL(
        path.resolve(import.meta.dirname, "../web/src/lib/agent-start.ts"),
      ).href
    )
  : null;

test("the Run box opens with Win+R", { skip }, () => {
  assert.deepEqual(start.runBoxSteps(), [{ modifier: 0x08, usage: 0x15 }]);
});

const DIGEST =
  "5387551047879ddf4d2d3dac19f65f86ceaa01cb20bdc2fe6d7fe5e4f1eeb929";

test(
  "every character of the line has a key, and Enter ends it",
  { skip },
  () => {
    const command = start.agentStartCommand(DIGEST);
    const steps = start.commandSteps(command);
    assert.equal(steps.length, [...command].length + 1);
    assert.deepEqual(steps.at(-1), { modifier: 0, usage: 0x28 });
    assert.ok(command.length < 260, "the Run box takes 259 characters");
    assert.ok(command.includes("'BLIKVMAGENT'"));
    assert.ok(command.includes("hardened-blikvm-agent.exe"));
    assert.equal(start.AGENT_DRIVE_LABEL.length <= 11, true);
    // Shifted characters carry Shift; '$' is Shift+4, '|' Shift+backslash.
    const at = (character) => steps[command.indexOf(character)];
    assert.deepEqual(at("$"), { modifier: 0x02, usage: 0x21 });
    assert.deepEqual(at("|"), { modifier: 0x02, usage: 0x31 });
    assert.deepEqual(at("p"), { modifier: 0, usage: 0x13 });
  },
);

test(
  "only a file of the board's agent's digest is run, the first one found",
  { skip },
  () => {
    const command = start.agentStartCommand(DIGEST);
    assert.ok(command.includes(`(Get-FileHash $_).Hash -eq '${DIGEST}'`));
    assert.ok(command.includes("|select -f 1|%{&$_}"));
    // The digest's filter comes before anything is run.
    assert.ok(command.indexOf("Get-FileHash") < command.indexOf("&$_"));
    for (const wrong of ["", "x", DIGEST.slice(1), `${DIGEST.slice(1)}'`]) {
      assert.throws(() => start.agentStartCommand(wrong), wrong);
    }
  },
);
