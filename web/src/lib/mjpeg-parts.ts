/**
 * The JPEG frames of an MJPEG stream (`multipart/x-mixed-replace`), as the
 * upscaled MJPEG view reads it (docs/design/screen-upscaling-v1.md): each
 * part's bytes once it is complete. A part says its length
 * (`Content-Length`, as the board's streamer does); one that does not ends
 * at the next boundary.
 */

/** The boundary a `multipart/x-mixed-replace` content type names, or null. */
export function boundaryOf(contentType: string | null): string | null {
  const found = /boundary="?([^";]+)"?/i.exec(contentType ?? '');
  return found ? found[1].trim() : null;
}

const encoder = new TextEncoder();
const decoder = new TextDecoder();
/** A part's headers end within this many bytes, or the stream is not one. */
const MAX_HEADERS = 4096;

function indexOf(haystack: Uint8Array, needle: Uint8Array, from: number): number {
  const last = haystack.length - needle.length;
  outer: for (let at = from; at <= last; at += 1) {
    for (let index = 0; index < needle.length; index += 1) {
      if (haystack[at + index] !== needle[index]) continue outer;
    }
    return at;
  }
  return -1;
}

export class MjpegParts {
  private buffer = new Uint8Array(0);
  /** Where the current part's body starts, and its length when it says. */
  private body = -1;
  private length: number | null = null;
  private readonly delimiter: Uint8Array;
  private readonly headersEnd = encoder.encode('\r\n\r\n');
  private readonly maxPart: number;

  // Plain fields rather than parameter properties: the node tests load this
  // file with type stripping only.
  constructor(boundary: string, maxPart = 16 * 1024 * 1024) {
    this.delimiter = encoder.encode(`--${boundary}`);
    this.maxPart = maxPart;
  }

  /** Takes the next bytes of the stream; returns the parts they complete. */
  push(chunk: Uint8Array): Uint8Array[] {
    const joined = new Uint8Array(this.buffer.length + chunk.length);
    joined.set(this.buffer);
    joined.set(chunk, this.buffer.length);
    this.buffer = joined;
    const parts: Uint8Array[] = [];
    for (;;) {
      if (this.body < 0) {
        const start = indexOf(this.buffer, this.delimiter, 0);
        if (start < 0) {
          // Keep what may be the start of a delimiter.
          this.buffer = this.buffer.slice(Math.max(0, this.buffer.length - this.delimiter.length));
          break;
        }
        const end = indexOf(this.buffer, this.headersEnd, start);
        if (end < 0) {
          if (this.buffer.length - start > MAX_HEADERS) throw new Error('not a multipart stream');
          this.buffer = this.buffer.slice(start);
          break;
        }
        const headers = decoder.decode(this.buffer.subarray(start, end));
        const length = /\r\ncontent-length:\s*(\d+)/i.exec(headers);
        this.length = length ? Number(length[1]) : null;
        if (this.length !== null && this.length > this.maxPart) throw new Error('part too large');
        this.buffer = this.buffer.slice(end + this.headersEnd.length);
        this.body = 0;
      }
      if (this.length !== null) {
        if (this.buffer.length < this.length) break;
        parts.push(this.buffer.slice(0, this.length));
        this.buffer = this.buffer.slice(this.length);
      } else {
        const next = indexOf(this.buffer, this.delimiter, 0);
        if (next < 0) {
          if (this.buffer.length > this.maxPart) throw new Error('part too large');
          break;
        }
        // The body ends before the CRLF that precedes the delimiter.
        let end = next;
        if (end >= 2 && this.buffer[end - 2] === 0x0d && this.buffer[end - 1] === 0x0a) end -= 2;
        parts.push(this.buffer.slice(0, end));
        this.buffer = this.buffer.slice(next);
      }
      this.body = -1;
      this.length = null;
    }
    return parts;
  }
}
