#![forbid(unsafe_code)]

//! Read builders for the `identity` slice.
//!
//! Same rules as [`super::runs`]: parameterized statements only, no writes, no
//! DDL, no connection. Column names track
//! `contracts/json-schema/identity.schema.json`.

use sea_orm::{DatabaseBackend, Statement, Value};

/// `orgs`
pub const ORGS: &str = "orgs";
/// `users`
pub const USERS: &str = "users";
/// `org_members`
pub const MEMBERS: &str = "org_members";
/// `org_invitations`
pub const INVITATIONS: &str = "org_invitations";
/// `org_seats`
pub const SEATS: &str = "org_seats";

/// Every org one user belongs to, with the role they hold there.
#[must_use]
pub fn orgs_for_user(user_id: &str) -> Statement {
    Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        r"
            SELECT o.id::text   AS id,
                   o.slug       AS slug,
                   o.display_name AS display_name,
                   m.role::text AS role
            FROM org_members AS m
            JOIN orgs AS o ON o.id = m.org_id
            WHERE m.user_id = $1::uuid
              AND o.deleted_at IS NULL
            ORDER BY o.slug
        ",
        [Value::from(user_id)],
    )
}

/// Resolve a shared-auth subject to the local user row.
#[must_use]
pub fn user_by_shared_auth_subject(subject: &str) -> Statement {
    Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        r"
            SELECT id::text AS id, email, display_name, github_login, created_at, last_seen_at
            FROM users
            WHERE shared_auth_subject = $1
        ",
        [Value::from(subject)],
    )
}

/// Seat accounting for one org: the limit, and how many seats are in each state.
#[must_use]
pub fn seat_usage(org_id: &str) -> Statement {
    Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        r"
            SELECT o.seat_limit AS seat_limit,
                   COUNT(*) FILTER (WHERE s.status = 'active')    AS active_seats,
                   COUNT(*) FILTER (WHERE s.status = 'suspended') AS suspended_seats,
                   COUNT(*) FILTER (WHERE s.status = 'released')  AS released_seats
            FROM orgs AS o
            LEFT JOIN org_seats AS s ON s.org_id = o.id
            WHERE o.id = $1::uuid
            GROUP BY o.seat_limit
        ",
        [Value::from(org_id)],
    )
}

/// The one live invitation for an email in an org, if there is one.
#[must_use]
pub fn live_invitation(org_id: &str, email: &str) -> Statement {
    Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        r"
            SELECT id::text AS id, role::text AS role, status::text AS status,
                   created_at, expires_at
            FROM org_invitations
            WHERE org_id = $1::uuid
              AND email = $2
              AND status = 'pending'
              AND expires_at > now()
        ",
        [Value::from(org_id), Value::from(email)],
    )
}

/// Members of an org holding any of `roles`, for the team page.
///
/// `roles` is bound as an array parameter; it is never spliced into the SQL.
#[must_use]
pub fn members_with_roles(org_id: &str, roles: &[&str]) -> Statement {
    let joined = roles.join(",");
    Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        r"
            SELECT u.id::text AS id, u.email, u.display_name, m.role::text AS role, m.created_at
            FROM org_members AS m
            JOIN users AS u ON u.id = m.user_id
            WHERE m.org_id = $1::uuid
              AND m.role::text = ANY(string_to_array($2, ','))
            ORDER BY u.email
        ",
        [Value::from(org_id), Value::from(joined)],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statements_are_parameterized() {
        for statement in [
            orgs_for_user("11111111-1111-4111-8111-111111111111"),
            user_by_shared_auth_subject("auth|abc"),
            seat_usage("11111111-1111-4111-8111-111111111111"),
            live_invitation("11111111-1111-4111-8111-111111111111", "a@b.example"),
            members_with_roles("11111111-1111-4111-8111-111111111111", &["owner", "admin"]),
        ] {
            assert!(statement.values.is_some(), "{}", statement.sql);
            assert_eq!(statement.db_backend, DatabaseBackend::Postgres);
        }
    }

    #[test]
    fn role_lists_are_bound_not_spliced() {
        let statement = members_with_roles("org", &["owner", "'; DROP TABLE users; --"]);
        assert!(!statement.sql.contains("DROP TABLE"));
        assert_eq!(statement.values.as_ref().unwrap().0.len(), 2);
    }

    #[test]
    fn soft_deleted_orgs_are_excluded() {
        assert!(orgs_for_user("u").sql.contains("deleted_at IS NULL"));
    }

    #[test]
    fn table_names_match_the_contract() {
        assert!(orgs_for_user("u").sql.contains(ORGS));
        assert!(orgs_for_user("u").sql.contains(MEMBERS));
        assert!(user_by_shared_auth_subject("s").sql.contains(USERS));
        assert!(seat_usage("o").sql.contains(SEATS));
        assert!(live_invitation("o", "e").sql.contains(INVITATIONS));
    }
}
