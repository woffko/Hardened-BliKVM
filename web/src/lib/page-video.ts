// What the page itself counts of its picture, for the quality monitor
// (docs/design/h264-engine-choice-and-quality-monitor-v1.md): the WebRTC
// views name their connection here, the Direct view counts what it is sent,
// and the monitor reads them once a second while it is open, never
// otherwise.
import type { PageSample } from '@/lib/quality-monitor.ts';
import type { QuicStatus } from '@/lib/quic/session.ts';
import type { VideoStats } from '@/lib/quic/video.worker.ts';

let watchedPeer: RTCPeerConnection | null = null;
/** The last `getStats()` totals, for what is measured between two reads. */
let previousRtc: {
  jitterDelay: number;
  emitted: number;
  decodeTime: number;
  decoded: number;
} | null = null;

/** The WebRTC view's connection while it shows the picture; the function
 * it returns forgets it. */
export function watchPeer(peer: RTCPeerConnection): () => void {
  watchedPeer = peer;
  previousRtc = null;
  return () => {
    if (watchedPeer === peer) watchedPeer = null;
  };
}

/** What the Direct view was sent, since the picture began. */
export const directCounters = { bytes: 0, messages: 0 };

const empty = (at: number): PageSample => ({
  at,
  bytes: null,
  framesReceived: null,
  framesDecoded: null,
  framesDrawn: null,
  framesDropped: null,
  delayMs: null,
  delayEstimated: false,
  decodeMs: null,
  decoder: null,
  width: null,
  height: null,
  quic: null,
  webrtc: null
});

/** The QUIC session's picture, from what its view and the session keep. */
export function sampleQuic(status: QuicStatus, stats: VideoStats | null): PageSample {
  const sample = empty(performance.now());
  if (!stats) return sample;
  const connected = status.state === 'connected' ? status : null;
  return {
    ...sample,
    bytes: stats.bytes,
    framesReceived: stats.received,
    framesDecoded: stats.frames,
    framesDrawn: stats.frames,
    framesDropped: stats.dropped,
    delayMs: stats.ageMedianMs,
    decodeMs: stats.decodeMedianMs,
    decoder: 'WebCodecs',
    quic: {
      roundTripMs: connected?.roundTripMs ?? null,
      packetsLost: connected?.counters?.packetsLost ?? null,
      givenUp: stats.dropped,
      keyFrameRequests: stats.requests,
      resets: stats.resets,
      gatewayFramesSent: connected?.counters?.framesSent ?? null,
      gatewayFramesDropped: connected?.counters?.framesDropped ?? null,
      gatewayKeyFrames: connected?.counters?.keyFrames ?? null
    }
  };
}

/** The Direct view's picture: what it was sent; its decoder counts nothing. */
export function sampleDirect(): PageSample {
  return { ...empty(performance.now()), bytes: directCounters.bytes, decoder: 'WebCodecs' };
}

type Stat = Record<string, unknown> & { type: string; id: string };
const number = (value: unknown) =>
  typeof value === 'number' && Number.isFinite(value) ? value : null;

/** The WebRTC picture from `getStats()` of the view's connection, and the
 * video element's own count of frames drawn. */
export async function sampleWebRtc(): Promise<PageSample> {
  const sample = empty(performance.now());
  const peer = watchedPeer;
  if (!peer) return sample;
  let report: RTCStatsReport;
  try {
    report = await peer.getStats();
  } catch {
    return sample;
  }
  const stats = new Map<string, Stat>();
  report.forEach((stat: Stat) => stats.set(stat.id, stat));
  const all = [...stats.values()];
  const inbound = all.find((stat) => stat.type === 'inbound-rtp' && stat.kind === 'video');
  const transport = all.find((stat) => stat.type === 'transport');
  const pair =
    (typeof transport?.selectedCandidatePairId === 'string'
      ? stats.get(transport.selectedCandidatePairId)
      : undefined) ??
    all.find(
      (stat) => stat.type === 'candidate-pair' && stat.state === 'succeeded' && stat.nominated
    );
  if (!inbound) return sample;
  const roundTrip = number(pair?.currentRoundTripTime);
  const typeOf = (id: unknown) =>
    typeof id === 'string' ? (stats.get(id)?.candidateType as string | undefined) : undefined;
  const candidates =
    pair && typeOf(pair.localCandidateId) && typeOf(pair.remoteCandidateId)
      ? `${typeOf(pair.localCandidateId)} - ${typeOf(pair.remoteCandidateId)}`
      : null;
  const jitterDelay = number(inbound.jitterBufferDelay) ?? 0;
  const emitted = number(inbound.jitterBufferEmittedCount) ?? 0;
  const decodeTime = number(inbound.totalDecodeTime) ?? 0;
  const decoded = number(inbound.framesDecoded) ?? 0;
  const before = previousRtc;
  previousRtc = { jitterDelay, emitted, decodeTime, decoded };
  const jitterBufferMs =
    before && emitted > before.emitted
      ? ((jitterDelay - before.jitterDelay) / (emitted - before.emitted)) * 1000
      : emitted > 0
        ? (jitterDelay / emitted) * 1000
        : null;
  const decodeMs =
    before && decoded > before.decoded
      ? ((decodeTime - before.decodeTime) / (decoded - before.decoded)) * 1000
      : decoded > 0
        ? (decodeTime / decoded) * 1000
        : null;
  // What the video element drew: its own count, where it keeps one.
  const video = document.querySelector('video#screen') as HTMLVideoElement | null;
  const playback = video?.getVideoPlaybackQuality?.();
  const implementation =
    typeof inbound.decoderImplementation === 'string' ? inbound.decoderImplementation : null;
  const hardware = inbound.powerEfficientDecoder === true;
  return {
    ...sample,
    bytes: number(inbound.bytesReceived),
    framesReceived: number(inbound.framesReceived),
    framesDecoded: number(inbound.framesDecoded),
    framesDrawn: playback
      ? playback.totalVideoFrames - playback.droppedVideoFrames
      : number(inbound.framesDecoded),
    framesDropped: number(inbound.framesDropped),
    // The jitter buffer's delay a frame and half the round trip: an estimate.
    delayMs:
      jitterBufferMs === null
        ? null
        : jitterBufferMs + (roundTrip === null ? 0 : (roundTrip * 1000) / 2),
    delayEstimated: true,
    decodeMs,
    decoder: implementation ? `${implementation}${hardware ? ' (hardware)' : ''}` : 'WebRTC',
    width: number(inbound.frameWidth),
    height: number(inbound.frameHeight),
    webrtc: {
      roundTripMs: roundTrip === null ? null : roundTrip * 1000,
      candidates,
      packetsReceived: number(inbound.packetsReceived),
      packetsLost: number(inbound.packetsLost),
      jitterMs: number(inbound.jitter) === null ? null : (inbound.jitter as number) * 1000,
      jitterBufferMsPerFrame: jitterBufferMs,
      nack: number(inbound.nackCount),
      pli: number(inbound.pliCount),
      fir: number(inbound.firCount)
    }
  };
}

/** Nothing the page counts: the MJPEG image counts nothing. */
export function sampleNothing(): PageSample {
  return empty(performance.now());
}
