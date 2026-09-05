//! Product-owned, named ORM operations for the isolated admin database.
//!
//! Admin services depend on this surface from their product's `*-lib-core`
//! repository. The contexts keep the `SeaORM` connection private so callers
//! cannot bypass the reviewed operations with ad-hoc SQL.

use std::time::Duration;

use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement,
    TransactionTrait, TryGetable,
};
use url::Url;
use uuid::Uuid;

const MAX_CONNECTIONS: u32 = 8;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(3);
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Secret-bearing connection input plus the reviewed, non-secret RDS identity.
///
/// This type intentionally does not implement `Debug`: a database URL must
/// never be emitted by logs, traces, panic reports, or test snapshots.
#[derive(Clone, Copy)]
pub struct AdminDatabaseConfig<'a> {
    pub database_url: &'a str,
    pub expected_host: &'a str,
    pub expected_database: &'a str,
    pub expected_role: &'a str,
}

impl AdminDatabaseConfig<'_> {
    fn connect_options(&self) -> Result<ConnectOptions, AdminOrmError> {
        self.validate()?;
        let mut options = ConnectOptions::new(self.database_url.to_owned());
        options
            .max_connections(MAX_CONNECTIONS)
            .min_connections(1)
            .connect_timeout(CONNECT_TIMEOUT)
            .acquire_timeout(ACQUIRE_TIMEOUT)
            .idle_timeout(IDLE_TIMEOUT)
            .sqlx_logging(false);
        Ok(options)
    }

    fn validate(&self) -> Result<(), AdminOrmError> {
        let url =
            Url::parse(self.database_url).map_err(|_| AdminOrmError::InvalidDatabaseTarget)?;
        if !matches!(url.scheme(), "postgres" | "postgresql")
            || url.host_str() != Some(self.expected_host)
            || url.username() != self.expected_role
            || url.fragment().is_some()
            || !valid_identifier(self.expected_database)
            || !valid_identifier(self.expected_role)
        {
            return Err(AdminOrmError::InvalidDatabaseTarget);
        }
        let database = url
            .path()
            .strip_prefix('/')
            .filter(|value| !value.is_empty() && !value.contains('/'))
            .ok_or(AdminOrmError::InvalidDatabaseTarget)?;
        if database != self.expected_database {
            return Err(AdminOrmError::InvalidDatabaseTarget);
        }
        let ssl_modes = url
            .query_pairs()
            .filter(|(name, _)| name == "sslmode")
            .map(|(_, value)| value)
            .collect::<Vec<_>>();
        if ssl_modes.len() != 1 || ssl_modes[0] != "verify-full" {
            return Err(AdminOrmError::InvalidDatabaseTarget);
        }
        Ok(())
    }
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdminPermission {
    Read,
    Write,
}

impl AdminPermission {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "admin:read",
            Self::Write => "admin:write",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AdminDashboardStats {
    pub active_admins: i64,
    pub pending_actions: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdminAction<'a> {
    pub idempotency_key: &'a str,
    pub actor_subject: &'a str,
    pub actor_session_id: &'a str,
    pub resource: &'a str,
    pub action: &'a str,
    pub reason: &'a str,
}

#[derive(Debug, thiserror::Error)]
pub enum AdminOrmError {
    #[error("admin database operation failed")]
    Database(#[source] sea_orm::DbErr),
    #[error("admin database returned an unexpected value")]
    Decode,
    #[error("admin database returned an invalid operation identifier")]
    InvalidOperationId,
    #[error("admin database target does not match the reviewed RDS identity")]
    InvalidDatabaseTarget,
    #[error("admin database runtime role has unsafe privileges")]
    UnsafeRuntimeRole,
    #[error("idempotency key was already used for a different admin action")]
    IdempotencyConflict,
    #[error("admin web credential is not database-enforced read-only")]
    ReadCredentialWritable,
    #[error("admin API credential cannot write to the admin database")]
    WriteCredentialReadOnly,
}

#[derive(Clone)]
pub struct AdminReadContext {
    connection: DatabaseConnection,
}

impl AdminReadContext {
    /// Connects and proves that `PostgreSQL` enforces a read-only session.
    ///
    /// # Errors
    ///
    /// Returns an error when the connection fails, the mode cannot be decoded,
    /// or the supplied credential can write.
    pub async fn connect(config: AdminDatabaseConfig<'_>) -> Result<Self, AdminOrmError> {
        let connection = Database::connect(config.connect_options()?)
            .await
            .map_err(AdminOrmError::Database)?;
        validate_runtime_role(&connection, &config).await?;
        if transaction_read_only(&connection).await? {
            Ok(Self { connection })
        } else {
            Err(AdminOrmError::ReadCredentialWritable)
        }
    }

    /// Checks that the admin database is reachable.
    ///
    /// # Errors
    ///
    /// Returns an error when the database cannot execute the probe.
    pub async fn ready(&self) -> Result<(), AdminOrmError> {
        ready(&self.connection).await
    }

    /// Loads the product-owned administrative grant.
    ///
    /// # Errors
    ///
    /// Returns an error when the query fails or the result cannot be decoded.
    pub async fn has_permission(
        &self,
        subject: &str,
        permission: AdminPermission,
    ) -> Result<bool, AdminOrmError> {
        has_permission(&self.connection, subject, permission).await
    }

    /// Returns bounded aggregate data for the admin dashboard.
    ///
    /// # Errors
    ///
    /// Returns an error when the query fails or the result cannot be decoded.
    pub async fn dashboard_stats(&self) -> Result<AdminDashboardStats, AdminOrmError> {
        let row = self
            .connection
            .query_one(Statement::from_string(
                DatabaseBackend::Postgres,
                r"
                    SELECT
                      (SELECT count(*) FROM admin_principals WHERE status = 'active')::bigint
                        AS active_admins,
                      (SELECT count(*) FROM admin_action_requests
                       WHERE status IN ('accepted', 'running'))::bigint AS pending_actions
                "
                .to_owned(),
            ))
            .await
            .map_err(AdminOrmError::Database)?
            .ok_or(AdminOrmError::Decode)?;
        Ok(AdminDashboardStats {
            active_admins: i64::try_get(&row, "", "active_admins")
                .map_err(|_| AdminOrmError::Decode)?,
            pending_actions: i64::try_get(&row, "", "pending_actions")
                .map_err(|_| AdminOrmError::Decode)?,
        })
    }
}

#[derive(Clone)]
pub struct AdminWriteContext {
    connection: DatabaseConnection,
}

impl AdminWriteContext {
    /// Connects and proves that the admin API credential is write-capable.
    ///
    /// # Errors
    ///
    /// Returns an error when the connection fails, the mode cannot be decoded,
    /// or the supplied credential is read-only.
    pub async fn connect(config: AdminDatabaseConfig<'_>) -> Result<Self, AdminOrmError> {
        let connection = Database::connect(config.connect_options()?)
            .await
            .map_err(AdminOrmError::Database)?;
        validate_runtime_role(&connection, &config).await?;
        if transaction_read_only(&connection).await? {
            Err(AdminOrmError::WriteCredentialReadOnly)
        } else {
            Ok(Self { connection })
        }
    }

    /// Checks that the admin database is reachable.
    ///
    /// # Errors
    ///
    /// Returns an error when the database cannot execute the probe.
    pub async fn ready(&self) -> Result<(), AdminOrmError> {
        ready(&self.connection).await
    }

    /// Loads the product-owned administrative grant.
    ///
    /// # Errors
    ///
    /// Returns an error when the query fails or the result cannot be decoded.
    pub async fn has_permission(
        &self,
        subject: &str,
        permission: AdminPermission,
    ) -> Result<bool, AdminOrmError> {
        has_permission(&self.connection, subject, permission).await
    }

    /// Records an idempotent administrative command before execution.
    ///
    /// # Errors
    ///
    /// Returns an error when the write fails or the operation id is invalid.
    pub async fn record_action(&self, input: &AdminAction<'_>) -> Result<Uuid, AdminOrmError> {
        let operation_id = Uuid::new_v4();
        let transaction = self
            .connection
            .begin()
            .await
            .map_err(AdminOrmError::Database)?;
        let inserted = transaction
            .query_one(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                r"
                    INSERT INTO admin_action_requests (
                        operation_id,
                        idempotency_key,
                        actor_subject,
                        actor_session_id,
                        resource,
                        action,
                        reason
                    ) VALUES ($1::uuid, $2, $3, $4, $5, $6, $7)
                    ON CONFLICT (idempotency_key) DO NOTHING
                    RETURNING operation_id::text AS operation_id
                ",
                [
                    operation_id.to_string().into(),
                    input.idempotency_key.into(),
                    input.actor_subject.into(),
                    input.actor_session_id.into(),
                    input.resource.into(),
                    input.action.into(),
                    input.reason.into(),
                ],
            ))
            .await
            .map_err(AdminOrmError::Database)?;
        let (operation_id, was_inserted) = if let Some(row) = inserted {
            (operation_id_from_row(&row)?, true)
        } else {
            let existing = transaction
                .query_one(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    r"
                            SELECT operation_id::text AS operation_id
                            FROM admin_action_requests
                            WHERE idempotency_key = $1
                              AND actor_subject = $2
                              AND actor_session_id = $3
                              AND resource = $4
                              AND action = $5
                              AND reason = $6
                        ",
                    [
                        input.idempotency_key.into(),
                        input.actor_subject.into(),
                        input.actor_session_id.into(),
                        input.resource.into(),
                        input.action.into(),
                        input.reason.into(),
                    ],
                ))
                .await
                .map_err(AdminOrmError::Database)?;
            if let Some(row) = existing {
                (operation_id_from_row(&row)?, false)
            } else {
                transaction
                    .rollback()
                    .await
                    .map_err(AdminOrmError::Database)?;
                return Err(AdminOrmError::IdempotencyConflict);
            }
        };
        if was_inserted {
            transaction
                .execute(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    r"
                        INSERT INTO admin_action_outbox (operation_id, event_kind)
                        VALUES ($1::uuid, 'admin.action.requested')
                    ",
                    [operation_id.to_string().into()],
                ))
                .await
                .map_err(AdminOrmError::Database)?;
        }
        transaction
            .commit()
            .await
            .map_err(AdminOrmError::Database)?;
        Ok(operation_id)
    }
}

fn operation_id_from_row(row: &sea_orm::QueryResult) -> Result<Uuid, AdminOrmError> {
    let value = String::try_get(row, "", "operation_id").map_err(|_| AdminOrmError::Decode)?;
    Uuid::parse_str(&value).map_err(|_| AdminOrmError::InvalidOperationId)
}

async fn ready(connection: &DatabaseConnection) -> Result<(), AdminOrmError> {
    connection
        .query_one(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT 1 AS ready".to_owned(),
        ))
        .await
        .map_err(AdminOrmError::Database)?;
    Ok(())
}

async fn validate_runtime_role(
    connection: &DatabaseConnection,
    config: &AdminDatabaseConfig<'_>,
) -> Result<(), AdminOrmError> {
    let row = connection
        .query_one(Statement::from_string(
            DatabaseBackend::Postgres,
            r"
                SELECT
                    current_user::text AS role_name,
                    current_database()::text AS database_name,
                    role.rolsuper,
                    role.rolcreaterole,
                    role.rolcreatedb,
                    role.rolreplication,
                    role.rolbypassrls,
                    has_database_privilege(current_user, current_database(), 'CREATE')
                        AS can_create_schemas,
                    EXISTS (
                        SELECT 1
                        FROM information_schema.schemata AS schema
                        WHERE schema.schema_name <> 'information_schema'
                          AND schema.schema_name NOT LIKE 'pg_%'
                          AND has_schema_privilege(current_user, schema.schema_name, 'CREATE')
                    ) AS can_create_schema_objects
                FROM pg_roles AS role
                WHERE role.rolname = current_user
            "
            .to_owned(),
        ))
        .await
        .map_err(AdminOrmError::Database)?
        .ok_or(AdminOrmError::UnsafeRuntimeRole)?;
    let role_name = String::try_get(&row, "", "role_name").map_err(|_| AdminOrmError::Decode)?;
    let database_name =
        String::try_get(&row, "", "database_name").map_err(|_| AdminOrmError::Decode)?;
    let unsafe_role = [
        "rolsuper",
        "rolcreaterole",
        "rolcreatedb",
        "rolreplication",
        "rolbypassrls",
        "can_create_schemas",
        "can_create_schema_objects",
    ]
    .into_iter()
    .try_fold(false, |unsafe_role, column| {
        bool::try_get(&row, "", column)
            .map(|value| unsafe_role || value)
            .map_err(|_| AdminOrmError::Decode)
    })?;
    if role_name != config.expected_role || database_name != config.expected_database || unsafe_role
    {
        return Err(AdminOrmError::UnsafeRuntimeRole);
    }
    Ok(())
}

async fn transaction_read_only(connection: &DatabaseConnection) -> Result<bool, AdminOrmError> {
    let row = connection
        .query_one(Statement::from_string(
            DatabaseBackend::Postgres,
            "SHOW transaction_read_only".to_owned(),
        ))
        .await
        .map_err(AdminOrmError::Database)?
        .ok_or(AdminOrmError::Decode)?;
    let mode =
        String::try_get(&row, "", "transaction_read_only").map_err(|_| AdminOrmError::Decode)?;
    match mode.as_str() {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(AdminOrmError::Decode),
    }
}

async fn has_permission(
    connection: &DatabaseConnection,
    subject: &str,
    permission: AdminPermission,
) -> Result<bool, AdminOrmError> {
    let row = connection
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r"
                SELECT EXISTS (
                    SELECT 1
                    FROM admin_principals
                    WHERE shared_auth_subject = $1
                      AND status = 'active'
                      AND ($2 = ANY(permissions) OR 'super_admin' = ANY(permissions))
                ) AS allowed
            ",
            [subject.into(), permission.as_str().into()],
        ))
        .await
        .map_err(AdminOrmError::Database)?;
    row.map(|result| bool::try_get(&result, "", "allowed"))
        .transpose()
        .map_err(|_| AdminOrmError::Decode)
        .map(|allowed| allowed.unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::{AdminDatabaseConfig, AdminPermission};

    #[test]
    fn permissions_have_stable_database_values() {
        assert_eq!(AdminPermission::Read.as_str(), "admin:read");
        assert_eq!(AdminPermission::Write.as_str(), "admin:write");
    }

    #[test]
    fn database_target_requires_exact_identity_and_verified_tls() {
        let valid = AdminDatabaseConfig {
            database_url: "postgresql://admin_api_runtime:secret@admin-db.example/admin_control?sslmode=verify-full",
            expected_host: "admin-db.example",
            expected_database: "admin_control",
            expected_role: "admin_api_runtime",
        };
        assert!(valid.validate().is_ok());
        assert!(
            AdminDatabaseConfig {
                database_url: "postgresql://admin_api_runtime:secret@customer-db.example/admin_control?sslmode=verify-full",
                ..valid
            }
            .validate()
            .is_err()
        );
        assert!(
            AdminDatabaseConfig {
                database_url:
                    "postgresql://admin_api_runtime:secret@admin-db.example/admin_control?sslmode=require",
                ..valid
            }
            .validate()
            .is_err()
        );
    }
}
