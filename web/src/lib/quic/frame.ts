/**
 * The header of an encoded frame of the QUIC transport, as the gateway
 * writes it (`crates/bli-kvm-quic-protocol`, `frame.rs`): 32 bytes, integers
 * most significant byte first. A key frame travels whole on a stream of its
 * own; a delta frame is cut into datagrams, each with this header.
 */

export const HEADER_BYTES = 32;
/** A datagram is never larger, whatever the path would carry. */
export const MAX_DATAGRAM_BYTES = 1200;
/** An encoded frame larger than this is refused. */
export const MAX_FRAME_BYTES = 4 * 1024 * 1024;

const VERSION = 1;
const KIND_KEY = 1;
const KIND_DELTA = 2;
const CODEC_H264 = 1;

export type FrameHeader = {
  key: boolean;
  fragmentIndex: number;
  fragmentCount: number;
  /** New after an encoder restart or a change of size; never zero. */
  stream: number;
  /** Rising by one within a stream; never zero. */
  frame: number;
  /** The key frame this frame depends on; its own id for a key frame. */
  reference: number;
  /** Bytes of the complete frame. */
  size: number;
  /** Microseconds of the board's monotonic clock. */
  captureMicros: number;
};

/**
 * The header at the start of `bytes`, or `null` if it is not one that may be
 * acted on: too short, an unknown version, kind or codec, ids that cannot
 * be, a size or a fragment numbering out of range.
 */
export function decodeHeader(bytes: Uint8Array): FrameHeader | null {
  if (bytes.length < HEADER_BYTES) return null;
  if (bytes[0] !== VERSION || bytes[2] !== CODEC_H264 || bytes[3] !== 0) return null;
  if (bytes[1] !== KIND_KEY && bytes[1] !== KIND_DELTA) return null;
  const view = new DataView(bytes.buffer, bytes.byteOffset, HEADER_BYTES);
  const key = bytes[1] === KIND_KEY;
  const fragmentIndex = view.getUint16(4);
  const fragmentCount = view.getUint16(6);
  const stream = view.getUint32(8);
  const frame = view.getUint32(12);
  const reference = view.getUint32(16);
  const size = view.getUint32(20);
  const high = view.getUint32(24);
  // A capture time beyond what a number holds exactly cannot be a time.
  if (high >= 0x200000) return null;
  const captureMicros = high * 0x100000000 + view.getUint32(28);
  if (stream === 0 || frame === 0 || reference === 0) return null;
  if (size === 0 || size > MAX_FRAME_BYTES) return null;
  if (key) {
    if (reference !== frame || fragmentIndex !== 0 || fragmentCount !== 1) return null;
  } else {
    // A delta frame depends on a key frame sent before it.
    if (reference >= frame) return null;
    if (fragmentCount === 0 || fragmentIndex >= fragmentCount || fragmentCount > size) {
      return null;
    }
  }
  return { key, fragmentIndex, fragmentCount, stream, frame, reference, size, captureMicros };
}
