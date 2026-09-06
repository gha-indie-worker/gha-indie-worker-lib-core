#![forbid(unsafe_code)]

//! Length-prefixed JSON framing for the stateful TCP avenue.
//!
//! Wire format, and there is nothing else to it:
//!
//! ```text
//! +----------------+--------------------------+
//! | u32 big-endian | that many bytes of UTF-8 |
//! |   length       |   JSON, no trailing NUL  |
//! +----------------+--------------------------+
//! ```
//!
//! The length is read before any body is buffered, so an oversized declaration
//! costs four bytes and a disconnect rather than a megabyte of allocation. The
//! decoder is a state machine over a growable buffer: feed it whatever a read
//! returned, pull whole frames out until it says `None`.

use super::{ProtocolError, MAX_FRAME_BYTES};

/// The size of the length prefix.
pub const HEADER_BYTES: usize = 4;

/// Frame one payload. The payload is written verbatim; it is the caller's job to
/// hand over serialized JSON.
///
/// # Errors
///
/// [`ProtocolError::EmptyFrame`] for an empty payload, and
/// [`ProtocolError::FrameTooLarge`] beyond [`MAX_FRAME_BYTES`].
pub fn encode(payload: &[u8]) -> Result<Vec<u8>, ProtocolError> {
    encode_with_limit(payload, MAX_FRAME_BYTES)
}

/// Frame one payload against an explicit limit.
///
/// # Errors
///
/// As [`encode`], with `limit` in place of [`MAX_FRAME_BYTES`].
pub fn encode_with_limit(payload: &[u8], limit: usize) -> Result<Vec<u8>, ProtocolError> {
    if payload.is_empty() {
        return Err(ProtocolError::EmptyFrame);
    }
    if payload.len() > limit {
        return Err(ProtocolError::FrameTooLarge {
            declared: payload.len(),
            limit,
        });
    }
    let mut out = Vec::with_capacity(HEADER_BYTES + payload.len());
    let length = u32::try_from(payload.len()).map_err(|_| ProtocolError::FrameTooLarge {
        declared: payload.len(),
        limit,
    })?;
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

/// Incremental decoder. Bytes go in in arbitrary chunks; whole payloads come out.
#[derive(Clone, Debug)]
pub struct Decoder {
    buffer: Vec<u8>,
    limit: usize,
    poisoned: bool,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    #[must_use]
    pub fn new() -> Self {
        Self::with_limit(MAX_FRAME_BYTES)
    }

    #[must_use]
    pub const fn with_limit(limit: usize) -> Self {
        Self {
            buffer: Vec::new(),
            limit,
            poisoned: false,
        }
    }

    /// Bytes currently held back waiting for the rest of a frame.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.buffer.len()
    }

    /// Whether a protocol error has been raised. A poisoned decoder never yields
    /// another frame: the connection must be closed.
    #[must_use]
    pub const fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// Append freshly read bytes.
    pub fn feed(&mut self, chunk: &[u8]) {
        if !self.poisoned {
            self.buffer.extend_from_slice(chunk);
        }
    }

    /// Pull the next complete payload, if one has arrived.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::FrameTooLarge`] or [`ProtocolError::EmptyFrame`] when the
    /// header is unacceptable. Both poison the decoder.
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>, ProtocolError> {
        if self.poisoned || self.buffer.len() < HEADER_BYTES {
            return Ok(None);
        }
        let mut header = [0u8; HEADER_BYTES];
        header.copy_from_slice(&self.buffer[..HEADER_BYTES]);
        let declared = u32::from_be_bytes(header) as usize;
        if declared == 0 {
            self.poisoned = true;
            return Err(ProtocolError::EmptyFrame);
        }
        if declared > self.limit {
            self.poisoned = true;
            return Err(ProtocolError::FrameTooLarge {
                declared,
                limit: self.limit,
            });
        }
        if self.buffer.len() < HEADER_BYTES + declared {
            return Ok(None);
        }
        let payload = self.buffer[HEADER_BYTES..HEADER_BYTES + declared].to_vec();
        self.buffer.drain(..HEADER_BYTES + declared);
        Ok(Some(payload))
    }

    /// Pull the next payload as `&str`, rejecting non-UTF-8.
    ///
    /// # Errors
    ///
    /// As [`Decoder::next_frame`], plus [`ProtocolError::NotUtf8`].
    pub fn next_text(&mut self) -> Result<Option<String>, ProtocolError> {
        match self.next_frame()? {
            None => Ok(None),
            Some(bytes) => match String::from_utf8(bytes) {
                Ok(text) => Ok(Some(text)),
                Err(_) => {
                    self.poisoned = true;
                    Err(ProtocolError::NotUtf8)
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_one_frame() {
        let mut decoder = Decoder::new();
        decoder.feed(&encode(br#"{"kind":"ping"}"#).unwrap());
        assert_eq!(
            decoder.next_text().unwrap().as_deref(),
            Some(r#"{"kind":"ping"}"#)
        );
        assert_eq!(decoder.next_frame().unwrap(), None);
        assert_eq!(decoder.buffered(), 0);
    }

    #[test]
    fn reassembles_across_arbitrary_chunk_boundaries() {
        let wire: Vec<u8> = [b"hello".as_slice(), b"world!!", b"third"]
            .iter()
            .flat_map(|p| encode(p).unwrap())
            .collect();
        for chunk_size in 1..=wire.len() {
            let mut decoder = Decoder::new();
            let mut got = Vec::new();
            for chunk in wire.chunks(chunk_size) {
                decoder.feed(chunk);
                while let Some(frame) = decoder.next_frame().unwrap() {
                    got.push(frame);
                }
            }
            assert_eq!(
                got,
                vec![b"hello".to_vec(), b"world!!".to_vec(), b"third".to_vec()],
                "chunk size {chunk_size}"
            );
            assert_eq!(decoder.buffered(), 0);
        }
    }

    #[test]
    fn a_partial_frame_yields_nothing_and_is_retained() {
        let wire = encode(b"abcdefgh").unwrap();
        let mut decoder = Decoder::new();
        decoder.feed(&wire[..6]);
        assert_eq!(decoder.next_frame().unwrap(), None);
        assert_eq!(decoder.buffered(), 6);
        decoder.feed(&wire[6..]);
        assert_eq!(decoder.next_frame().unwrap(), Some(b"abcdefgh".to_vec()));
    }

    #[test]
    fn oversized_declaration_is_refused_before_the_body_is_buffered() {
        let mut decoder = Decoder::with_limit(16);
        decoder.feed(&1_000_000_u32.to_be_bytes());
        let err = decoder.next_frame().unwrap_err();
        assert_eq!(
            err,
            ProtocolError::FrameTooLarge {
                declared: 1_000_000,
                limit: 16
            }
        );
        assert!(decoder.is_poisoned());
        // A poisoned decoder stays shut even if the peer keeps talking.
        decoder.feed(&encode_with_limit(b"ok", 16).unwrap());
        assert_eq!(decoder.next_frame().unwrap(), None);
    }

    #[test]
    fn zero_length_frames_are_a_protocol_error() {
        assert_eq!(encode(b"").unwrap_err(), ProtocolError::EmptyFrame);
        let mut decoder = Decoder::new();
        decoder.feed(&0u32.to_be_bytes());
        assert_eq!(decoder.next_frame().unwrap_err(), ProtocolError::EmptyFrame);
    }

    #[test]
    fn encoding_refuses_an_oversized_payload() {
        let payload = vec![b'x'; 32];
        assert_eq!(
            encode_with_limit(&payload, 16).unwrap_err(),
            ProtocolError::FrameTooLarge {
                declared: 32,
                limit: 16
            }
        );
    }

    #[test]
    fn non_utf8_payloads_are_rejected_as_text() {
        let mut decoder = Decoder::new();
        decoder.feed(&encode(&[0xff, 0xfe]).unwrap());
        assert_eq!(decoder.next_text().unwrap_err(), ProtocolError::NotUtf8);
        assert!(decoder.is_poisoned());
    }

    #[test]
    fn a_frame_exactly_at_the_limit_is_accepted() {
        let payload = vec![b'x'; 64];
        let mut decoder = Decoder::with_limit(64);
        decoder.feed(&encode_with_limit(&payload, 64).unwrap());
        assert_eq!(decoder.next_frame().unwrap(), Some(payload));
    }
}
