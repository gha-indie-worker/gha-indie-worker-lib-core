#![forbid(unsafe_code)]

//! Websocket payload discipline for `/v1/ws`.
//!
//! A websocket already frames for us, so there is no length prefix here. What
//! this module owns instead is the part the transport does *not* give you for
//! free:
//!
//! * the same [`MAX_FRAME_BYTES`] ceiling as the TCP avenue, applied before a
//!   text frame is parsed;
//! * per-direction sequence numbers, so log replay after a reconnect is exact
//!   rather than approximately right;
//! * the resume rule: a client reconnects with the last sequence it saw, and the
//!   server replays from there or tells it the window is gone.
//!
//! The same `WsCommand` / `WsEvent` envelopes that `gha-indie-worker-interfaces`
//! defines travel over this; nothing here parses them, so this module stays
//! usable when the `embedded-schemas` feature is off.

use super::{ProtocolError, MAX_FRAME_BYTES};

/// Check an inbound text frame before anything tries to parse it.
///
/// # Errors
///
/// [`ProtocolError::EmptyFrame`] or [`ProtocolError::FrameTooLarge`].
pub fn accept_text(frame: &str) -> Result<&str, ProtocolError> {
    accept_text_with_limit(frame, MAX_FRAME_BYTES)
}

/// As [`accept_text`], against an explicit limit.
///
/// # Errors
///
/// [`ProtocolError::EmptyFrame`] or [`ProtocolError::FrameTooLarge`].
pub fn accept_text_with_limit(frame: &str, limit: usize) -> Result<&str, ProtocolError> {
    if frame.trim().is_empty() {
        return Err(ProtocolError::EmptyFrame);
    }
    if frame.len() > limit {
        return Err(ProtocolError::FrameTooLarge {
            declared: frame.len(),
            limit,
        });
    }
    Ok(frame)
}

/// Check an inbound binary frame, which must still be UTF-8 JSON on this avenue.
///
/// # Errors
///
/// [`ProtocolError::EmptyFrame`], [`ProtocolError::FrameTooLarge`] or
/// [`ProtocolError::NotUtf8`].
pub fn accept_binary(frame: &[u8]) -> Result<&str, ProtocolError> {
    if frame.is_empty() {
        return Err(ProtocolError::EmptyFrame);
    }
    if frame.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge {
            declared: frame.len(),
            limit: MAX_FRAME_BYTES,
        });
    }
    core::str::from_utf8(frame).map_err(|_| ProtocolError::NotUtf8)
}

/// One direction of a connection's sequence numbering.
///
/// Sequences are dense and start at zero, which is what lets a reconnecting
/// client say "I have through 41" and get 42 onwards.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Sequencer {
    next: u64,
}

impl Sequencer {
    #[must_use]
    pub const fn new() -> Self {
        Self { next: 0 }
    }

    /// Resume at `last_seen + 1`.
    #[must_use]
    pub const fn resuming_after(last_seen: u64) -> Self {
        Self {
            next: last_seen.saturating_add(1),
        }
    }

    /// The sequence the next emitted frame will carry.
    #[must_use]
    pub const fn peek(&self) -> u64 {
        self.next
    }

    /// Take the next sequence for an outbound frame.
    pub fn issue(&mut self) -> u64 {
        let current = self.next;
        self.next = self.next.saturating_add(1);
        current
    }

    /// Accept an inbound frame's sequence, requiring density.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::OutOfOrder`] when the peer skipped or repeated.
    pub fn accept(&mut self, sequence: u64) -> Result<(), ProtocolError> {
        if sequence != self.next {
            return Err(ProtocolError::OutOfOrder {
                expected: self.next,
                got: sequence,
            });
        }
        self.next = self.next.saturating_add(1);
        Ok(())
    }
}

/// What a server can do with a client's resume request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Resume {
    /// Replay `from..=through`.
    Replay { from: u64, through: u64 },
    /// The client is already current; stream live.
    UpToDate,
    /// The retained window no longer covers what the client asked for. The
    /// client must resubscribe from `earliest_retained` and reconcile.
    WindowLost { earliest_retained: u64 },
}

/// Decide how to answer a resume request.
///
/// `retained` is the inclusive range the server can still serve;
/// `client_last_seen` is `None` for a fresh subscription.
#[must_use]
pub fn plan_resume(retained: (u64, u64), client_last_seen: Option<u64>) -> Resume {
    let (earliest, latest) = retained;
    let Some(last_seen) = client_last_seen else {
        return Resume::Replay {
            from: earliest,
            through: latest,
        };
    };
    if last_seen >= latest {
        return Resume::UpToDate;
    }
    let wanted = last_seen.saturating_add(1);
    if wanted < earliest {
        return Resume::WindowLost {
            earliest_retained: earliest,
        };
    }
    Resume::Replay {
        from: wanted,
        through: latest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_and_empty_text_frames_are_refused() {
        assert_eq!(accept_text("   ").unwrap_err(), ProtocolError::EmptyFrame);
        let big = "x".repeat(64);
        assert_eq!(
            accept_text_with_limit(&big, 16).unwrap_err(),
            ProtocolError::FrameTooLarge {
                declared: 64,
                limit: 16
            }
        );
        assert_eq!(accept_text_with_limit("{}", 16).unwrap(), "{}");
    }

    #[test]
    fn binary_frames_must_still_be_utf8() {
        assert_eq!(accept_binary(b"{}").unwrap(), "{}");
        assert_eq!(accept_binary(&[0xff]).unwrap_err(), ProtocolError::NotUtf8);
        assert_eq!(accept_binary(b"").unwrap_err(), ProtocolError::EmptyFrame);
    }

    #[test]
    fn sequences_are_dense_in_both_directions() {
        let mut out = Sequencer::new();
        assert_eq!(out.issue(), 0);
        assert_eq!(out.issue(), 1);
        assert_eq!(out.peek(), 2);

        let mut inbound = Sequencer::new();
        assert!(inbound.accept(0).is_ok());
        assert!(inbound.accept(1).is_ok());
        assert_eq!(
            inbound.accept(3).unwrap_err(),
            ProtocolError::OutOfOrder {
                expected: 2,
                got: 3
            }
        );
        // a rejected frame does not advance the expectation
        assert!(inbound.accept(2).is_ok());
    }

    #[test]
    fn a_replayed_sequence_is_rejected() {
        let mut inbound = Sequencer::new();
        assert!(inbound.accept(0).is_ok());
        assert_eq!(
            inbound.accept(0).unwrap_err(),
            ProtocolError::OutOfOrder {
                expected: 1,
                got: 0
            }
        );
    }

    #[test]
    fn resuming_starts_after_the_last_seen_frame() {
        let mut out = Sequencer::resuming_after(41);
        assert_eq!(out.issue(), 42);
    }

    #[test]
    fn resume_planning_covers_every_case() {
        assert_eq!(
            plan_resume((10, 20), None),
            Resume::Replay {
                from: 10,
                through: 20
            }
        );
        assert_eq!(
            plan_resume((10, 20), Some(14)),
            Resume::Replay {
                from: 15,
                through: 20
            }
        );
        assert_eq!(plan_resume((10, 20), Some(20)), Resume::UpToDate);
        assert_eq!(plan_resume((10, 20), Some(99)), Resume::UpToDate);
        assert_eq!(
            plan_resume((10, 20), Some(3)),
            Resume::WindowLost {
                earliest_retained: 10
            }
        );
        // the boundary: last_seen 9 wants 10, which is still retained
        assert_eq!(
            plan_resume((10, 20), Some(9)),
            Resume::Replay {
                from: 10,
                through: 20
            }
        );
    }
}
