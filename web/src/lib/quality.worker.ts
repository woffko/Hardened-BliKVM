// The quality monitor's worker (docs/design/h264-engine-choice-and-quality-monitor-v1.md):
// once a second, when the page sends what it counted of its own picture, it
// asks the board (`GET /api/vm/quality`), makes the rows and draws them on
// the monitor's canvas. The request, the rows and the drawing stay off the
// page's main thread, which only counts and sends; the canvas is the
// compositor's, like the QUIC picture's. The worker lives while the monitor
// is open.
import {
  basicRows,
  extendedSections,
  pageRates,
  type PageSample,
  type QualityAnswer,
  type QualityInputs,
  type QualityLevel,
  type QualitySection,
  type VideoPath
} from '@/lib/quality-monitor.ts';

/** A request that has not answered within this is given up for this second. */
const TIMEOUT_MS = 900;
/** The board's answer counts as known this long. */
const FRESH_MS = 3000;
const FONT = 'ui-sans-serif, system-ui, sans-serif';

/** How the rows are drawn: the level, the canvas's width in CSS pixels, the
 * text's size, the display's pixel ratio, the labels in the page's language. */
export type QualityView = {
  level: QualityLevel;
  codec: QualityInputs['codec'];
  width: number;
  fontPx: number;
  scale: number;
  labels: Record<string, string>;
  refreshHz: number | null;
};

export type ToQualityWorker =
  | { type: 'start'; url: string; canvas: OffscreenCanvas; view: QualityView }
  | { type: 'view'; view: QualityView }
  | { type: 'sample'; path: VideoPath; sample: PageSample };

/** The rows' height in CSS pixels, when it changes. */
export type FromQualityWorker = { type: 'height'; height: number };

type Scope = {
  postMessage(message: FromQualityWorker): void;
  onmessage: ((event: MessageEvent<ToQualityWorker>) => void) | null;
};
const scope = self as unknown as Scope & {
  /** What was drawn last, by row: for the contracts, read from the worker. */
  qualityMonitorRows?: Record<string, string>;
  qualityMonitorLabels?: Record<string, string>;
  qualityMonitorSections?: string[];
};

let url = '';
let canvas: OffscreenCanvas | null = null;
let context: OffscreenCanvasRenderingContext2D | null = null;
let view: QualityView | null = null;
let previous: { path: VideoPath; sample: PageSample } | null = null;
let board: { at: number; answer: QualityAnswer } | null = null;
let inputs: QualityInputs | null = null;
let height = 0;

async function ask(): Promise<QualityAnswer | null> {
  const abort = new AbortController();
  const timer = setTimeout(() => abort.abort(), TIMEOUT_MS);
  try {
    const response = await fetch(url, {
      credentials: 'include',
      headers: { Accept: 'application/json' },
      signal: abort.signal
    });
    if (!response.ok) return null;
    const body = (await response.json()) as { code?: number; data?: unknown };
    return body.code === 0 && body.data ? (body.data as QualityAnswer) : null;
  } catch {
    return null;
  } finally {
    clearTimeout(timer);
  }
}

/** The page's labels, with `{{value}}` filled in where a row has one. */
function translate(key: string, options?: Record<string, unknown>): string {
  const label = view?.labels[key.replace(/^qualityMonitor\./, '')] ?? key;
  return options ? label.replace(/\{\{(\w+)\}\}/g, (_, name) => String(options[name])) : label;
}

function fit(text: string, room: number): string {
  if (!context || context.measureText(text).width <= room) return text;
  let cut = text;
  while (cut.length > 1 && context.measureText(`${cut}…`).width > room) cut = cut.slice(0, -1);
  return `${cut}…`;
}

function draw() {
  if (!canvas || !context || !view || !inputs) return;
  const sections: QualitySection[] =
    view.level === 'extended'
      ? extendedSections(inputs, translate)
      : [{ key: 'basic', title: null, rows: basicRows(inputs, translate) }];
  scope.qualityMonitorRows = Object.fromEntries(
    sections.flatMap((section) => section.rows.map((row) => [row.key, row.value]))
  );
  scope.qualityMonitorLabels = Object.fromEntries(
    sections.flatMap((section) => section.rows.map((row) => [row.key, row.label]))
  );
  scope.qualityMonitorSections = sections.map((section) => section.key);

  const { width, fontPx, scale } = view;
  const line = Math.round(fontPx * 1.35);
  const titleLine = Math.round(fontPx * 1.6);
  const padX = 6;
  const padY = 4;
  let total = padY * 2;
  for (const section of sections) {
    if (section.title) total += titleLine;
    total += section.rows.length * line;
  }
  const pixelWidth = Math.max(1, Math.round(width * scale));
  const pixelHeight = Math.max(1, Math.round(total * scale));
  if (canvas.width !== pixelWidth || canvas.height !== pixelHeight) {
    canvas.width = pixelWidth;
    canvas.height = pixelHeight;
  }
  context.setTransform(scale, 0, 0, scale, 0, 0);
  context.clearRect(0, 0, width, total);
  context.textBaseline = 'middle';
  const labelRoom = Math.floor((width - padX * 2) * 0.4);
  const gap = 8;
  const valueRoom = width - padX * 2 - labelRoom - gap;
  let y = padY;
  for (const section of sections) {
    if (section.title) {
      context.font = `${fontPx - 2}px ${FONT}`;
      context.fillStyle = 'rgb(163, 163, 163)';
      context.textAlign = 'left';
      context.fillText(
        fit(section.title.toUpperCase(), width - padX * 2),
        padX,
        y + titleLine / 2 + 1
      );
      y += titleLine;
    }
    context.font = `${fontPx}px ${FONT}`;
    for (const row of section.rows) {
      context.fillStyle = 'rgb(212, 212, 212)';
      context.textAlign = 'right';
      context.fillText(fit(row.label, labelRoom), padX + labelRoom, y + line / 2);
      context.fillStyle = 'rgb(255, 255, 255)';
      context.textAlign = 'left';
      context.fillText(fit(row.value, valueRoom), padX + labelRoom + gap, y + line / 2);
      y += line;
    }
  }
  if (total !== height) {
    height = total;
    scope.postMessage({ type: 'height', height });
  }
}

scope.onmessage = async (event) => {
  const message = event.data;
  switch (message.type) {
    case 'start':
      url = message.url;
      canvas = message.canvas;
      context = canvas.getContext('2d');
      view = message.view;
      inputs = {
        path: 'mjpeg',
        codec: view.codec,
        board: null,
        sample: null,
        rates: null,
        refreshHz: view.refreshHz
      };
      draw();
      break;
    case 'view':
      view = message.view;
      if (inputs) inputs = { ...inputs, codec: view.codec, refreshHz: view.refreshHz };
      draw();
      break;
    case 'sample': {
      const answer = await ask();
      if (answer) board = { at: performance.now(), answer };
      const { path, sample } = message;
      // A sample of another path makes no rate with this one.
      const rates = pageRates(previous?.path === path ? previous.sample : null, sample);
      previous = { path, sample };
      // An answer older than three seconds is no longer known.
      const fresh = board && performance.now() - board.at <= FRESH_MS ? board.answer : null;
      inputs = {
        path,
        codec: view?.codec ?? 'h264',
        board: fresh,
        sample,
        rates,
        refreshHz: view?.refreshHz ?? null
      };
      draw();
      break;
    }
  }
};
