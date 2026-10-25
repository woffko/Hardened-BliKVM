// Test pictures for choosing the page's upscaler (docs/baselines/work-plan-20261008-screen-upscaling.md, phase 0).
// A lab tool, never shipped. Everything is drawn deterministically on a 2D canvas at the target's size.

/**
 * Text, interface elements, lines and edges at 2560x1440, as a desktop shows them. Returns the step edges drawn
 * (pixel-aligned, for the rise and halo measures) and the panels (for per-panel scores).
 */
export function drawSynthetic(ctx, width = 2560, height = 1440) {
  const edges = [];
  const panels = [];
  ctx.save();
  ctx.fillStyle = "#ffffff";
  ctx.fillRect(0, 0, width, height);
  ctx.textBaseline = "alphabetic";

  const sentence = "The quick brown fox jumps over the lazy dog 0123456789 — Съешь же ещё этих мягких булок";
  const code = "for (let i = 0; i < frames.length; i++) { draw(frames[i], scale * 1.25); } // ok";

  // Dark text on light, sans-serif and monospace, 9 to 16 px.
  const textPanel = (x0, y0, w, h, fg, bg, name) => {
    ctx.fillStyle = bg;
    ctx.fillRect(x0, y0, w, h);
    ctx.fillStyle = fg;
    let y = y0 + 22;
    for (const family of ["sans-serif", "monospace"]) {
      for (let size = 9; size <= 16; size++) {
        ctx.font = `${size}px ${family}`;
        ctx.fillText(`${size}px ${family === "monospace" ? code : sentence}`, x0 + 12, y);
        y += size + 9;
      }
    }
    panels.push({ name, x: x0, y: y0, w, h });
  };
  textPanel(0, 0, 1280, 480, "#1a1a1a", "#ffffff", "text-dark-on-light");
  textPanel(1280, 0, 1280, 480, "#e8e8e8", "#1e1e1e", "text-light-on-dark");

  // Interface: buttons, fields, checkboxes, a title bar, a menu, a scroll bar, icons.
  {
    const x0 = 0, y0 = 480, w = 1280, h = 480;
    ctx.fillStyle = "#f3f3f3";
    ctx.fillRect(x0, y0, w, h);
    ctx.fillStyle = "#dfe7f2";
    ctx.fillRect(x0, y0, w, 32);
    ctx.fillStyle = "#202020";
    ctx.font = "13px sans-serif";
    ctx.fillText("Settings — Display — Scale and layout", x0 + 12, y0 + 21);
    for (let i = 0; i < 6; i++) {
      const bx = x0 + 24 + i * 200, by = y0 + 60;
      ctx.fillStyle = i % 2 ? "#ffffff" : "#0067c0";
      ctx.fillRect(bx, by, 170, 32);
      ctx.strokeStyle = "#8a8a8a";
      ctx.lineWidth = 1;
      ctx.strokeRect(bx + 0.5, by + 0.5, 169, 31);
      ctx.fillStyle = i % 2 ? "#1a1a1a" : "#ffffff";
      ctx.font = "12px sans-serif";
      ctx.fillText(`Button ${i + 1}`, bx + 50, by + 21);
    }
    for (let i = 0; i < 8; i++) {
      const cx = x0 + 24, cy = y0 + 120 + i * 30;
      ctx.strokeStyle = "#5a5a5a";
      ctx.strokeRect(cx + 0.5, cy + 0.5, 15, 15);
      if (i % 2 === 0) {
        ctx.fillStyle = "#0067c0";
        ctx.fillRect(cx + 4, cy + 4, 8, 8);
      }
      ctx.fillStyle = "#1a1a1a";
      ctx.font = `${11 + (i % 3)}px sans-serif`;
      ctx.fillText(`Option ${i + 1}: show the picture at its own size`, cx + 26, cy + 13);
    }
    for (let i = 0; i < 10; i++) {
      ctx.fillStyle = i === 3 ? "#cce4f7" : "#ffffff";
      ctx.fillRect(x0 + 520, y0 + 120 + i * 26, 360, 26);
      ctx.fillStyle = "#1a1a1a";
      ctx.font = "12px sans-serif";
      ctx.fillText(`Menu item ${i + 1}    Ctrl+${String.fromCharCode(65 + i)}`, x0 + 532, y0 + 138 + i * 26);
    }
    ctx.fillStyle = "#e0e0e0";
    ctx.fillRect(x0 + 900, y0 + 120, 14, 300);
    ctx.fillStyle = "#9a9a9a";
    ctx.fillRect(x0 + 902, y0 + 180, 10, 80);
    for (let i = 0; i < 8; i++) {
      const ix = x0 + 960 + (i % 4) * 70, iy = y0 + 140 + Math.floor(i / 4) * 90;
      ctx.fillStyle = ["#d13438", "#107c10", "#0067c0", "#ff8c00"][i % 4];
      ctx.fillRect(ix, iy, 32, 32);
      ctx.fillStyle = "#ffffff";
      ctx.fillRect(ix + 8, iy + 8, 16, 4);
      ctx.fillRect(ix + 8, iy + 16, 10, 4);
      ctx.fillStyle = "#1a1a1a";
      ctx.font = "11px sans-serif";
      ctx.fillText(`App ${i + 1}`, ix, iy + 48);
    }
    panels.push({ name: "interface", x: x0, y: y0, w, h });
  }

  // Lines: one pixel wide at 2, 3, 4 and 5 px spacing, both ways; anti-aliased diagonals; a fine checkerboard.
  {
    const x0 = 1280, y0 = 480, w = 1280, h = 480;
    ctx.fillStyle = "#ffffff";
    ctx.fillRect(x0, y0, w, h);
    ctx.fillStyle = "#000000";
    [2, 3, 4, 5].forEach((step, k) => {
      for (let x = 0; x < 120; x += step) ctx.fillRect(x0 + 20 + k * 150 + x, y0 + 20, 1, 140);
      for (let y = 0; y < 140; y += step) ctx.fillRect(x0 + 20 + k * 150, y0 + 190 + y, 120, 1);
    });
    ctx.strokeStyle = "#000000";
    ctx.lineWidth = 1;
    for (let i = 0; i < 12; i++) {
      ctx.beginPath();
      ctx.moveTo(x0 + 640 + i * 10, y0 + 20);
      ctx.lineTo(x0 + 640 + i * 40, y0 + 320);
      ctx.stroke();
    }
    for (let y = 0; y < 100; y++) for (let x = 0; x < 100; x++) if ((x + y) % 2 === 0) ctx.fillRect(x0 + 1120 + x, y0 + 20 + y, 1, 1);
    for (let y = 0; y < 100; y += 2) for (let x = 0; x < 100; x += 2) if ((x / 2 + y / 2) % 2 === 0) ctx.fillRect(x0 + 1120 + x, y0 + 140 + y, 2, 2);
    panels.push({ name: "lines", x: x0, y: y0, w, h });
  }

  // Step edges, pixel-aligned at every phase of the 3/4 grid, vertical and horizontal, black and white and in colour.
  {
    const x0 = 0, y0 = 960, w = 1280, h = 480;
    ctx.fillStyle = "#ffffff";
    ctx.fillRect(x0, y0, w, h);
    const pairs = [["#000000", "#ffffff", 0, 255], ["#202020", "#d0d0d0", 32, 208], ["#ffffff", "#000000", 255, 0]];
    pairs.forEach(([a, b, la, lb], row) => {
      for (let k = 0; k < 12; k++) {
        const bx = x0 + 16 + k * 104, by = y0 + 16 + row * 150;
        ctx.fillStyle = a;
        ctx.fillRect(bx, by, 104, 130);
        ctx.fillStyle = b;
        const cut = 40 + k; // the edge's column, at each phase in turn
        ctx.fillRect(bx + cut, by, 104 - cut, 64);
        edges.push({ orientation: "v", pos: bx + cut, from: by + 8, to: by + 56, low: la, high: lb });
        const cutY = 64 + 20 + (k % 4); // a horizontal edge below
        ctx.fillRect(bx, by + cutY, 104, 130 - cutY);
        edges.push({ orientation: "h", pos: by + cutY, from: bx + 8, to: bx + 96, low: la, high: lb });
      }
    });
    panels.push({ name: "edges", x: x0, y: y0, w, h });
  }

  // A smooth gradient and colour bars.
  {
    const x0 = 1280, y0 = 960, w = 1280, h = 480;
    const g = ctx.createLinearGradient(x0, 0, x0 + w, 0);
    g.addColorStop(0, "#000000");
    g.addColorStop(1, "#ffffff");
    ctx.fillStyle = g;
    ctx.fillRect(x0, y0, w, 240);
    ["#ffffff", "#ffff00", "#00ffff", "#00ff00", "#ff00ff", "#ff0000", "#0000ff", "#000000"].forEach((c, k) => {
      ctx.fillStyle = c;
      ctx.fillRect(x0 + k * 160, y0 + 240, 160, 240);
    });
    panels.push({ name: "gradient", x: x0, y: y0, w, h });
  }
  ctx.restore();
  return { edges, panels };
}

/**
 * The pattern the target shows on its screen for the real pairs: pixel-aligned geometry only (fonts differ between
 * systems), drawn at device pixels, with corner marks to find it again in the captured frame.
 */
export function drawCalibration(ctx, width, height) {
  const edges = [];
  ctx.save();
  ctx.fillStyle = "#808080";
  ctx.fillRect(0, 0, width, height);
  // Corner marks: 64x64 white squares with a black 32x32 core.
  for (const [x, y] of [[0, 0], [width - 64, 0], [0, height - 64], [width - 64, height - 64]]) {
    ctx.fillStyle = "#ffffff";
    ctx.fillRect(x, y, 64, 64);
    ctx.fillStyle = "#000000";
    ctx.fillRect(x + 16, y + 16, 32, 32);
  }
  // Step edges at every phase, as in the synthetic set, across the middle of the screen.
  const cols = Math.floor((width - 256) / 104);
  for (let row = 0; row < 3; row++) {
    for (let k = 0; k < cols; k++) {
      const bx = 128 + k * 104, by = 128 + row * 150;
      ctx.fillStyle = row === 1 ? "#202020" : "#000000";
      ctx.fillRect(bx, by, 104, 130);
      ctx.fillStyle = row === 1 ? "#d0d0d0" : "#ffffff";
      const cut = 40 + (k % 12);
      ctx.fillRect(bx + cut, by, 104 - cut, 64);
      edges.push({ orientation: "v", pos: bx + cut, from: by + 8, to: by + 56, low: row === 1 ? 32 : 0, high: row === 1 ? 208 : 255 });
      const cutY = 84 + (k % 4);
      ctx.fillRect(bx, by + cutY, 104, 130 - cutY);
      edges.push({ orientation: "h", pos: by + cutY, from: bx + 8, to: bx + 96, low: row === 1 ? 32 : 0, high: row === 1 ? 208 : 255 });
    }
  }
  // One-pixel lines at 2 to 5 px spacing and a checkerboard.
  ctx.fillStyle = "#000000";
  [2, 3, 4, 5].forEach((step, k) => {
    for (let x = 0; x < 160; x += step) ctx.fillRect(128 + k * 200 + x, 620, 1, 160);
    for (let y = 0; y < 160; y += step) ctx.fillRect(128 + k * 200, 800 + y, 160, 1);
  });
  for (let y = 0; y < 160; y++) for (let x = 0; x < 160; x++) if ((x + y) % 2 === 0) ctx.fillRect(1000 + x, 620 + y, 1, 1);
  ctx.restore();
  return { edges };
}
