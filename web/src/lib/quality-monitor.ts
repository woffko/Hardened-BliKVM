// The quality monitor (docs/design/h264-engine-choice-and-quality-monitor-v1.md):
// its window as this browser keeps it, what the board answers in
// `GET /api/vm/quality`, what the page measures of its own picture, and the
// rows of its two levels made from both.

import type { UpscaleReport } from './upscale/upscaler.ts';
import type { UpscaleStop } from './upscale/watchdog.ts';

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

/**
 * What the page counts of its own picture at one moment: totals since the
 * picture began, and what the path's own counters say. A count the path
 * does not have is `null`.
 */
export type PageSample = {
  at: number;
  bytes: number | null;
  framesReceived: number | null;
  framesDecoded: number | null;
  framesDrawn: number | null;
  framesDropped: number | null;
  /** A frame's age when it is drawn, ms; estimated over WebRTC. */
  delayMs: number | null;
  delayEstimated: boolean;
  /** Decoding a frame, ms. */
  decodeMs: number | null;
  decoder: string | null;
  width: number | null;
  height: number | null;
  /** How the view draws its frames, where it says (the QUIC view). */
  upscale: UpscaleReport | null;
  /** Why upscaling stopped on this page load, if it did. */
  upscaleStopped: UpscaleStop | null;
  /** QUIC: the session's and the gateway's counters. */
  quic: {
    roundTripMs: number | null;
    packetsLost: number | null;
    givenUp: number;
    keyFrameRequests: number;
    resets: number;
    gatewayFramesSent: number | null;
    gatewayFramesDropped: number | null;
    gatewayKeyFrames: number | null;
  } | null;
  /** WebRTC: `getStats()`. */
  webrtc: {
    roundTripMs: number | null;
    candidates: string | null;
    packetsReceived: number | null;
    packetsLost: number | null;
    jitterMs: number | null;
    jitterBufferMsPerFrame: number | null;
    nack: number | null;
    pli: number | null;
    fir: number | null;
  } | null;
};

/** Per second, from two samples. */
export type PageRates = {
  megabitsPerSecond: number | null;
  receivedPerSecond: number | null;
  decodedPerSecond: number | null;
  drawnPerSecond: number | null;
  droppedPerSecond: number | null;
};

function perSecond(now: number | null, before: number | null, seconds: number) {
  if (now === null || before === null || now < before || seconds <= 0) return null;
  return (now - before) / seconds;
}

export function pageRates(previous: PageSample | null, current: PageSample): PageRates {
  const seconds = previous ? (current.at - previous.at) / 1000 : 0;
  const rate = (pick: (sample: PageSample) => number | null) =>
    previous ? perSecond(pick(current), pick(previous), seconds) : null;
  const bytes = rate((sample) => sample.bytes);
  return {
    megabitsPerSecond: bytes === null ? null : (bytes * 8) / 1e6,
    receivedPerSecond: rate((sample) => sample.framesReceived),
    decodedPerSecond: rate((sample) => sample.framesDecoded),
    drawnPerSecond: rate((sample) => sample.framesDrawn),
    droppedPerSecond: rate((sample) => sample.framesDropped)
  };
}

const STOPPED: Record<UpscaleStop, string> = {
  unavailable: 'qualityMonitor.upscaleStopUnavailable',
  lost: 'qualityMonitor.upscaleStopLost',
  slow: 'qualityMonitor.upscaleStopSlow'
};

/** How the frames are drawn, as the monitor's row says it. */
function upscaleText(
  report: UpscaleReport | null,
  stopped: UpscaleStop | null,
  t: Translate
): string {
  if (stopped) return t('qualityMonitor.upscaleStopped', { reason: t(STOPPED[stopped]) });
  if (!report || report.mode === 'off') return t('qualityMonitor.upscaleOff');
  if (report.mode === 'plain') return t('qualityMonitor.upscaleNotEnlarged');
  // The monitor draws in a worker, which has the monitor's own labels only.
  const sharpness = t(
    `qualityMonitor.upscale${report.mode[0].toUpperCase()}${report.mode.slice(1)}`
  );
  const how = t('qualityMonitor.upscaleWith', { sharpness });
  if (report.gpuMs !== null) {
    return `${how}, ${t('qualityMonitor.upscaleGpu', { value: report.gpuMs.toFixed(2) })}`;
  }
  if (report.drawMs !== null) {
    return `${how}, ${t('qualityMonitor.upscaleDraw', { value: report.drawMs.toFixed(2) })}`;
  }
  return how;
}

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

/** What the monitor shows: the board's answer while it is fresh, the page's numbers. */
export type QualityInputs = {
  path: VideoPath;
  /** The browser's mode: `h264` or `vp8` over WebRTC, `h264` over QUIC. */
  codec: 'h264' | 'vp8' | 'mjpeg';
  board: QualityAnswer | null;
  sample: PageSample | null;
  rates: PageRates | null;
  /** Measured with animation frames, Hz. */
  refreshHz: number | null;
};

/**
 * The encoder that serves the path, from the board's answer. The Direct view
 * is sent the capture device's own H.264 and no encoder of ours serves it:
 * what the H.264 encoder reports is another viewer's.
 */
function servingEncoder(inputs: QualityInputs): EncoderQuality | null {
  if (!inputs.board || inputs.codec === 'mjpeg' || inputs.path === 'direct') return null;
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
  const { path, board, sample, rates } = inputs;
  const encoder = servingEncoder(inputs);
  const pictureSize =
    size(encoder?.width, encoder?.height) ??
    size(sample?.width, sample?.height) ??
    size(board?.capture.width, board?.capture.height);
  const delay =
    sample?.delayMs === null || sample?.delayMs === undefined
      ? NOT_KNOWN
      : `${sample.delayEstimated ? '~' : ''}${fixed(sample.delayMs)} ms`;
  const drawn = path === 'mjpeg' ? NOT_KNOWN : fixed(rates?.drawnPerSecond, 1);
  // To a kilobit below a megabit a second: a still screen sends a few.
  const speed =
    path === 'mjpeg' || !rates || rates.megabitsPerSecond === null
      ? NOT_KNOWN
      : `${fixed(rates.megabitsPerSecond, rates.megabitsPerSecond < 1 ? 3 : 2)} Mbit/s`;
  return [
    { key: 'speed', label: t('qualityMonitor.speed'), value: speed },
    { key: 'delay', label: t('qualityMonitor.delay'), value: path === 'mjpeg' ? NOT_KNOWN : delay },
    { key: 'path', label: t('qualityMonitor.path'), value: PATH_LABEL[path] },
    {
      key: 'fps',
      label: t('qualityMonitor.fps'),
      value: `${drawn} / ${fixed(board?.capture.fps, 1)}`
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

/** The sections of Extended: Basic's rows first, then the transport, the
 * board and the local side. Rows that do not apply are left out. */
export function extendedSections(inputs: QualityInputs, t: Translate): QualitySection[] {
  const { path, board, sample, rates } = inputs;
  const encoder = servingEncoder(inputs);
  const transport: QualityRow[] = [];
  const row = (key: string, label: string, value: string) =>
    transport.push({ key, label: t(label), value });
  if (path === 'quic' && sample?.quic) {
    const quic = sample.quic;
    row('quicRoundTrip', 'qualityMonitor.roundTrip', `${fixed(quic.roundTripMs, 1)} ms`);
    row('quicLost', 'qualityMonitor.datagramsLost', fixed(quic.packetsLost));
    row('quicFrames', 'qualityMonitor.framesReceived', fixed(sample.framesReceived));
    row('quicGivenUp', 'qualityMonitor.givenUp', fixed(quic.givenUp));
    row('quicRequests', 'qualityMonitor.keyFrameRequests', fixed(quic.keyFrameRequests));
    row(
      'quicGateway',
      'qualityMonitor.gatewayFrames',
      `${fixed(quic.gatewayFramesSent)} / ${fixed(quic.gatewayFramesDropped)} / ${fixed(quic.gatewayKeyFrames)}`
    );
  }
  if (path === 'webrtc' && sample?.webrtc) {
    const rtc = sample.webrtc;
    row('rtcRoundTrip', 'qualityMonitor.roundTrip', `${fixed(rtc.roundTripMs, 1)} ms`);
    row('rtcCandidates', 'qualityMonitor.candidates', rtc.candidates ?? NOT_KNOWN);
    row(
      'rtcPackets',
      'qualityMonitor.packets',
      `${fixed(rtc.packetsReceived)} / ${fixed(rtc.packetsLost)}`
    );
    row('rtcJitter', 'qualityMonitor.jitter', `${fixed(rtc.jitterMs, 1)} ms`);
    row(
      'rtcJitterBuffer',
      'qualityMonitor.jitterBuffer',
      `${fixed(rtc.jitterBufferMsPerFrame, 1)} ms`
    );
    row(
      'rtcFeedback',
      'qualityMonitor.feedback',
      `${fixed(rtc.nack)} / ${fixed(rtc.pli)} / ${fixed(rtc.fir)}`
    );
    row(
      'rtcFrames',
      'qualityMonitor.framesReceivedDecodedDropped',
      `${fixed(sample.framesReceived)} / ${fixed(sample.framesDecoded)} / ${fixed(sample.framesDropped)}`
    );
  }
  if (path === 'mjpeg') {
    row('mjpegCapture', 'qualityMonitor.captureRate', `${fixed(board?.capture.fps, 1)}`);
    row('mjpegQueued', 'qualityMonitor.queuedRate', `${fixed(board?.capture.queuedFps, 1)}`);
  }

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

  const local: QualityRow[] = [];
  const put = (key: string, label: string, value: string) =>
    local.push({ key, label: t(label), value });
  if (path !== 'mjpeg') {
    put('decoder', 'qualityMonitor.decoder', sample?.decoder ?? NOT_KNOWN);
    put(
      'framesPerSecond',
      'qualityMonitor.framesPerSecond',
      `${fixed(rates?.receivedPerSecond, 1)} / ${fixed(rates?.decodedPerSecond, 1)} / ${fixed(rates?.drawnPerSecond, 1)} / ${fixed(rates?.droppedPerSecond, 1)}`
    );
    put('decodeTime', 'qualityMonitor.decodeTime', `${fixed(sample?.decodeMs, 1)} ms`);
  }
  if (sample?.upscale || sample?.upscaleStopped) {
    put(
      'upscaler',
      'qualityMonitor.upscaler',
      upscaleText(sample.upscale, sample.upscaleStopped, t)
    );
  }
  put('refresh', 'qualityMonitor.refresh', `${fixed(inputs.refreshHz)} Hz`);

  return [
    { key: 'basic', title: null, rows: basicRows(inputs, t) },
    ...(transport.length
      ? [{ key: 'transport', title: t('qualityMonitor.transport'), rows: transport }]
      : []),
    { key: 'board', title: t('qualityMonitor.board'), rows: boardRows },
    { key: 'local', title: t('qualityMonitor.local'), rows: local }
  ];
}
