/**
 * What the decoder must be told about an H.264 stream in Annex B form: its
 * codec string, read from the sequence parameter set that the encoder puts
 * in front of every key frame.
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
