#![forbid(unsafe_code)]

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("database URL must use postgres or postgresql")]
    InvalidDatabaseUrl,
    #[error("schema revision mismatch: required {required}, found {found}")]
    SchemaRevision { required: String, found: String },
    #[error("read-only connections cannot migrate")]
    WritesDisabled,
}
