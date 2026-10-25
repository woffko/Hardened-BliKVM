/**
 * What the decoder must be told about an H.264 stream in Annex B form: its
 * codec string, read from the sequence parameter set that the encoder puts
 * in front of every key frame, and what the board's samples mean.
 */

const NAL_SPS = 7;

/**
 * `avc1.PPCCLL` (profile, constraint flags, level, as the parameter set
 * gives them), or `null` if `frame` has no sequence parameter set.
 */
export function codecOf(frame: Uint8Array): string | null {
  // NAL units start after 00 00 01; a four-byte start code ends the same way.
  for (let index = 0; index + 6 < frame.length; index += 1) {
    if (frame[index] !== 0 || frame[index + 1] !== 0 || frame[index + 2] !== 1) continue;
    const nal = index + 3;
    if ((frame[nal] & 0x1f) === NAL_SPS) {
      const hex = (byte: number) => byte.toString(16).padStart(2, '0');
      return `avc1.${hex(frame[nal + 1])}${hex(frame[nal + 2])}${hex(frame[nal + 3])}`;
    }
    index = nal;
  }
  return null;
}

/**
 * What the board's samples mean: the capture's JPEG YCbCr, given to the
 * encoders as it is, is of the full range with the BT.601 matrix, of an sRGB
 * picture (Chromium calls it JPEG's colour space). The encoders say so in
 * their streams since the colour fix; an encoder of an older system does not,
 * and a decoder that is not told takes the narrow range with the BT.709
 * matrix, which draws the picture darker than the MJPEG one, with other
 * colours. Told here, the page draws both alike.
 */
export const BOARD_COLOR_SPACE: VideoColorSpaceInit = {
  primaries: 'bt709',
  transfer: 'iec61966-2-1',
  matrix: 'smpte170m',
  fullRange: true
};

/** The configuration of the decoder for a stream of `codec`. */
export function decoderConfig(codec: string): VideoDecoderConfig {
  return { codec, optimizeForLatency: true, colorSpace: { ...BOARD_COLOR_SPACE } };
}
