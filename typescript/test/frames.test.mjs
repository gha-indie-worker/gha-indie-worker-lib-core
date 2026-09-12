import test from 'node:test';
import assert from 'node:assert/strict';

import { encodeFrame, FrameDecoder, ProtocolError, MAX_FRAME_BYTES } from '../src/frames.mjs';

const bytes = (text) => new TextEncoder().encode(text);

test('round trips one frame', () => {
  const decoder = new FrameDecoder();
  decoder.feed(encodeFrame(bytes('{"kind":"ping"}')));
  assert.equal(decoder.nextText(), '{"kind":"ping"}');
  assert.equal(decoder.next(), null);
  assert.equal(decoder.buffered, 0);
});

test('reassembles across arbitrary chunk boundaries', () => {
  const payloads = ['hello', 'world!!', 'third'];
  const wire = new Uint8Array(payloads.flatMap((p) => [...encodeFrame(bytes(p))]));
  for (let size = 1; size <= wire.length; size += 1) {
    const decoder = new FrameDecoder();
    const got = [];
    for (let i = 0; i < wire.length; i += size) {
      decoder.feed(wire.slice(i, i + size));
      for (let frame = decoder.nextText(); frame !== null; frame = decoder.nextText()) got.push(frame);
    }
    assert.deepEqual(got, payloads, `chunk size ${size}`);
    assert.equal(decoder.buffered, 0);
  }
});

test('oversized declarations are refused before the body is buffered', () => {
  const decoder = new FrameDecoder(16);
  const header = new Uint8Array(4);
  new DataView(header.buffer).setUint32(0, 1000000, false);
  decoder.feed(header);
  assert.throws(() => decoder.next(), (e) => e instanceof ProtocolError && e.kind === 'frame-too-large');
  assert.ok(decoder.poisoned);
  decoder.feed(encodeFrame(bytes('ok'), 16));
  assert.equal(decoder.next(), null);
});

test('zero length frames are a protocol error', () => {
  assert.throws(() => encodeFrame(new Uint8Array(0)), (e) => e.kind === 'empty-frame');
  const decoder = new FrameDecoder();
  decoder.feed(new Uint8Array(4));
  assert.throws(() => decoder.next(), (e) => e.kind === 'empty-frame');
});

test('the limit matches the Rust crate', () => {
  assert.equal(MAX_FRAME_BYTES, 1048576);
});
