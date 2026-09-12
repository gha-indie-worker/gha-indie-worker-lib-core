#![forbid(unsafe_code)]

//! Frame codecs for the two stateful avenues.
//!
//! * [`tcp`] — the length-prefixed JSON framing used on `GHA_INDIE_WORKER_*_TCP_BIND`.
//! * [`ws`] — the same payload discipline for websocket text frames, plus the
//!   sequencing rules that make `/v1/ws` log replay exact.
//!
//! Both are byte-level and synchronous: they take and return `Vec<u8>`/`&[u8]`
//! and never touch a socket, a runtime or a clock. That is what lets the same
//! code run in the servers, in the CLI, in the desktop app and under `cargo test`
//! with no I/O at all.

pub mod tcp;
pub mod ws;

use core::fmt;

/// The hard ceiling on one decoded payload, on either avenue. A peer that
/// announces more is disconnected before a single byte of body is buffered.
pub const MAX_FRAME_BYTES: usize = 1_048_576;

/// Everything that can go wrong framing or deframing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    /// The declared length exceeds [`MAX_FRAME_BYTES`].
    FrameTooLarge { declared: usize, limit: usize },
    /// A zero-length frame. The protocol has no empty frame.
    EmptyFrame,
    /// The payload was not UTF-8, so it cannot be JSON.
    NotUtf8,
    /// A sequence number went backwards or skipped.
    OutOfOrder { expected: u64, got: u64 },
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtocolError::FrameTooLarge { declared, limit } => {
                write!(f, "frame declares {declared} bytes, limit is {limit}")
            }
            ProtocolError::EmptyFrame => f.write_str("frame is empty"),
            ProtocolError::NotUtf8 => f.write_str("frame payload is not UTF-8"),
            ProtocolError::OutOfOrder { expected, got } => {
                write!(f, "expected sequence {expected}, got {got}")
            }
        }
    }
}

impl std::error::Error for ProtocolError {}
