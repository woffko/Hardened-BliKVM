// Text typed from the page with the target's layout following its letters (web/src/lib/agent-typing.ts): the text in
// runs of one script, punctuation with the letters around it, and the installed layout each script is typed for. The
// file is TypeScript, which Node reads by stripping its types; where this Node cannot, the tests are skipped.
import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

const readsTypeScript = Boolean(process.features.typescript);
const skip = readsTypeScript
  ? false
  : "this Node cannot read TypeScript (process.features.typescript)";
const typing = readsTypeScript
  ? await import(
      pathToFileURL(
        path.resolve(import.meta.dirname, "../web/src/lib/agent-typing.ts"),
      ).href
    )
  : null;

test("the text in runs of one script", { skip }, () => {
  assert.deepEqual(typing.scriptRuns("Привет, world! Ёж."), [
    { script: "cyrillic", text: "Привет, " },
    { script: "latin", text: "world! " },
    { script: "cyrillic", text: "Ёж." },
  ]);
  assert.deepEqual(typing.scriptRuns("123 abc"), [
    { script: "latin", text: "123 abc" },
  ]);
  assert.deepEqual(typing.scriptRuns("?!"), [{ script: null, text: "?!" }]);
  assert.deepEqual(typing.scriptRuns(""), []);
});

test("each script is typed for its installed layout", { skip }, () => {
  const layout = (id, language) => ({ id, language, name: language });
  const installed = [
    layout("04070407", "de-DE"),
    layout("04190419", "ru-RU"),
    layout("08090809", "en-GB"),
    layout("04090409", "en-US"),
  ];
  assert.equal(typing.layoutForScript(installed, "latin").id, "04090409");
  assert.equal(typing.layoutForScript(installed, "cyrillic").id, "04190419");
  assert.equal(
    typing.layoutForScript(installed.slice(0, 3), "latin").id,
    "08090809",
  );
  // No English layout: Latin letters are left to the layout there is.
  assert.equal(
    typing.layoutForScript([layout("04070407", "de-DE")], "latin"),
    null,
  );
  assert.equal(typing.layoutForScript([], "cyrillic"), null);
});

test(
  "Estonian letters make an Estonian run, Latin letters join it",
  { skip },
  () => {
    assert.deepEqual(typing.scriptRuns("Tänan, hello! Привет"), [
      { script: "estonian", text: "Tänan, hello! " },
      { script: "cyrillic", text: "Привет" },
    ]);
    assert.deepEqual(typing.scriptRuns("ok ÕUN"), [
      { script: "estonian", text: "ok ÕUN" },
    ]);
  },
);

test(
  "Latin text keeps an English or Estonian layout in front",
  { skip },
  () => {
    const layout = (id, language) => ({ id, language, name: language });
    const english = layout("04090409", "en-US");
    const russian = layout("04190419", "ru-RU");
    const estonian = layout("04250425", "et-EE");
    const installed = [english, russian, estonian];
    assert.equal(typing.layoutToType(installed, estonian, "latin"), estonian);
    assert.equal(typing.layoutToType(installed, english, "latin"), english);
    assert.equal(typing.layoutToType(installed, russian, "latin"), english);
    assert.equal(typing.layoutToType(installed, english, "estonian"), estonian);
    assert.equal(typing.layoutToType(installed, null, "cyrillic"), russian);
    assert.equal(typing.layoutToType([english], english, "estonian"), null);
    assert.equal(typing.scriptOfLayout(estonian), "estonian");
    assert.equal(typing.scriptOfLayout(russian), "cyrillic");
    assert.equal(typing.scriptOfLayout(english), "latin");
  },
);
