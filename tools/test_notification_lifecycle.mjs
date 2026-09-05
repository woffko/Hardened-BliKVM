import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import ts from "../web/node_modules/typescript/lib/typescript.js";

const source = fs.readFileSync(
  new URL("../web/src/pages/desktop/notification.tsx", import.meta.url),
  "utf8",
);
const { outputText } = ts.transpileModule(source, {
  compilerOptions: {
    target: ts.ScriptTarget.ES2020,
    module: ts.ModuleKind.CommonJS,
    jsx: ts.JsxEmit.ReactJSX,
  },
});
const unhandled = [];
const onUnhandled = (error) => unhandled.push(error);
process.on("unhandledRejection", onUnhandled);
try {
  for (const scenario of [
    "warning",
    "updated",
    "failed",
    "unmounted-success",
    "unmounted-failure",
    "skip",
  ]) {
    let effect;
    let resolve;
    let reject;
    let requests = 0;
    let warnings = 0;
    let skipped = 0;
    const pending = new Promise((yes, no) => {
      resolve = yes;
      reject = no;
    });
    const modules = {
      react: {
        useEffect: (callback) => {
          effect = callback;
        },
      },
      "react/jsx-runtime": {
        jsx: () => null,
        jsxs: () => null,
        Fragment: "fragment",
      },
      antd: {
        Button: "button",
        notification: {
          useNotification: () => [
            {
              warning: () => {
                warnings++;
              },
            },
            null,
          ],
        },
      },
      "react-i18next": { useTranslation: () => ({ t: (value) => value }) },
      "react-router-dom": { useNavigate: () => () => {} },
      "@/api/auth.ts": {
        isPasswordUpdated: () => {
          requests++;
          return pending;
        },
      },
      "@/lib/localstorage.ts": {
        getSkipModifyPassword: () => scenario === "skip",
        setSkipModifyPassword: () => {
          skipped++;
        },
      },
    };
    const context = {
      exports: {},
      require: (name) => {
        assert.ok(name in modules, name);
        return modules[name];
      },
    };
    vm.runInNewContext(outputText, context);
    context.exports.Notification();
    const cleanup = effect();
    if (scenario.startsWith("unmounted")) cleanup();
    if (["failed", "unmounted-failure"].includes(scenario))
      reject(new Error("simulated HTTP 401"));
    else resolve({ code: 0, data: { isUpdated: scenario === "updated" } });
    await new Promise((done) => setImmediate(done));
    assert.equal(warnings, scenario === "warning" ? 1 : 0, scenario);
    assert.equal(requests, scenario === "skip" ? 0 : 1, scenario);
    assert.equal(skipped, 0, scenario);
    assert.deepEqual(unhandled, [], scenario);
    cleanup?.();
  }
} finally {
  process.removeListener("unhandledRejection", onUnhandled);
}
console.log("notification lifecycle: six cases passed");
