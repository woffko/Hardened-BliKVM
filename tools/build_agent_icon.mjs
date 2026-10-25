// The target agent's icon (crates/bli-kvm-target-agent/assets/agent.ico) from the project's logo
// (web/public/hardened-blikvm-logo.png): the mark at its left, the bracket with its copper edge and the "b", on the
// logo's white as a rounded tile, so that it shows on a dark taskbar as on a light one. Each size Windows asks for is
// drawn from the logo itself, and the icon file keeps them as PNG images (Windows Vista and later read those).
// The icon is committed; this tool makes it again when the logo changes.
// usage: node tools/build_agent_icon.mjs
import fs from "node:fs";
import path from "node:path";
import { launchChromium } from "./playwright/quality_fixture.mjs";

const root = path.resolve(import.meta.dirname, "..");
const logo = fs
  .readFileSync(path.join(root, "web/public/hardened-blikvm-logo.png"))
  .toString("base64");
const SIZES = [16, 20, 24, 32, 40, 48, 64, 256];

const browser = await launchChromium();
try {
  const page = await browser.newPage();
  const images = await page.evaluate(
    async ({ logo, sizes }) => {
      const image = await createImageBitmap(
        await (await fetch(`data:image/png;base64,${logo}`)).blob(),
      );
      // The bracket, the arc's start and the "b", in the logo's pixels.
      const crop = { x: 52, y: 372, w: 356, h: 460 };
      const images = {};
      for (const size of sizes) {
        const canvas = new OffscreenCanvas(size, size);
        const context = canvas.getContext("2d");
        context.imageSmoothingQuality = "high";
        const radius = size * 0.2;
        context.beginPath();
        context.roundRect(0, 0, size, size, radius);
        context.fillStyle = "#fefeff";
        context.fill();
        context.clip();
        const margin = size <= 24 ? size * 0.06 : size * 0.1;
        const scale = Math.min(
          (size - 2 * margin) / crop.w,
          (size - 2 * margin) / crop.h,
        );
        const width = crop.w * scale;
        const height = crop.h * scale;
        context.drawImage(
          image,
          crop.x,
          crop.y,
          crop.w,
          crop.h,
          (size - width) / 2,
          (size - height) / 2,
          width,
          height,
        );
        const png = await canvas.convertToBlob({ type: "image/png" });
        let text = "";
        for (const byte of new Uint8Array(await png.arrayBuffer()))
          text += String.fromCharCode(byte);
        images[size] = btoa(text);
      }
      return images;
    },
    { logo, sizes: SIZES },
  );
  // ICONDIR, one ICONDIRENTRY per size, then the PNG images.
  const pngs = SIZES.map((size) => Buffer.from(images[size], "base64"));
  const header = Buffer.alloc(6 + 16 * SIZES.length);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(SIZES.length, 4);
  let offset = header.length;
  SIZES.forEach((size, index) => {
    const entry = 6 + 16 * index;
    header.writeUInt8(size >= 256 ? 0 : size, entry);
    header.writeUInt8(size >= 256 ? 0 : size, entry + 1);
    header.writeUInt8(0, entry + 2);
    header.writeUInt8(0, entry + 3);
    header.writeUInt16LE(1, entry + 4);
    header.writeUInt16LE(32, entry + 6);
    header.writeUInt32LE(pngs[index].length, entry + 8);
    header.writeUInt32LE(offset, entry + 12);
    offset += pngs[index].length;
  });
  const out = path.join(root, "crates/bli-kvm-target-agent/assets/agent.ico");
  fs.writeFileSync(out, Buffer.concat([header, ...pngs]));
  console.log(`${path.relative(root, out)}: ${SIZES.join(", ")} px`);
} finally {
  await browser.close();
}
