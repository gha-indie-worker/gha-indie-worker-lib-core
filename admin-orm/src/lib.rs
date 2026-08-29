//! Product-owned, named ORM operations for the isolated admin database.
//!
//! Admin services depend on this surface from their product's `*-lib-core`
//! repository. The contexts keep the `SeaORM` connection private so callers
//! cannot bypass the reviewed operations with ad-hoc SQL.

use sea_orm::{
    ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement, TryGetable,
};
use uuid::Uuid;

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
    pub actor_session_id: Option<&'a str>,
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
    pub async fn connect(database_url: &str) -> Result<Self, AdminOrmError> {
        let connection = Database::connect(database_url)
            .await
            .map_err(AdminOrmError::Database)?;
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
    pub async fn connect(database_url: &str) -> Result<Self, AdminOrmError> {
        let connection = Database::connect(database_url)
            .await
            .map_err(AdminOrmError::Database)?;
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
        let row = self
            .connection
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
                    ON CONFLICT (idempotency_key) DO UPDATE
                    SET idempotency_key = EXCLUDED.idempotency_key
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
            .map_err(AdminOrmError::Database)?
            .ok_or(AdminOrmError::InvalidOperationId)?;
        let value = String::try_get(&row, "", "operation_id").map_err(|_| AdminOrmError::Decode)?;
        Uuid::parse_str(&value).map_err(|_| AdminOrmError::InvalidOperationId)
    }
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
    use super::AdminPermission;

    #[test]
    fn permissions_have_stable_database_values() {
        assert_eq!(AdminPermission::Read.as_str(), "admin:read");
        assert_eq!(AdminPermission::Write.as_str(), "admin:write");
    }
}
