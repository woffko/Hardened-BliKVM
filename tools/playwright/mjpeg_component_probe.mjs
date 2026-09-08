import assert from "node:assert/strict";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { pathToFileURL } from "node:url";
import { build } from "../../web/node_modules/vite/dist/node/index.js";
import ts from "../../web/node_modules/typescript/lib/typescript.js";

const [modulePath, executablePath, baseline] = process.argv.slice(2);
assert.ok(modulePath && executablePath);
assert.ok(baseline === undefined || baseline === "baseline");
const componentPath = "web/src/pages/desktop/screen/mjpeg.tsx";
const source = baseline
  ? execFileSync("git", ["show", `HEAD:${componentPath}`], { encoding: "utf8" })
  : fs.readFileSync(componentPath, "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: {
    jsx: ts.JsxEmit.ReactJSX,
    target: ts.ScriptTarget.ES2022,
    module: ts.ModuleKind.ESNext,
  },
}).outputText;
const mocks = new Map([
  [
    "jotai",
    `const setScale=()=>{};export const useAtomValue=(key)=>window.fixture[key];export const useAtom=(key)=>[window.fixture[key],setScale];`,
  ],
  [
    "@/api/stream.ts",
    `export const stopFrameDetect=(seconds)=>{window.stops.push(seconds);return Promise.resolve();};`,
  ],
  [
    "@/lib/localstorage.ts",
    `export const getFrameDetect=()=>{window.effects++;return window.frameDetect;};export const getVideoScale=()=>undefined;`,
  ],
  ["@/lib/service.ts", `export const getBaseUrl=()=>location.origin;`],
  ["@/jotai/capabilities.ts", `export const capabilitiesAtom='capabilities';`],
  ["@/jotai/mouse.ts", `export const mouseStyleAtom='mouseStyle';`],
  [
    "@/jotai/screen.ts",
    `export const resolutionAtom='resolution';export const videoScaleAtom='videoScale';`,
  ],
  [
    "./use-screen-fit.ts",
    `export const useScreenFit=()=>({containerRef:undefined,mediaStyle:{}});`,
  ],
]);
const entry = `import React from 'react';import {createRoot} from 'react-dom/client';import {Mjpeg} from 'mjpeg-under-test';
window.effects=0;window.stops=[];window.frameDetect=true;
window.fixture={resolution:{width:0,height:0},capabilities:{video:{status:{availability:'read_only'}}},mouseStyle:'',videoScale:''};
const root=createRoot(document.querySelector('#root'));
window.render=change=>{Object.assign(window.fixture,change);root.render(React.createElement(React.StrictMode,null,React.createElement(Mjpeg)));};
window.render({});`;
const result = await build({
  root: path.resolve("web"),
  configFile: false,
  logLevel: "silent",
  define: { "process.env.NODE_ENV": '"production"' },
  plugins: [
    {
      name: "isolated-mjpeg-component-fixture",
      enforce: "pre",
      resolveId(id) {
        if (
          id === "mjpeg-under-test" ||
          id === "mjpeg-test-entry" ||
          mocks.has(id)
        )
          return "\0" + id;
      },
      load(id) {
        if (id === "\0mjpeg-under-test") return compiled;
        if (id === "\0mjpeg-test-entry") return entry;
        return mocks.get(id.slice(1));
      },
    },
  ],
  build: {
    write: false,
    minify: false,
    rollupOptions: {
      input: "mjpeg-test-entry",
      output: { codeSplitting: false },
    },
  },
});
const outputs = Array.isArray(result) ? result : [result];
const chunks = outputs
  .flatMap((output) => output.output)
  .filter((item) => item.type === "chunk");
assert.equal(chunks.length, 1);
const bundle = chunks[0].code;
const { chromium } = await import(pathToFileURL(modulePath).href);
const browser = await chromium.launch({
  executablePath,
  headless: true,
  args: ["--no-sandbox"],
});
let server;
try {
  const page = await browser.newPage();
  page.setDefaultTimeout(5000);
  page.on("pageerror", (error) =>
    console.error("local fixture page error:", error.message),
  );
  const jpeg = Buffer.from(
    await page.evaluate(() => {
      const canvas = document.createElement("canvas");
      canvas.width = canvas.height = 2;
      const context = canvas.getContext("2d");
      context.fillStyle = "red";
      context.fillRect(0, 0, 2, 2);
      return canvas.toDataURL("image/jpeg").split(",")[1];
    }),
    "base64",
  );
  let requests = 0;
  server = http.createServer((request, response) => {
    const route = new URL(request.url, "http://synthetic").pathname;
    if (route === "/entry.js") {
      response.writeHead(200, { "Content-Type": "application/javascript" });
      response.end(bundle);
      return;
    }
    if (route !== "/api/stream/mjpeg") {
      response.writeHead(200, { "Content-Type": "text/html" });
      response.end(
        '<div id="root"></div><script type="module" src="/entry.js"></script>',
      );
      return;
    }
    requests++;
    response.writeHead(200, {
      "Content-Type": "multipart/x-mixed-replace; boundary=frame",
      "Cache-Control": "no-store",
    });
    const timer = setInterval(() => {
      response.write(
        `--frame\r\nContent-Type: image/jpeg\r\nContent-Length: ${jpeg.length}\r\n\r\n`,
      );
      response.write(jpeg);
      response.write("\r\n");
    }, 100);
    response.on("close", () => clearInterval(timer));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  assert.ok(address && typeof address === "object");
  await page.goto(`http://127.0.0.1:${address.port}`, {
    waitUntil: "domcontentloaded",
  });
  await page.waitForFunction(
    () =>
      window.effects > 0 && document.querySelector("#screen")?.naturalWidth > 0,
  );
  assert.equal(
    await page.locator("#screen").getAttribute("src"),
    `http://127.0.0.1:${address.port}/api/stream/mjpeg?v=0`,
  );
  assert.equal(requests, 1, "mount must start exactly one stream");
  assert.deepEqual(await page.evaluate(() => window.stops), []);
  await page.evaluate(() =>
    window.render({ resolution: { width: 1920, height: 1080 } }),
  );
  await page.waitForFunction(() =>
    document.querySelector("#screen").src.endsWith("?v=1"),
  );
  await page.evaluate(() =>
    window.render({
      capabilities: { video: { status: { availability: "read_write" } } },
    }),
  );
  await page.waitForFunction(() =>
    document.querySelector("#screen").src.endsWith("?v=2"),
  );
  assert.deepEqual(await page.evaluate(() => window.stops), [10]);
  await page.evaluate(() =>
    document.querySelector("#screen").dispatchEvent(new Event("error")),
  );
  await page.waitForFunction(
    () => !document.querySelector("#screen").getAttribute("src"),
  );
  await page.evaluate(() =>
    window.render({ resolution: { width: 1920, height: 1080 } }),
  );
  await page.waitForFunction(() =>
    document.querySelector("#screen").src.endsWith("?v=3"),
  );
  assert.deepEqual(await page.evaluate(() => window.stops), [10, 10]);
  console.log(
    JSON.stringify({
      initialRequests: 1,
      resolutionReconnect: true,
      writabilityReconnect: true,
      errorReset: true,
      frameDetectPreserved: true,
    }),
  );
} finally {
  await browser.close();
  if (server) {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
}
