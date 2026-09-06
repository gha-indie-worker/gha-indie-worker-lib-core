#![forbid(unsafe_code)]

use crate::config::CoreConfig;
use crate::error::CoreError;
use crate::schema::SCHEMA_REVISION;

#[derive(Clone, Debug)]
pub struct CorePool {
    pub flavor: crate::flavor::DatabaseFlavor,
    read_only: bool,
}

impl CorePool {
    pub fn connect(config: &CoreConfig) -> Result<Self, CoreError> {
        Ok(Self {
            flavor: config.flavor,
            read_only: config.read_only,
        })
    }

    pub fn assert_schema(&self, found: &str) -> Result<(), CoreError> {
        if found != SCHEMA_REVISION {
            return Err(CoreError::SchemaRevision {
                required: SCHEMA_REVISION.to_string(),
                found: found.to_string(),
            });
        }
        Ok(())
    }

    pub fn migrate(&self) -> Result<(), CoreError> {
        if self.read_only {
            return Err(CoreError::WritesDisabled);
        }
        Ok(())
    }
}
