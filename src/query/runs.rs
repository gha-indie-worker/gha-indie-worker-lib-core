#![forbid(unsafe_code)]

//! Read builders for the `runs` slice.
//!
//! Every function returns a parameterized [`Statement`]. Nothing here executes,
//! and nothing here writes: writes go through `gha-indie-worker-orm-core`'s
//! reviewed operations so they can be audited in one place.
//!
//! Table and column names come from
//! `gha-indie-worker-interfaces/contracts/json-schema/runs.schema.json`
//! (`x-ores-table`, snake_cased columns), which is also what the generated DDL
//! uses — so a rename in an authority breaks these queries loudly at test time
//! rather than quietly at runtime.

use sea_orm::{DatabaseBackend, Statement, Value};

/// `run_plans`
pub const PLANS: &str = "run_plans";
/// `runs`
pub const RUNS: &str = "runs";
/// `run_jobs`
pub const JOBS: &str = "run_jobs";
/// `run_log_chunks`
pub const LOG_CHUNKS: &str = "run_log_chunks";
/// `run_cancellations`
pub const CANCELLATIONS: &str = "run_cancellations";

/// The most recent runs for one org, newest first.
#[must_use]
pub fn recent_runs_for_org(org_id: &str, limit: u64) -> Statement {
    Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        r"
            SELECT r.id::text        AS id,
                   r.status::text    AS status,
                   r.attempt         AS attempt,
                   r.queued_at       AS queued_at,
                   r.started_at      AS started_at,
                   r.finished_at     AS finished_at,
                   p.repository      AS repository,
                   p.revision        AS revision,
                   p.workflow_path   AS workflow_path
            FROM runs AS r
            JOIN run_plans AS p ON p.id = r.plan_id
            WHERE r.org_id = $1::uuid
            ORDER BY r.queued_at DESC
            LIMIT $2
        ",
        [
            Value::from(org_id),
            Value::from(i64::try_from(limit).unwrap_or(i64::MAX)),
        ],
    )
}

/// The dense log window a websocket resume needs: everything a job has emitted
/// after `after_sequence`, capped.
#[must_use]
pub fn log_chunks_after(job_id: &str, after_sequence: i64, limit: u64) -> Statement {
    Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        r"
            SELECT sequence, offset_bytes, body, truncated, emitted_at
            FROM run_log_chunks
            WHERE job_id = $1::uuid
              AND sequence > $2
            ORDER BY sequence ASC
            LIMIT $3
        ",
        [
            Value::from(job_id),
            Value::from(after_sequence),
            Value::from(i64::try_from(limit).unwrap_or(i64::MAX)),
        ],
    )
}

/// The retained sequence window for a job, which is what
/// `protocol::ws::plan_resume` needs to answer a reconnect.
#[must_use]
pub fn log_chunk_window(job_id: &str) -> Statement {
    Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        r"
            SELECT COALESCE(MIN(sequence), 0) AS earliest,
                   COALESCE(MAX(sequence), 0) AS latest,
                   COUNT(*)                   AS retained
            FROM run_log_chunks
            WHERE job_id = $1::uuid
        ",
        [Value::from(job_id)],
    )
}

/// Job status counts for one run, for the summary header.
#[must_use]
pub fn job_status_counts(run_id: &str) -> Statement {
    Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        r"
            SELECT status::text AS status, COUNT(*) AS total
            FROM run_jobs
            WHERE run_id = $1::uuid
            GROUP BY status
            ORDER BY status
        ",
        [Value::from(run_id)],
    )
}

/// Whether a run has an outstanding cancellation the workers have not yet
/// acknowledged. This is the read the dispatcher does before it hands out work.
#[must_use]
pub fn pending_cancellation(run_id: &str) -> Statement {
    Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        r"
            SELECT reason::text AS reason, requested_at
            FROM run_cancellations
            WHERE run_id = $1::uuid
              AND effective_at IS NULL
        ",
        [Value::from(run_id)],
    )
}

/// Plans for an exact `repository` at an exact 40-hex `revision`. The revision is
/// bound, never interpolated, so a branch name can never reach the database as
/// SQL.
#[must_use]
pub fn plans_at_revision(repository: &str, revision: &str) -> Statement {
    Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        r"
            SELECT id::text AS id, workflow_path, parity_lane::text AS parity_lane,
                   job_count, supported, profile, created_at
            FROM run_plans
            WHERE repository = $1
              AND revision = $2
            ORDER BY workflow_path
        ",
        [Value::from(repository), Value::from(revision)],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placeholders(sql: &str) -> usize {
        let mut seen = std::collections::BTreeSet::new();
        let bytes: Vec<char> = sql.chars().collect();
        for (i, c) in bytes.iter().enumerate() {
            if *c == '$' {
                let digits: String = bytes[i + 1..]
                    .iter()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                if !digits.is_empty() {
                    seen.insert(digits);
                }
            }
        }
        seen.len()
    }

    fn check(statement: &Statement) {
        let values = statement.values.as_ref().expect("parameterized").0.len();
        assert_eq!(
            placeholders(&statement.sql),
            values,
            "placeholder/parameter mismatch in: {}",
            statement.sql
        );
        assert_eq!(statement.db_backend, DatabaseBackend::Postgres);
    }

    #[test]
    fn every_builder_is_parameterized_and_balanced() {
        check(&recent_runs_for_org(
            "11111111-1111-4111-8111-111111111111",
            20,
        ));
        check(&log_chunks_after(
            "22222222-2222-4222-8222-222222222222",
            41,
            200,
        ));
        check(&log_chunk_window("22222222-2222-4222-8222-222222222222"));
        check(&job_status_counts("33333333-3333-4333-8333-333333333333"));
        check(&pending_cancellation(
            "33333333-3333-4333-8333-333333333333",
        ));
        check(&plans_at_revision("owner/repo", &"9f".repeat(20)));
    }

    #[test]
    fn no_builder_interpolates_a_caller_value() {
        let hostile = "'; DROP TABLE runs; --";
        let statement = plans_at_revision(hostile, hostile);
        assert!(!statement.sql.contains("DROP TABLE"));
        assert_eq!(statement.values.as_ref().unwrap().0.len(), 2);
    }

    #[test]
    fn table_names_match_the_contract() {
        assert!(recent_runs_for_org("x", 1).sql.contains(RUNS));
        assert!(recent_runs_for_org("x", 1).sql.contains(PLANS));
        assert!(log_chunks_after("x", 0, 1).sql.contains(LOG_CHUNKS));
        assert!(job_status_counts("x").sql.contains(JOBS));
        assert!(pending_cancellation("x").sql.contains(CANCELLATIONS));
    }
}
