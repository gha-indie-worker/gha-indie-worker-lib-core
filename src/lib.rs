#![forbid(unsafe_code)]

pub mod config;
pub mod connection;
pub mod error;
pub mod flavor;
pub mod schema;

pub use config::CoreConfig;
pub use connection::CorePool;
pub use error::CoreError;
pub use flavor::DatabaseFlavor;
pub use schema::SCHEMA_REVISION;

