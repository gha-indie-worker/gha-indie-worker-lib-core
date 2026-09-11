#![forbid(unsafe_code)]

use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LogPosition {
    pub sequence: u64,
    pub offset_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogAdmission {
    Accepted {
        next_sequence: u64,
        next_offset_bytes: u64,
    },
    Duplicate {
        next_sequence: u64,
        next_offset_bytes: u64,
    },
    Gap {
        expected_sequence: u64,
        received_sequence: u64,
    },
    OffsetMismatch {
        expected_offset_bytes: u64,
        received_offset_bytes: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LogCursor {
    next_sequence: u64,
    next_offset_bytes: u64,
}

impl Default for LogCursor {
    fn default() -> Self {
        Self::new()
    }
}

impl LogCursor {
    pub const fn new() -> Self {
        Self {
            next_sequence: 0,
            next_offset_bytes: 0,
        }
    }

    pub const fn from_expected(next_sequence: u64, next_offset_bytes: u64) -> Self {
        Self {
            next_sequence,
            next_offset_bytes,
        }
    }

    pub const fn next_sequence(self) -> u64 {
        self.next_sequence
    }

    pub const fn next_offset_bytes(self) -> u64 {
        self.next_offset_bytes
    }

    /// Pure admission for one already-validated record in one logical stream.
    ///
    /// Callers keep a separate cursor for each
    /// `(org, repo, run, attempt, job, stream)` identity. A duplicate decision
    /// only means the sequence is already behind the cursor; payload/digest
    /// equality remains the idempotency layer's responsibility.
    pub fn observe(
        self,
        position: LogPosition,
        body_bytes: u64,
    ) -> Result<(Self, LogAdmission), LogCursorError> {
        if position.sequence < self.next_sequence {
            return Ok((
                self,
                LogAdmission::Duplicate {
                    next_sequence: self.next_sequence,
                    next_offset_bytes: self.next_offset_bytes,
                },
            ));
        }

        if position.sequence > self.next_sequence {
            return Ok((
                self,
                LogAdmission::Gap {
                    expected_sequence: self.next_sequence,
                    received_sequence: position.sequence,
                },
            ));
        }

        if position.offset_bytes != self.next_offset_bytes {
            return Ok((
                self,
                LogAdmission::OffsetMismatch {
                    expected_offset_bytes: self.next_offset_bytes,
                    received_offset_bytes: position.offset_bytes,
                },
            ));
        }

        let next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(LogCursorError::SequenceOverflow)?;
        let next_offset_bytes = self
            .next_offset_bytes
            .checked_add(body_bytes)
            .ok_or(LogCursorError::OffsetOverflow)?;
        let next = Self {
            next_sequence,
            next_offset_bytes,
        };

        Ok((
            next,
            LogAdmission::Accepted {
                next_sequence,
                next_offset_bytes,
            },
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum LogCursorError {
    #[error("build-log sequence overflow")]
    SequenceOverflow,
    #[error("build-log byte offset overflow")]
    OffsetOverflow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferPolicy {
    pub max_records: usize,
    pub max_buffer_bytes: u64,
    pub max_record_bytes: u64,
}

impl BufferPolicy {
    pub fn new(
        max_records: usize,
        max_buffer_bytes: u64,
        max_record_bytes: u64,
    ) -> Result<Self, BufferPolicyError> {
        if max_records == 0 || max_buffer_bytes == 0 || max_record_bytes == 0 {
            return Err(BufferPolicyError::ZeroLimit);
        }
        if max_record_bytes > max_buffer_bytes {
            return Err(BufferPolicyError::RecordLimitExceedsBuffer);
        }
        Ok(Self {
            max_records,
            max_buffer_bytes,
            max_record_bytes,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferAction {
    Enqueue,
    FlushThenEnqueue,
    RejectOversize,
}

pub fn plan_buffer_action(
    policy: BufferPolicy,
    buffered_records: usize,
    buffered_bytes: u64,
    record_bytes: u64,
) -> Result<BufferAction, BufferPolicyError> {
    if record_bytes > policy.max_record_bytes || record_bytes > policy.max_buffer_bytes {
        return Ok(BufferAction::RejectOversize);
    }

    let next_records = buffered_records
        .checked_add(1)
        .ok_or(BufferPolicyError::CounterOverflow)?;
    let next_bytes = buffered_bytes
        .checked_add(record_bytes)
        .ok_or(BufferPolicyError::CounterOverflow)?;

    if next_records > policy.max_records || next_bytes > policy.max_buffer_bytes {
        return Ok(BufferAction::FlushThenEnqueue);
    }

    Ok(BufferAction::Enqueue)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum BufferPolicyError {
    #[error("build-log buffer limits must all be non-zero")]
    ZeroLimit,
    #[error("build-log per-record byte limit cannot exceed the whole-buffer byte limit")]
    RecordLimitExceedsBuffer,
    #[error("build-log buffer counter overflow")]
    CounterOverflow,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contiguous_records_advance_sequence_and_byte_offset() {
        let cursor = LogCursor::new();
        let (cursor, decision) = cursor
            .observe(
                LogPosition {
                    sequence: 0,
                    offset_bytes: 0,
                },
                5,
            )
            .unwrap();
        assert_eq!(
            decision,
            LogAdmission::Accepted {
                next_sequence: 1,
                next_offset_bytes: 5,
            }
        );

        let (cursor, decision) = cursor
            .observe(
                LogPosition {
                    sequence: 1,
                    offset_bytes: 5,
                },
                7,
            )
            .unwrap();
        assert_eq!(cursor.next_sequence(), 2);
        assert_eq!(cursor.next_offset_bytes(), 12);
        assert_eq!(
            decision,
            LogAdmission::Accepted {
                next_sequence: 2,
                next_offset_bytes: 12,
            }
        );
    }

    #[test]
    fn gap_does_not_advance_and_missing_record_can_arrive_later() {
        let cursor = LogCursor::from_expected(3, 100);
        let (unchanged, decision) = cursor
            .observe(
                LogPosition {
                    sequence: 5,
                    offset_bytes: 150,
                },
                10,
            )
            .unwrap();
        assert_eq!(unchanged, cursor);
        assert_eq!(
            decision,
            LogAdmission::Gap {
                expected_sequence: 3,
                received_sequence: 5,
            }
        );

        let (advanced, decision) = unchanged
            .observe(
                LogPosition {
                    sequence: 3,
                    offset_bytes: 100,
                },
                20,
            )
            .unwrap();
        assert_eq!(advanced.next_sequence(), 4);
        assert_eq!(advanced.next_offset_bytes(), 120);
        assert!(matches!(decision, LogAdmission::Accepted { .. }));
    }

    #[test]
    fn duplicate_does_not_rewrite_cursor_history() {
        let cursor = LogCursor::from_expected(7, 900);
        let (unchanged, decision) = cursor
            .observe(
                LogPosition {
                    sequence: 6,
                    offset_bytes: 800,
                },
                50,
            )
            .unwrap();
        assert_eq!(unchanged, cursor);
        assert_eq!(
            decision,
            LogAdmission::Duplicate {
                next_sequence: 7,
                next_offset_bytes: 900,
            }
        );
    }

    #[test]
    fn offset_mismatch_fails_closed_without_advancing() {
        let cursor = LogCursor::from_expected(2, 64);
        for received in [63, 65, 1000] {
            let (unchanged, decision) = cursor
                .observe(
                    LogPosition {
                        sequence: 2,
                        offset_bytes: received,
                    },
                    8,
                )
                .unwrap();
            assert_eq!(unchanged, cursor);
            assert_eq!(
                decision,
                LogAdmission::OffsetMismatch {
                    expected_offset_bytes: 64,
                    received_offset_bytes: received,
                }
            );
        }
    }

    #[test]
    fn exhaustive_small_state_only_exact_position_mutates_cursor() {
        for expected_sequence in 0..=4 {
            for expected_offset in 0..=4 {
                let cursor = LogCursor::from_expected(expected_sequence, expected_offset);
                for received_sequence in 0..=4 {
                    for received_offset in 0..=4 {
                        let (next, decision) = cursor
                            .observe(
                                LogPosition {
                                    sequence: received_sequence,
                                    offset_bytes: received_offset,
                                },
                                1,
                            )
                            .unwrap();
                        let exact = received_sequence == expected_sequence
                            && received_offset == expected_offset;
                        assert_eq!(next != cursor, exact);
                        assert_eq!(matches!(decision, LogAdmission::Accepted { .. }), exact);
                    }
                }
            }
        }
    }

    #[test]
    fn sequence_and_offset_overflow_are_explicit_errors() {
        let sequence = LogCursor::from_expected(u64::MAX, 0)
            .observe(
                LogPosition {
                    sequence: u64::MAX,
                    offset_bytes: 0,
                },
                1,
            )
            .unwrap_err();
        assert_eq!(sequence, LogCursorError::SequenceOverflow);

        let offset = LogCursor::from_expected(2, u64::MAX)
            .observe(
                LogPosition {
                    sequence: 2,
                    offset_bytes: u64::MAX,
                },
                1,
            )
            .unwrap_err();
        assert_eq!(offset, LogCursorError::OffsetOverflow);
    }

    #[test]
    fn buffer_policy_rejects_invalid_limits() {
        assert_eq!(
            BufferPolicy::new(0, 1024, 512),
            Err(BufferPolicyError::ZeroLimit)
        );
        assert_eq!(
            BufferPolicy::new(10, 100, 101),
            Err(BufferPolicyError::RecordLimitExceedsBuffer)
        );
    }

    #[test]
    fn buffer_plan_enqueues_flushes_and_rejects_without_side_effects() {
        let policy = BufferPolicy::new(3, 100, 60).unwrap();
        assert_eq!(
            plan_buffer_action(policy, 1, 20, 30).unwrap(),
            BufferAction::Enqueue
        );
        assert_eq!(
            plan_buffer_action(policy, 3, 70, 20).unwrap(),
            BufferAction::FlushThenEnqueue
        );
        assert_eq!(
            plan_buffer_action(policy, 2, 90, 20).unwrap(),
            BufferAction::FlushThenEnqueue
        );
        assert_eq!(
            plan_buffer_action(policy, 0, 0, 61).unwrap(),
            BufferAction::RejectOversize
        );
    }

    #[test]
    fn buffer_counter_overflow_is_not_treated_as_available_capacity() {
        let policy = BufferPolicy::new(usize::MAX, u64::MAX, u64::MAX).unwrap();
        assert_eq!(
            plan_buffer_action(policy, usize::MAX, 0, 1),
            Err(BufferPolicyError::CounterOverflow)
        );
        assert_eq!(
            plan_buffer_action(policy, 0, u64::MAX, 1),
            Err(BufferPolicyError::CounterOverflow)
        );
    }
}
