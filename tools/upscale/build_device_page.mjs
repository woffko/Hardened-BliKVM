// Builds the self-contained device page of phase 0 (docs/baselines/work-plan-20261008-screen-upscaling.md): the
// cost bench for the operator's phone and desktop, and the calibration picture the target shows for the real pair.
// The bench modules are inlined, the pinned Anime4K sources embedded (with their MIT notice, which they carry), and
// the calibration picture embedded as a PNG. A lab tool, never shipped.
//
//   node tools/upscale/build_device_page.mjs --third-party DIR --calibration PNG --out HTML [--make-calibration]
//
// --make-calibration first writes PNG: the synthetic picture at 2560x1440 as headless Chromium draws it. Keep that
// file: it is the original the captured frame is measured against.
import { readFile, readdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { launchChromium } from "../playwright/quality_fixture.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const option = (name) => {
  const at = args.indexOf(name);
  return at >= 0 ? args[at + 1] : undefined;
};
const thirdParty = option("--third-party");
const calibration = option("--calibration");
const out = option("--out");
if (!thirdParty || !calibration || !out) {
  throw new Error("usage: build_device_page.mjs --third-party DIR --calibration PNG --out HTML [--make-calibration]");
}

/** A module's text without its imports and with its exports made plain declarations. */
async function inline(name) {
  const text = await readFile(path.join(here, name), "utf8");
  return text
    .split("\n")
    .filter((line) => !/^import .* from "\.\/[a-z0-9-]+\.js";$/.test(line))
    .map((line) => line.replace(/^export (?=(const|function|async function|class) )/, ""))
    .join("\n");
}

if (args.includes("--make-calibration")) {
  const patterns = await inline("patterns.js");
  const browser = await launchChromium();
  try {
    const page = await browser.newPage();
    const base64 = await page.evaluate(async (source) => {
      const drawSynthetic = new Function(`${source}; return drawSynthetic;`)();
      const canvas = new OffscreenCanvas(2560, 1440);
      drawSynthetic(canvas.getContext("2d"), 2560, 1440);
      const bytes = new Uint8Array(await (await canvas.convertToBlob({ type: "image/png" })).arrayBuffer());
      let text = "";
      for (let i = 0; i < bytes.length; i += 0x8000) text += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
      return btoa(text);
    }, patterns);
    await writeFile(calibration, Buffer.from(base64, "base64"));
  } finally {
    await browser.close();
  }
}

const anime4kDir = (await readdir(thirdParty)).find((name) => name.startsWith("anime4k-"));
if (!anime4kDir) throw new Error(`no anime4k-* directory in ${thirdParty}`);
const anime4k = {};
for (const variant of ["S", "M"]) {
  anime4k[variant] = await readFile(path.join(thirdParty, anime4kDir, "glsl/Upscale", `Anime4K_Upscale_CNN_x2_${variant}.glsl`), "utf8");
}

const modules = [];
for (const name of ["patterns.js", "kernels.js", "metrics.js", "candidates.js", "bench.js"]) modules.push(await inline(name));
const png = await readFile(calibration);
const template = await readFile(path.join(here, "device-page.template.html"), "utf8");
const page = template
  .replace("/*@BENCH@*/", () => modules.join("\n"))
  .replace('"@ANIME4K@"', () => JSON.stringify(anime4k).replaceAll("</", "<\\/"))
  .replace("@CALIBRATION@", () => `data:image/png;base64,${png.toString("base64")}`);
await writeFile(out, page);
console.log(`${out}: ${(page.length / 1048576).toFixed(2)} MB, calibration ${png.length} bytes`);
