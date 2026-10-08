// Runs the phase-0 quality bench (bench.js) in headless Chromium drawing with SwiftShader, and writes the results
// and the crop sheets to a directory. A lab tool for docs/baselines/work-plan-20261008-screen-upscaling.md.
//
//   node tools/upscale/run_quality.mjs --third-party DIR --out DIR [--quick] [--cost]
//                                      [--reference PNG --frames JPEG,JPEG,...]
//
// DIR of --third-party holds the pinned upstream sources (anime4k-*/glsl/Upscale/*.glsl); --out receives
// quality.json, the crop sheets and, with --cost, cost-swiftshader.json. With --reference (the original as the
// target showed it) and --frames (frames the board captured meanwhile), the frames that show the picture are found,
// the bridge's downscale is fitted (real-pair.json), and the best frame joins the sources as "real".
import { readFile, readdir, mkdir, writeFile } from "node:fs/promises";
import http from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { launchChromium } from "../playwright/quality_fixture.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const option = (name) => {
  const at = args.indexOf(name);
  return at >= 0 ? args[at + 1] : undefined;
};
const thirdParty = option("--third-party") ?? process.env.UPSCALE_THIRD_PARTY;
const out = option("--out");
if (!thirdParty || !out) throw new Error("usage: run_quality.mjs --third-party DIR --out DIR [--quick] [--cost]");
const quick = args.includes("--quick");
const reference = option("--reference");
const frames = option("--frames")?.split(",") ?? [];
// The files the page may fetch besides the bench: the reference and the frames, by index.
const files = [reference, ...frames].filter(Boolean).map((file) => path.resolve(file));

const anime4kDir = (await readdir(thirdParty)).find((name) => name.startsWith("anime4k-"));
const anime4k = {};
if (anime4kDir) {
  for (const variant of ["S", "M"]) {
    anime4k[variant] = await readFile(path.join(thirdParty, anime4kDir, "glsl/Upscale", `Anime4K_Upscale_CNN_x2_${variant}.glsl`), "utf8");
  }
}

const types = { ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript" };
const server = http.createServer(async (request, response) => {
  const pathname = new URL(request.url, "http://localhost").pathname;
  const name = path.basename(pathname) || "bench.html";
  try {
    const file = pathname.startsWith("/files/") ? files[Number(name)] : path.join(here, name);
    if (!file) throw new Error("not served");
    const body = await readFile(file);
    response.writeHead(200, { "content-type": types[path.extname(name)] ?? "application/octet-stream" });
    response.end(body);
  } catch {
    response.writeHead(404);
    response.end();
  }
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const base = `http://127.0.0.1:${server.address().port}`;

const browser = await launchChromium({ args: ["--enable-unsafe-swiftshader", "--use-angle=swiftshader"] });
try {
  const page = await browser.newPage();
  page.on("pageerror", (error) => console.error("pageerror", error.message));
  await page.goto(`${base}/bench.html`);
  await page.waitForFunction(() => window.benchApi);
  await mkdir(out, { recursive: true });

  let real = null;
  let realOriginal = null;
  if (reference && frames.length) {
    const pair = await page.evaluate(
      ({ frames }) => window.benchApi.runRealPair({ reference: "/files/0", frames }),
      { frames: frames.map((_, index) => `/files/${index + 1}`) },
    );
    pair.frames.forEach((entry) => (entry.file = path.basename(frames[Number(entry.url.split("/").pop()) - 1])));
    if (pair.chosen) pair.chosenFile = path.basename(frames[Number(pair.chosen.split("/").pop()) - 1]);
    await mkdir(out, { recursive: true });
    await writeFile(path.join(out, "real-pair.json"), `${JSON.stringify(pair, null, 1)}\n`);
    console.log("frames:", pair.frames.map((f) => `${f.file} ${f.psnr?.toFixed(1) ?? f.size}`).join(", "));
    console.log("chosen:", pair.chosenFile, "levels:", JSON.stringify(pair.levels));
    for (const [kind, fit] of Object.entries(pair.fits)) {
      const regions = Object.entries(fit.regions ?? {}).map(([region, value]) => `${region} ${value.toFixed(2)}`).join(" | ");
      console.log(kind, `${fit.psnr.toFixed(2)} dB with the grid at (${fit.dx}, ${fit.dy}):`, regions);
    }
    if (pair.frameEdges) console.log("rise in the frame", pair.frameEdges.riseMedian.toFixed(3), "area downscale", pair.areaEdges.riseMedian.toFixed(3));
    real = pair.chosen ?? null;
    if (real) {
      // The original as the chain delivered it: the fitted levels, moved by the best fit's grid in whole pixels.
      const [kind, fit] = Object.entries(pair.fits).sort((p, q) => q[1].psnr - p[1].psnr)[0];
      realOriginal = { levels: pair.levels, shift: [Math.round(-fit.dx), Math.round(-fit.dy)], kind };
      console.log("the bridge's downscale is closest to", kind, "; the original is moved by", realOriginal.shift);
    }
  }

  const variants = quick
    ? [{ name: "area+jpeg75", w: 1920, h: 1080, kind: "area", jpeg: 0.75 }]
    : [
        { name: "area", w: 1920, h: 1080, kind: "area" },
        { name: "area+jpeg75", w: 1920, h: 1080, kind: "area", jpeg: 0.75 },
        { name: "bilinear+jpeg75", w: 1920, h: 1080, kind: "bilinear", jpeg: 0.75 },
        { name: "bicubic+jpeg75", w: 1920, h: 1080, kind: "bicubic", jpeg: 0.75 },
        { name: "tent+jpeg75", w: 1920, h: 1080, kind: "tent", jpeg: 0.75 },
        { name: "x2:area", w: 1280, h: 720, kind: "area" },
        { name: "x2:area+jpeg75", w: 1280, h: 720, kind: "area", jpeg: 0.75 },
      ];
  const started = Date.now();
  const measure = (variants, original) =>
    page.evaluate(
      ({ anime4k, variants, reference, original }) =>
        window.benchApi.runQuality({ anime4k, variants, reference, ...original, crops: variants.map((v) => v.name) }),
      { anime4k, variants, reference: reference ? "/files/0" : null, original },
    );
  const results = await measure(variants, {});
  if (real) {
    // The captured frame against the original as the chain delivered it; its rows go first.
    const captured = await measure([{ name: "real", url: real }], realOriginal);
    results.rows.unshift(...captured.rows);
    Object.assign(results.sheets, captured.sheets);
    results.realOriginal = realOriginal;
  }
  results.wallSeconds = Math.round((Date.now() - started) / 1000);
  for (const [name, url] of Object.entries(results.sheets)) {
    await writeFile(path.join(out, `crops-${name.replace(/[^a-z0-9]+/gi, "-")}.png`), Buffer.from(url.split(",")[1], "base64"));
  }
  delete results.sheets;
  await writeFile(path.join(out, quick ? "quality-quick.json" : "quality.json"), `${JSON.stringify(results, null, 1)}\n`);

  // A table: per variant and candidate, the text panels' SSIM, the edges' rise and overshoot, and the halo seen
  // around text (the background moved away from the text, 95th percentile, on the dark text panel and the
  // interface) with what it adds to bilinear's on the same source.
  const rows = [];
  const seen = (row) => Math.max(row.panels["text-light-on-dark"].backgroundHalo.p95, row.panels.interface.backgroundHalo.p95);
  for (const row of results.rows) {
    const text = ["text-dark-on-light", "text-light-on-dark"].map((name) => row.panels[name]);
    const bilinear = results.rows.find((r) => r.variant === row.variant && r.candidate === "bilinear");
    rows.push({
      variant: row.variant,
      candidate: row.candidate,
      ssimText: +((text[0].ssim + text[1].ssim) / 2).toFixed(4),
      dSsim: +((text[0].ssim + text[1].ssim) / 2 - (bilinear.panels["text-dark-on-light"].ssim + bilinear.panels["text-light-on-dark"].ssim) / 2).toFixed(4),
      ssimUi: +row.panels.interface.ssim.toFixed(4),
      rise: +row.edges.riseMedian.toFixed(3),
      dRise: `${Math.round((row.edges.riseMedian / bilinear.edges.riseMedian - 1) * 100)}%`,
      overshoot: +row.edges.overshootP90.toFixed(1),
      halo95: +seen(row).toFixed(1),
      haloAdded: +(seen(row) - seen(bilinear)).toFixed(1),
    });
  }
  console.table(rows);
  console.log("device", results.device.renderer, "wall", results.wallSeconds, "s");

  if (args.includes("--cost")) {
    const cost = await page.evaluate(
      ({ anime4k }) => window.benchApi.runCost({ anime4k, sizes: [[2560, 1440]], frames: 4 }),
      { anime4k },
    );
    await writeFile(path.join(out, "cost-swiftshader.json"), `${JSON.stringify(cost, null, 1)}\n`);
    console.table(cost.rows);
  }
} finally {
  await browser.close();
  server.close();
}
