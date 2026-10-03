// The quality monitor (docs/design/h264-engine-choice-and-quality-monitor-v1.md):
// its window as this browser keeps it, what the board answers in
// `GET /api/vm/quality`, what the page measures of its own picture, and the
// rows of its two levels made from both.

const LAYOUT_KEY = 'hardened-blikvm-quality-monitor';

export type QualityLevel = 'basic' | 'extended';

/** The window: open or closed, its level, place, size and opacity. */
export type QualityMonitorLayout = {
  open: boolean;
  level: QualityLevel;
  x: number;
  y: number;
  width: number;
  height: number;
  opaque: boolean;
};

export const MIN_WIDTH = 200;
export const MIN_HEIGHT = 140;
const MAX_SIDE = 4000;

export const DEFAULT_LAYOUT: QualityMonitorLayout = {
  open: false,
  level: 'basic',
  x: 12,
  y: 56,
  width: 340,
  height: 460,
  opaque: false
};

const within = (value: unknown, least: number, most: number, fallback: number) =>
  typeof value === 'number' && Number.isFinite(value)
    ? Math.min(most, Math.max(least, Math.round(value)))
    : fallback;

/** The window as this browser left it, or as it is at first. */
export function loadLayout(): QualityMonitorLayout {
  try {
    const stored = JSON.parse(localStorage.getItem(LAYOUT_KEY) ?? 'null') as Record<
      string,
      unknown
    > | null;
    if (!stored || typeof stored !== 'object') return DEFAULT_LAYOUT;
    return {
      open: stored.open === true,
      level: stored.level === 'extended' ? 'extended' : 'basic',
      x: within(stored.x, 0, MAX_SIDE, DEFAULT_LAYOUT.x),
      y: within(stored.y, 0, MAX_SIDE, DEFAULT_LAYOUT.y),
      width: within(stored.width, MIN_WIDTH, MAX_SIDE, DEFAULT_LAYOUT.width),
      height: within(stored.height, MIN_HEIGHT, MAX_SIDE, DEFAULT_LAYOUT.height),
      opaque: stored.opaque === true
    };
  } catch {
    return DEFAULT_LAYOUT;
  }
}

export function saveLayout(layout: QualityMonitorLayout) {
  try {
    localStorage.setItem(LAYOUT_KEY, JSON.stringify(layout));
  } catch {
    // A browser that keeps nothing opens the monitor as it is at first.
  }
}

/** An encoder's report with the rates the server made of it. */
export type EncoderQuality = {
  codec: 'h264' | 'vp8';
  streaming: boolean;
  engine: 'hardware' | 'x264' | 'libvpx' | null;
  engineWanted: 'hardware' | 'software' | null;
  hardwareAvailable: boolean | null;
  hardwareReason: string | null;
  width: number;
  height: number;
  kbps: number;
  maxFps: number;
  converters: number;
  encodedFrames: number;
  stageMicros: { decode: number; convert: number; encode: number };
  gate: { admitted: number; tooSoon: number; unchanged: number };
  hardware: {
    passes: number;
    bothSearches: number;
    wide: number;
    higherQp: number;
    leftOut: number;
    keyFrames: number;
    failures: number;
    engineMicros: number;
    passesPerSecond: number | null;
    engineMicrosPerPass: number | null;
  } | null;
  fps: number | null;
  gatePerSecond: { admitted: number; tooSoon: number; unchanged: number } | null;
};

/** `GET /api/vm/quality`. */
export type QualityAnswer = {
  capture: {
    width: number | null;
    height: number | null;
    fps: number | null;
    queuedFps: number | null;
    quality: number | null;
  };
  h264: EncoderQuality | null;
  vp8: EncoderQuality | null;
  units: { h264CpuPercent: number | null; vp8CpuPercent: number | null };
  temperatureMillidegrees: number | null;
  version: string | null;
};

/** The way the picture comes to this page. */
export type VideoPath = 'mjpeg' | 'direct' | 'webrtc' | 'quic';

/** One row: what it is, and its value as shown; `-` where not known. */
export type QualityRow = { key: string; label: string; value: string };
export type QualitySection = { key: string; title: string | null; rows: QualityRow[] };

type Translate = (key: string, options?: Record<string, unknown>) => string;

const NOT_KNOWN = '-';

function fixed(value: number | null | undefined, places = 0): string {
  return value === null || value === undefined || !Number.isFinite(value)
    ? NOT_KNOWN
    : value.toFixed(places);
}

function size(width: number | null | undefined, height: number | null | undefined) {
  return width && height ? `${width}x${height}` : null;
}

/** What the monitor shows: the board's answer while it is fresh. */
export type QualityInputs = {
  path: VideoPath;
  /** The browser's mode: `h264` or `vp8` over WebRTC, `h264` over QUIC. */
  codec: 'h264' | 'vp8' | 'mjpeg';
  board: QualityAnswer | null;
};

/** The encoder that serves the path, from the board's answer. */
function servingEncoder(inputs: QualityInputs): EncoderQuality | null {
  if (!inputs.board || inputs.codec === 'mjpeg') return null;
  return inputs.codec === 'vp8' ? inputs.board.vp8 : inputs.board.h264;
}

function codecLabel(inputs: QualityInputs, encoder: EncoderQuality | null, t: Translate) {
  switch (inputs.codec) {
    case 'mjpeg':
      return 'MJPEG';
    case 'vp8':
      return 'VP8';
    case 'h264':
      return encoder?.engine === 'hardware'
        ? 'H.264 HW'
        : encoder?.engine === 'x264'
          ? 'H.264 SW'
          : t('qualityMonitor.h264');
  }
}

const PATH_LABEL: Record<VideoPath, string> = {
  mjpeg: 'MJPEG',
  direct: 'H.264 Direct',
  webrtc: 'WebRTC',
  quic: 'QUIC'
};

/** The rows of Basic. */
export function basicRows(inputs: QualityInputs, t: Translate): QualityRow[] {
  const { path, board } = inputs;
  const encoder = servingEncoder(inputs);
  const pictureSize =
    size(encoder?.width, encoder?.height) ?? size(board?.capture.width, board?.capture.height);
  return [
    { key: 'speed', label: t('qualityMonitor.speed'), value: NOT_KNOWN },
    { key: 'delay', label: t('qualityMonitor.delay'), value: NOT_KNOWN },
    { key: 'path', label: t('qualityMonitor.path'), value: PATH_LABEL[path] },
    {
      key: 'fps',
      label: t('qualityMonitor.fps'),
      value: `${NOT_KNOWN} / ${fixed(board?.capture.fps, 1)}`
    },
    {
      key: 'bitrate',
      label: t('qualityMonitor.bitrate'),
      value:
        inputs.codec === 'mjpeg'
          ? board?.capture.quality === null || board?.capture.quality === undefined
            ? NOT_KNOWN
            : t('qualityMonitor.jpegQuality', { value: board.capture.quality })
          : encoder
            ? `${encoder.kbps} kbit/s`
            : NOT_KNOWN
    },
    {
      key: 'codec',
      label: t('qualityMonitor.codec'),
      value: [codecLabel(inputs, encoder, t), pictureSize].filter(Boolean).join(' ')
    },
    {
      key: 'chroma',
      label: t('qualityMonitor.chroma'),
      value: inputs.codec === 'mjpeg' ? '4:2:2' : '4:2:0'
    }
  ];
}

/** The sections of Extended: Basic's rows first, then the board's. Rows
 * that do not apply are left out. */
export function extendedSections(inputs: QualityInputs, t: Translate): QualitySection[] {
  const { board } = inputs;
  const encoder = servingEncoder(inputs);
  const boardRows: QualityRow[] = [];
  const add = (key: string, label: string, value: string) =>
    boardRows.push({ key, label: t(label), value });
  add('version', 'qualityMonitor.version', board?.version ?? NOT_KNOWN);
  add(
    'capture',
    'qualityMonitor.capture',
    `${size(board?.capture.width, board?.capture.height) ?? NOT_KNOWN}, ${fixed(board?.capture.fps, 1)} / ${fixed(board?.capture.queuedFps, 1)}`
  );
  add('jpegQuality', 'qualityMonitor.captureQuality', fixed(board?.capture.quality));
  if (encoder) {
    const engine =
      encoder.engine === 'hardware'
        ? t('qualityMonitor.videoEngine')
        : (encoder.engine ?? NOT_KNOWN);
    const wanted =
      encoder.engineWanted === 'hardware'
        ? t('qualityMonitor.videoEngine')
        : encoder.engineWanted === 'software'
          ? 'x264'
          : null;
    add(
      'encoder',
      'qualityMonitor.encoder',
      wanted && wanted !== engine
        ? `${engine} (${t('qualityMonitor.chosen', { value: wanted })})`
        : engine
    );
    add('encoderFps', 'qualityMonitor.encodedFps', fixed(encoder.fps, 1));
    add(
      'stages',
      'qualityMonitor.stages',
      `${fixed(encoder.stageMicros.decode / 1000, 1)} / ${fixed(encoder.stageMicros.convert / 1000, 1)} / ${fixed(encoder.stageMicros.encode / 1000, 1)} ms`
    );
    if (encoder.hardware) {
      const hardware = encoder.hardware;
      const perFrame =
        hardware.passesPerSecond !== null && encoder.fps
          ? hardware.passesPerSecond / encoder.fps
          : null;
      add(
        'enginePass',
        'qualityMonitor.enginePass',
        `${fixed(hardware.engineMicrosPerPass === null ? null : hardware.engineMicrosPerPass / 1000, 1)} ms, ${fixed(perFrame, 2)}`
      );
      add(
        'engineCounts',
        'qualityMonitor.engineCounts',
        `${hardware.higherQp} / ${hardware.leftOut} / ${hardware.keyFrames} / ${hardware.failures}`
      );
    }
    add(
      'gate',
      'qualityMonitor.gate',
      encoder.gatePerSecond
        ? `${fixed(encoder.gatePerSecond.admitted, 1)} / ${fixed(encoder.gatePerSecond.tooSoon, 1)} / ${fixed(encoder.gatePerSecond.unchanged, 1)}`
        : NOT_KNOWN
    );
    const share =
      encoder.codec === 'vp8' ? board?.units.vp8CpuPercent : board?.units.h264CpuPercent;
    add(
      'unitCpu',
      'qualityMonitor.unitCpu',
      share === null || share === undefined ? NOT_KNOWN : `${fixed(share)} %`
    );
  }
  add(
    'temperature',
    'qualityMonitor.temperature',
    board?.temperatureMillidegrees === null || board?.temperatureMillidegrees === undefined
      ? NOT_KNOWN
      : `${fixed(board.temperatureMillidegrees / 1000, 1)} °C`
  );

  return [
    { key: 'basic', title: null, rows: basicRows(inputs, t) },
    { key: 'board', title: t('qualityMonitor.board'), rows: boardRows }
  ];
}
