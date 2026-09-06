// Length-prefixed JSON framing, mirroring src/protocol/tcp.rs.
//
//   [u32 big-endian length][that many bytes of UTF-8 JSON]
//
// The length is read before any body is buffered, so an oversized declaration
// costs four bytes and a disconnect. Zero dependencies; works in Node and in the
// browser (Uint8Array in, Uint8Array out).

export const MAX_FRAME_BYTES = 1048576;
export const HEADER_BYTES = 4;

export class ProtocolError extends Error {
  constructor(kind, detail) {
    super(detail);
    this.name = 'ProtocolError';
    this.kind = kind;
  }
}

/** Frame one payload. @param {Uint8Array} payload */
export function encodeFrame(payload, limit = MAX_FRAME_BYTES) {
  if (payload.length === 0) throw new ProtocolError('empty-frame', 'frame is empty');
  if (payload.length > limit) {
    throw new ProtocolError('frame-too-large', `frame declares ${payload.length} bytes, limit is ${limit}`);
  }
  const out = new Uint8Array(HEADER_BYTES + payload.length);
  new DataView(out.buffer).setUint32(0, payload.length, false);
  out.set(payload, HEADER_BYTES);
  return out;
}

/** Incremental decoder: feed arbitrary chunks, pull whole payloads. */
export class FrameDecoder {
  #buffer = new Uint8Array(0);
  #limit;
  #poisoned = false;

  constructor(limit = MAX_FRAME_BYTES) {
    this.#limit = limit;
  }

  get buffered() {
    return this.#buffer.length;
  }

  get poisoned() {
    return this.#poisoned;
  }

  feed(chunk) {
    if (this.#poisoned) return;
    const next = new Uint8Array(this.#buffer.length + chunk.length);
    next.set(this.#buffer, 0);
    next.set(chunk, this.#buffer.length);
    this.#buffer = next;
  }

  /** @returns {Uint8Array|null} */
  next() {
    if (this.#poisoned || this.#buffer.length < HEADER_BYTES) return null;
    const declared = new DataView(
      this.#buffer.buffer,
      this.#buffer.byteOffset,
      this.#buffer.byteLength,
    ).getUint32(0, false);
    if (declared === 0) {
      this.#poisoned = true;
      throw new ProtocolError('empty-frame', 'frame is empty');
    }
    if (declared > this.#limit) {
      this.#poisoned = true;
      throw new ProtocolError('frame-too-large', `frame declares ${declared} bytes, limit is ${this.#limit}`);
    }
    if (this.#buffer.length < HEADER_BYTES + declared) return null;
    const payload = this.#buffer.slice(HEADER_BYTES, HEADER_BYTES + declared);
    this.#buffer = this.#buffer.slice(HEADER_BYTES + declared);
    return payload;
  }

  /** @returns {string|null} */
  nextText() {
    const payload = this.next();
    return payload === null ? null : new TextDecoder('utf-8', { fatal: true }).decode(payload);
  }
}
