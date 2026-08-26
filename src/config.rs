#![forbid(unsafe_code)]

use crate::error::CoreError;
use crate::flavor::DatabaseFlavor;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreConfig {
    pub database_url: String,
    pub flavor: DatabaseFlavor,
    pub read_only: bool,
}

impl CoreConfig {
    pub fn from_env() -> Result<Self, CoreError> {
        let database_url = std::env::var("GHA_INDIE_WORKER_DATABASE_URL").map_err(|_| CoreError::InvalidDatabaseUrl)?;
        if !(database_url.starts_with("postgres://") || database_url.starts_with("postgresql://")) {
            return Err(CoreError::InvalidDatabaseUrl);
        }
        let flavor = if database_url.contains("cockroach") {
            DatabaseFlavor::CockroachDb
        } else {
            DatabaseFlavor::PostgreSql
        };
        Ok(Self {
            database_url,
            flavor,
            read_only: std::env::var("GHA_INDIE_WORKER_DB_READ_ONLY").ok().as_deref() != Some("0"),
        })
    }
}

