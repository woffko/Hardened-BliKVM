/**
 * The frames of a group's stream (`docs/design/quic-transport-v1.md`,
 * "Groups of pictures"): the gateway writes a key frame and then the delta
 * frames that refer to it, each a 32-byte header and the frame's bytes, in
 * order. This cuts the stream into those records as its bytes come, each
 * record handed on as soon as it is whole.
 */
import { decodeHeader, HEADER_BYTES } from './frame.ts';

export class GroupRecords {
  private buffer = new Uint8Array(0);
  /** The size of the record under way, once its header is read. */
  private size: number | null = null;
  private first = true;

  /**
   * Takes the next bytes of the stream; returns the records they complete.
   * Throws where the stream is not a group: a header that cannot be (a
   * frame larger than any among them), or a first record that is not a key
   * frame or a later one that is.
   */
  push(chunk: Uint8Array): Uint8Array[] {
    const joined = new Uint8Array(this.buffer.length + chunk.length);
    joined.set(this.buffer);
    joined.set(chunk, this.buffer.length);
    this.buffer = joined;
    const records: Uint8Array[] = [];
    for (;;) {
      if (this.size === null) {
        if (this.buffer.length < HEADER_BYTES) break;
        const header = decodeHeader(this.buffer);
        if (!header) throw new Error('not a frame header');
        if (header.key !== this.first) throw new Error('not a group of pictures');
        if (!header.key && header.fragmentCount !== 1)
          throw new Error('a delta frame in fragments');
        this.first = false;
        this.size = HEADER_BYTES + header.size;
      }
      if (this.buffer.length < this.size) break;
      records.push(this.buffer.slice(0, this.size));
      this.buffer = this.buffer.slice(this.size);
      this.size = null;
    }
    return records;
  }
}
