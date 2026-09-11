//! One session protocol, two transports.
//!
//! The API server offers the same live session over a **WebSocket** (browsers, the Flutter app)
//! and over a **raw TCP** socket (runners, the CLI, the sidecar — anything that wants a long-lived
//! connection without an HTTP upgrade). Rather than two protocols that drift, there is one
//! [`Frame`] vocabulary with two encodings:
//!
//! * **TCP** — a 4-byte big-endian length prefix followed by the binary encoding in this module.
//!   Self-delimiting, bounded, and decodable from a partially-filled buffer.
//! * **WebSocket** — the same frames as JSON text (behind the `serde` feature), because a
//!   browser devtools pane that shows readable frames is worth a few bytes.
//!
//! Three things here are worth more than the wire format:
//!
//! 1. **Backpressure is explicit.** A subscriber issues [`Frame::Credit`]; the server may only
//!    send that many events. A runner streaming a 200 MB build log cannot make the server buffer
//!    it, because the server literally has no permission to send.
//! 2. **Resumption is exact.** Every event carries a monotonic sequence per stream, and a
//!    reconnecting client says where it got to. A dropped Wi-Fi connection resumes the log tail
//!    instead of restarting it or silently skipping lines.
//! 3. **Liveness is a state machine.** Missed heartbeats have a defined outcome, so a
//!    half-open TCP connection — the failure that leaves a runner "connected" forever — is
//!    detected rather than waited on.

use core::fmt;

/// Wire protocol version. Bumped only for an incompatible change; [`Frame::Hello`] negotiates it.
pub const PROTOCOL_VERSION: u16 = 1;

/// Largest frame we will encode or accept, on either transport. A log line is kilobytes; a
/// megabyte is already generous, and an unbounded length prefix is a trivial memory exhaustion.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// The name of a stream a client can subscribe to (`run:01H…:logs`, `org:acme:events`).
pub type Stream = String;

/// Monotonic, per-stream, starting at 1. `0` means "from the beginning".
pub type Sequence = u64;

/// What either side can say.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Frame {
    /// Client → server, first frame. Carries the protocol version and, when resuming, the session
    /// the client is continuing.
    Hello {
        version: u16,
        resume: Option<String>,
    },
    /// Server → client, in reply to `Hello`. `resumed` is false when the server could not honour
    /// the resume (session gone, buffer evicted) and the client must re-subscribe from a cursor.
    Welcome {
        session: String,
        resumed: bool,
        heartbeat_seconds: u32,
    },
    /// Either side. `nonce` is echoed in the `Pong`, so a stale reply cannot refresh liveness.
    Ping {
        nonce: u64,
    },
    Pong {
        nonce: u64,
    },
    /// Client → server. `from` is the last sequence the client already has; the server resumes at
    /// `from + 1`. `credit` is how many events it may send before waiting.
    Subscribe {
        stream: Stream,
        from: Sequence,
        credit: u32,
    },
    Unsubscribe {
        stream: Stream,
    },
    /// Client → server. Grants permission to send `additional` more events on `stream`.
    Credit {
        stream: Stream,
        additional: u32,
    },
    /// Server → client. One event, consuming one credit.
    Event {
        stream: Stream,
        sequence: Sequence,
        payload: Vec<u8>,
    },
    /// Server → client. The server dropped events because the client did not keep up and the
    /// buffer wrapped. Saying so is mandatory: a silent gap in a build log is worse than an error.
    Lagged {
        stream: Stream,
        skipped_to: Sequence,
    },
    /// Client → server. A request with a correlation id, answered by `Ack` or `Error`.
    Command {
        id: u64,
        verb: String,
        payload: Vec<u8>,
    },
    Ack {
        id: u64,
        payload: Vec<u8>,
    },
    /// Either side. `code` is stable and machine-readable; `message` is for humans and never
    /// carries a token, a connection string or a subject.
    Error {
        id: Option<u64>,
        code: ErrorCode,
        message: String,
    },
    /// Either side, last frame.
    Close {
        code: ErrorCode,
        message: String,
    },
}

/// Stable error codes. Kept small and closed so both ends can match exhaustively.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ErrorCode {
    /// The `Hello` version is not one we speak.
    UnsupportedVersion,
    /// No valid credential, or it expired mid-session.
    Unauthorized,
    /// Authenticated, but not for this stream.
    Forbidden,
    /// Malformed frame, unknown tag, or a length that exceeds [`MAX_FRAME_BYTES`].
    Protocol,
    /// The client sent faster than its rate limit allows.
    RateLimited,
    /// The stream does not exist (or no longer does).
    NotFound,
    /// The server is going away — deploy, drain, or shutdown. Clients should reconnect.
    GoingAway,
    /// Heartbeats stopped.
    Timeout,
    /// Anything else. Deliberately last, and deliberately vague to the client.
    Internal,
}

impl ErrorCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            ErrorCode::UnsupportedVersion => "unsupported_version",
            ErrorCode::Unauthorized => "unauthorized",
            ErrorCode::Forbidden => "forbidden",
            ErrorCode::Protocol => "protocol",
            ErrorCode::RateLimited => "rate_limited",
            ErrorCode::NotFound => "not_found",
            ErrorCode::GoingAway => "going_away",
            ErrorCode::Timeout => "timeout",
            ErrorCode::Internal => "internal",
        }
    }

    const fn tag(self) -> u8 {
        match self {
            ErrorCode::UnsupportedVersion => 1,
            ErrorCode::Unauthorized => 2,
            ErrorCode::Forbidden => 3,
            ErrorCode::Protocol => 4,
            ErrorCode::RateLimited => 5,
            ErrorCode::NotFound => 6,
            ErrorCode::GoingAway => 7,
            ErrorCode::Timeout => 8,
            ErrorCode::Internal => 9,
        }
    }

    const fn from_tag(tag: u8) -> Option<Self> {
        Some(match tag {
            1 => ErrorCode::UnsupportedVersion,
            2 => ErrorCode::Unauthorized,
            3 => ErrorCode::Forbidden,
            4 => ErrorCode::Protocol,
            5 => ErrorCode::RateLimited,
            6 => ErrorCode::NotFound,
            7 => ErrorCode::GoingAway,
            8 => ErrorCode::Timeout,
            9 => ErrorCode::Internal,
            _ => return None,
        })
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a byte sequence was not a frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodecError {
    /// Not enough bytes yet — call again with more. Not an error condition on a stream.
    Incomplete,
    /// The declared length exceeds [`MAX_FRAME_BYTES`]. Fatal: the stream is desynchronized or hostile.
    TooLarge { declared: usize },
    /// Unknown tag, truncated field, or invalid UTF-8. Fatal for the connection.
    Malformed,
}

// ---------------------------------------------------------------------------------------------
// Binary encoding. Every variable-length field is length-prefixed, so decoding never scans for a
// terminator and a hostile length can only ever fail the frame, not read past it.
// ---------------------------------------------------------------------------------------------

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}
fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}
fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_be_bytes());
}
fn put_bytes(out: &mut Vec<u8>, value: &[u8]) {
    put_u32(out, u32::try_from(value.len()).unwrap_or(u32::MAX));
    out.extend_from_slice(value);
}
fn put_str(out: &mut Vec<u8>, value: &str) {
    put_bytes(out, value.as_bytes());
}
fn put_opt_str(out: &mut Vec<u8>, value: Option<&str>) {
    match value {
        Some(v) => {
            out.push(1);
            put_str(out, v);
        }
        None => out.push(0),
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], CodecError> {
        let end = self.at.checked_add(n).ok_or(CodecError::Malformed)?;
        let slice = self.bytes.get(self.at..end).ok_or(CodecError::Malformed)?;
        self.at = end;
        Ok(slice)
    }
    fn u8(&mut self) -> Result<u8, CodecError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, CodecError> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32, CodecError> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Result<u64, CodecError> {
        let b = self.take(8)?;
        Ok(u64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }
    fn bytes(&mut self) -> Result<Vec<u8>, CodecError> {
        let len = self.u32()? as usize;
        Ok(self.take(len)?.to_vec())
    }
    fn string(&mut self) -> Result<String, CodecError> {
        let raw = self.bytes()?;
        String::from_utf8(raw).map_err(|_| CodecError::Malformed)
    }
    fn opt_string(&mut self) -> Result<Option<String>, CodecError> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.string()?)),
            _ => Err(CodecError::Malformed),
        }
    }
    const fn done(&self) -> bool {
        self.at == self.bytes.len()
    }
}

impl Frame {
    const fn tag(&self) -> u8 {
        match self {
            Frame::Hello { .. } => 1,
            Frame::Welcome { .. } => 2,
            Frame::Ping { .. } => 3,
            Frame::Pong { .. } => 4,
            Frame::Subscribe { .. } => 5,
            Frame::Unsubscribe { .. } => 6,
            Frame::Credit { .. } => 7,
            Frame::Event { .. } => 8,
            Frame::Lagged { .. } => 9,
            Frame::Command { .. } => 10,
            Frame::Ack { .. } => 11,
            Frame::Error { .. } => 12,
            Frame::Close { .. } => 13,
        }
    }

    /// Encode the frame body (no length prefix).
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32);
        out.push(self.tag());
        match self {
            Frame::Hello { version, resume } => {
                put_u16(&mut out, *version);
                put_opt_str(&mut out, resume.as_deref());
            }
            Frame::Welcome {
                session,
                resumed,
                heartbeat_seconds,
            } => {
                put_str(&mut out, session);
                out.push(u8::from(*resumed));
                put_u32(&mut out, *heartbeat_seconds);
            }
            Frame::Ping { nonce } | Frame::Pong { nonce } => put_u64(&mut out, *nonce),
            Frame::Subscribe {
                stream,
                from,
                credit,
            } => {
                put_str(&mut out, stream);
                put_u64(&mut out, *from);
                put_u32(&mut out, *credit);
            }
            Frame::Unsubscribe { stream } => put_str(&mut out, stream),
            Frame::Credit { stream, additional } => {
                put_str(&mut out, stream);
                put_u32(&mut out, *additional);
            }
            Frame::Event {
                stream,
                sequence,
                payload,
            } => {
                put_str(&mut out, stream);
                put_u64(&mut out, *sequence);
                put_bytes(&mut out, payload);
            }
            Frame::Lagged { stream, skipped_to } => {
                put_str(&mut out, stream);
                put_u64(&mut out, *skipped_to);
            }
            Frame::Command { id, verb, payload } => {
                put_u64(&mut out, *id);
                put_str(&mut out, verb);
                put_bytes(&mut out, payload);
            }
            Frame::Ack { id, payload } => {
                put_u64(&mut out, *id);
                put_bytes(&mut out, payload);
            }
            Frame::Error { id, code, message } => {
                match id {
                    Some(id) => {
                        out.push(1);
                        put_u64(&mut out, *id);
                    }
                    None => out.push(0),
                }
                out.push(code.tag());
                put_str(&mut out, message);
            }
            Frame::Close { code, message } => {
                out.push(code.tag());
                put_str(&mut out, message);
            }
        }
        out
    }

    /// Encode with the 4-byte big-endian length prefix used on the TCP transport.
    #[must_use]
    pub fn encode_framed(&self) -> Vec<u8> {
        let body = self.encode();
        let mut out = Vec::with_capacity(body.len() + 4);
        put_u32(&mut out, u32::try_from(body.len()).unwrap_or(u32::MAX));
        out.extend_from_slice(&body);
        out
    }

    /// Decode a frame body (no length prefix).
    ///
    /// # Errors
    /// [`CodecError::Malformed`] for an unknown tag, a truncated field, invalid UTF-8, or trailing
    /// bytes — a frame that decodes but leaves bytes over means the two ends disagree, and
    /// continuing would be guessing.
    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut r = Reader::new(bytes);
        let frame = match r.u8()? {
            1 => Frame::Hello {
                version: r.u16()?,
                resume: r.opt_string()?,
            },
            2 => Frame::Welcome {
                session: r.string()?,
                resumed: match r.u8()? {
                    0 => false,
                    1 => true,
                    _ => return Err(CodecError::Malformed),
                },
                heartbeat_seconds: r.u32()?,
            },
            3 => Frame::Ping { nonce: r.u64()? },
            4 => Frame::Pong { nonce: r.u64()? },
            5 => Frame::Subscribe {
                stream: r.string()?,
                from: r.u64()?,
                credit: r.u32()?,
            },
            6 => Frame::Unsubscribe {
                stream: r.string()?,
            },
            7 => Frame::Credit {
                stream: r.string()?,
                additional: r.u32()?,
            },
            8 => Frame::Event {
                stream: r.string()?,
                sequence: r.u64()?,
                payload: r.bytes()?,
            },
            9 => Frame::Lagged {
                stream: r.string()?,
                skipped_to: r.u64()?,
            },
            10 => Frame::Command {
                id: r.u64()?,
                verb: r.string()?,
                payload: r.bytes()?,
            },
            11 => Frame::Ack {
                id: r.u64()?,
                payload: r.bytes()?,
            },
            12 => {
                let id = match r.u8()? {
                    0 => None,
                    1 => Some(r.u64()?),
                    _ => return Err(CodecError::Malformed),
                };
                let code = ErrorCode::from_tag(r.u8()?).ok_or(CodecError::Malformed)?;
                Frame::Error {
                    id,
                    code,
                    message: r.string()?,
                }
            }
            13 => Frame::Close {
                code: ErrorCode::from_tag(r.u8()?).ok_or(CodecError::Malformed)?,
                message: r.string()?,
            },
            _ => return Err(CodecError::Malformed),
        };
        if !r.done() {
            return Err(CodecError::Malformed);
        }
        Ok(frame)
    }

    /// Read one length-prefixed frame from the front of `buffer`, returning it and how many bytes
    /// it consumed. [`CodecError::Incomplete`] means "await more bytes", not "give up".
    ///
    /// # Errors
    /// [`CodecError::Incomplete`], [`CodecError::TooLarge`], [`CodecError::Malformed`].
    pub fn decode_framed(buffer: &[u8]) -> Result<(Self, usize), CodecError> {
        if buffer.len() < 4 {
            return Err(CodecError::Incomplete);
        }
        let declared = u32::from_be_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]) as usize;
        if declared > MAX_FRAME_BYTES {
            return Err(CodecError::TooLarge { declared });
        }
        let end = 4 + declared;
        if buffer.len() < end {
            return Err(CodecError::Incomplete);
        }
        Ok((Frame::decode(&buffer[4..end])?, end))
    }
}

/// Incremental decoder for a byte stream. Feed it whatever `read()` returned; take whole frames out.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buffer: Vec<u8>,
}

impl FrameDecoder {
    #[must_use]
    pub const fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// Next complete frame, if there is one.
    ///
    /// # Errors
    /// [`CodecError::TooLarge`] or [`CodecError::Malformed`]; both are fatal for the connection.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, CodecError> {
        match Frame::decode_framed(&self.buffer) {
            Ok((frame, consumed)) => {
                self.buffer.drain(..consumed);
                Ok(Some(frame))
            }
            Err(CodecError::Incomplete) => Ok(None),
            Err(other) => Err(other),
        }
    }

    /// Bytes held but not yet a whole frame. Useful as a metric: a number that only grows is a
    /// peer sending a length prefix it never fills.
    #[must_use]
    pub const fn pending(&self) -> usize {
        self.buffer.len()
    }
}

// ---------------------------------------------------------------------------------------------
// Flow control
// ---------------------------------------------------------------------------------------------

/// Credit-based backpressure for one stream. The server may send only what the client granted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FlowControl {
    credit: u32,
    max_credit: u32,
}

impl FlowControl {
    /// `max_credit` bounds how much a client may bank, so a client cannot grant a million and
    /// then stop reading, turning our send buffer into its memory.
    #[must_use]
    pub const fn new(initial: u32, max_credit: u32) -> Self {
        Self {
            credit: if initial > max_credit {
                max_credit
            } else {
                initial
            },
            max_credit,
        }
    }

    #[must_use]
    pub const fn available(self) -> u32 {
        self.credit
    }

    #[must_use]
    pub const fn may_send(self) -> bool {
        self.credit > 0
    }

    /// Consume one credit. Returns false when there is none, and the caller must not send.
    pub fn consume(&mut self) -> bool {
        if self.credit == 0 {
            return false;
        }
        self.credit -= 1;
        true
    }

    /// Grant more, saturating at `max_credit`. Returns how much was actually added, so a peer
    /// that over-grants can be told rather than silently ignored.
    pub fn grant(&mut self, additional: u32) -> u32 {
        let before = self.credit;
        self.credit = self.credit.saturating_add(additional).min(self.max_credit);
        self.credit - before
    }
}

// ---------------------------------------------------------------------------------------------
// Liveness
// ---------------------------------------------------------------------------------------------

/// What the heartbeat machine wants done at this instant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Liveness {
    /// Nothing to do yet.
    Idle,
    /// Send a `Ping` with this nonce.
    SendPing { nonce: u64 },
    /// The peer missed too many; close with [`ErrorCode::Timeout`].
    Dead,
}

/// Detects a peer that has stopped answering — including a half-open TCP connection, where the
/// socket still looks writable and no data ever arrives.
#[derive(Clone, Copy, Debug)]
pub struct Heartbeat {
    interval_seconds: u64,
    max_missed: u32,
    last_seen: u64,
    outstanding: Option<u64>,
    missed: u32,
    next_nonce: u64,
}

impl Heartbeat {
    #[must_use]
    pub const fn new(interval_seconds: u64, max_missed: u32, now: u64) -> Self {
        Self {
            interval_seconds,
            max_missed,
            last_seen: now,
            outstanding: None,
            missed: 0,
            next_nonce: 1,
        }
    }

    /// Any frame from the peer proves liveness, not just a `Pong`.
    pub fn saw_traffic(&mut self, now: u64) {
        self.last_seen = now;
        self.missed = 0;
        self.outstanding = None;
    }

    /// Record a `Pong`. A nonce we did not send is ignored: it must not refresh liveness, or a
    /// replayed pong keeps a dead session alive forever.
    pub fn saw_pong(&mut self, nonce: u64, now: u64) -> bool {
        if self.outstanding == Some(nonce) {
            self.saw_traffic(now);
            true
        } else {
            false
        }
    }

    /// Advance the clock and say what to do.
    pub fn poll(&mut self, now: u64) -> Liveness {
        if now.saturating_sub(self.last_seen) < self.interval_seconds {
            return Liveness::Idle;
        }
        if self.outstanding.is_some() {
            self.missed = self.missed.saturating_add(1);
            if self.missed >= self.max_missed {
                return Liveness::Dead;
            }
        }
        let nonce = self.next_nonce;
        self.next_nonce = self.next_nonce.wrapping_add(1);
        self.outstanding = Some(nonce);
        self.last_seen = now;
        Liveness::SendPing { nonce }
    }
}

// ---------------------------------------------------------------------------------------------
// Resumption
// ---------------------------------------------------------------------------------------------

/// Where a subscriber has got to on one stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cursor {
    pub delivered: Sequence,
}

impl Cursor {
    #[must_use]
    pub const fn start() -> Self {
        Self { delivered: 0 }
    }

    /// Accept an event, or say why not. Out-of-order and duplicate events are rejected rather
    /// than tolerated: a client that silently accepts a repeat writes a duplicated log line.
    ///
    /// # Errors
    /// The expected sequence, when `sequence` is not it.
    pub fn accept(&mut self, sequence: Sequence) -> Result<(), Sequence> {
        let expected = self.delivered + 1;
        if sequence == expected {
            self.delivered = sequence;
            Ok(())
        } else {
            Err(expected)
        }
    }

    /// The server could not serve from `delivered + 1` and jumped ahead. Record the jump so the
    /// gap is visible to the caller (who should surface "N lines dropped", never hide it).
    pub fn jump(&mut self, skipped_to: Sequence) -> u64 {
        let gap = skipped_to.saturating_sub(self.delivered + 1);
        self.delivered = skipped_to;
        gap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples() -> Vec<Frame> {
        vec![
            Frame::Hello {
                version: PROTOCOL_VERSION,
                resume: None,
            },
            Frame::Hello {
                version: 7,
                resume: Some("sess_01H".into()),
            },
            Frame::Welcome {
                session: "sess_01H".into(),
                resumed: true,
                heartbeat_seconds: 20,
            },
            Frame::Ping { nonce: u64::MAX },
            Frame::Pong { nonce: 0 },
            Frame::Subscribe {
                stream: "run:01H:logs".into(),
                from: 42,
                credit: 128,
            },
            Frame::Unsubscribe {
                stream: "run:01H:logs".into(),
            },
            Frame::Credit {
                stream: "run:01H:logs".into(),
                additional: 64,
            },
            Frame::Event {
                stream: "s".into(),
                sequence: 1,
                payload: vec![0, 255, 10, 13],
            },
            Frame::Event {
                stream: "unicode:✓".into(),
                sequence: 9,
                payload: Vec::new(),
            },
            Frame::Lagged {
                stream: "s".into(),
                skipped_to: 900,
            },
            Frame::Command {
                id: 5,
                verb: "runs.cancel".into(),
                payload: b"{}".to_vec(),
            },
            Frame::Ack {
                id: 5,
                payload: b"{\"ok\":true}".to_vec(),
            },
            Frame::Error {
                id: Some(5),
                code: ErrorCode::Forbidden,
                message: "no".into(),
            },
            Frame::Error {
                id: None,
                code: ErrorCode::RateLimited,
                message: String::new(),
            },
            Frame::Close {
                code: ErrorCode::GoingAway,
                message: "deploy".into(),
            },
        ]
    }

    #[test]
    fn every_frame_round_trips() {
        for frame in samples() {
            let encoded = frame.encode();
            assert_eq!(
                Frame::decode(&encoded).unwrap(),
                frame,
                "body round-trip for {frame:?}"
            );
            let framed = frame.encode_framed();
            let (decoded, consumed) = Frame::decode_framed(&framed).unwrap();
            assert_eq!(decoded, frame, "framed round-trip for {frame:?}");
            assert_eq!(consumed, framed.len());
        }
    }

    #[test]
    fn a_stream_split_at_every_byte_still_decodes() {
        let frames = samples();
        let mut wire = Vec::new();
        for frame in &frames {
            wire.extend_from_slice(&frame.encode_framed());
        }
        // Feed one byte at a time: the decoder must never mis-frame or lose a frame.
        let mut decoder = FrameDecoder::new();
        let mut out = Vec::new();
        for byte in &wire {
            decoder.feed(&[*byte]);
            while let Some(frame) = decoder.next_frame().unwrap() {
                out.push(frame);
            }
        }
        assert_eq!(out, frames);
        assert_eq!(decoder.pending(), 0);
    }

    #[test]
    fn a_partial_frame_is_incomplete_not_an_error() {
        let framed = Frame::Ping { nonce: 1 }.encode_framed();
        for cut in 0..framed.len() {
            assert_eq!(
                Frame::decode_framed(&framed[..cut]),
                Err(CodecError::Incomplete),
                "cut {cut}"
            );
        }
    }

    #[test]
    fn an_oversized_length_prefix_is_refused_without_allocating() {
        let mut hostile = Vec::new();
        put_u32(&mut hostile, u32::MAX);
        assert_eq!(
            Frame::decode_framed(&hostile),
            Err(CodecError::TooLarge {
                declared: u32::MAX as usize
            })
        );
    }

    #[test]
    fn junk_and_trailing_bytes_are_malformed() {
        assert_eq!(Frame::decode(&[]), Err(CodecError::Malformed));
        assert_eq!(Frame::decode(&[200]), Err(CodecError::Malformed));
        // A valid Ping with one extra byte: the ends disagree, so refuse rather than guess.
        let mut body = Frame::Ping { nonce: 1 }.encode();
        body.push(0);
        assert_eq!(Frame::decode(&body), Err(CodecError::Malformed));
        // Truncated string length.
        assert_eq!(
            Frame::decode(&[6, 0, 0, 0, 9, b'a']),
            Err(CodecError::Malformed)
        );
        // Invalid UTF-8 in a stream name.
        assert_eq!(
            Frame::decode(&[6, 0, 0, 0, 1, 0xff]),
            Err(CodecError::Malformed)
        );
    }

    #[test]
    fn the_server_cannot_send_without_credit() {
        let mut flow = FlowControl::new(2, 1024);
        assert!(flow.consume() && flow.consume());
        assert!(!flow.consume());
        assert!(!flow.may_send());
        assert_eq!(flow.grant(10), 10);
        assert_eq!(flow.available(), 10);
    }

    #[test]
    fn banked_credit_is_capped_so_a_client_cannot_make_us_buffer() {
        let mut flow = FlowControl::new(1000, 64);
        assert_eq!(flow.available(), 64, "initial credit is clamped to the cap");
        assert_eq!(
            flow.grant(1_000_000),
            0,
            "over-granting adds nothing once at the cap"
        );
        assert!(flow.consume());
        assert_eq!(
            flow.grant(10),
            1,
            "only the room that actually exists is granted"
        );
    }

    #[test]
    fn heartbeats_detect_a_peer_that_stops_answering() {
        let mut hb = Heartbeat::new(10, 3, 0);
        assert_eq!(hb.poll(5), Liveness::Idle);
        let Liveness::SendPing { nonce } = hb.poll(10) else {
            panic!("expected a ping at the interval")
        };
        // A pong for a nonce we never sent must not count.
        assert!(!hb.saw_pong(nonce.wrapping_add(1), 11));
        assert!(hb.saw_pong(nonce, 11));
        assert_eq!(hb.poll(12), Liveness::Idle);

        // Now go silent: ping, miss, miss, dead.
        let mut hb = Heartbeat::new(10, 3, 0);
        assert!(matches!(hb.poll(10), Liveness::SendPing { .. }));
        assert!(matches!(hb.poll(20), Liveness::SendPing { .. }));
        assert!(matches!(hb.poll(30), Liveness::SendPing { .. }));
        assert_eq!(hb.poll(40), Liveness::Dead);
    }

    #[test]
    fn any_traffic_counts_as_liveness() {
        let mut hb = Heartbeat::new(10, 2, 0);
        assert!(matches!(hb.poll(10), Liveness::SendPing { .. }));
        hb.saw_traffic(11); // an Event or a Command, not a Pong
        assert_eq!(hb.poll(15), Liveness::Idle);
    }

    #[test]
    fn a_cursor_refuses_gaps_and_duplicates_and_reports_a_jump() {
        let mut cursor = Cursor::start();
        assert!(cursor.accept(1).is_ok());
        assert!(cursor.accept(2).is_ok());
        assert_eq!(
            cursor.accept(2),
            Err(3),
            "a duplicate is refused with the expected sequence"
        );
        assert_eq!(
            cursor.accept(9),
            Err(3),
            "a gap is refused, not silently accepted"
        );
        assert_eq!(
            cursor.jump(10),
            7,
            "an explicit Lagged reports how many were skipped"
        );
        assert!(cursor.accept(11).is_ok());
    }

    #[test]
    fn resume_starts_after_the_last_delivered_sequence() {
        let mut cursor = Cursor::start();
        for sequence in 1..=5 {
            cursor.accept(sequence).unwrap();
        }
        let resume = Frame::Subscribe {
            stream: "s".into(),
            from: cursor.delivered,
            credit: 32,
        };
        let Frame::Subscribe { from, .. } = resume else {
            unreachable!()
        };
        assert_eq!(from, 5, "the server resumes at 6");
    }
}
