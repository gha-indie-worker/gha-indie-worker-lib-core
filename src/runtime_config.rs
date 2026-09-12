#![forbid(unsafe_code)]

//! The runtime configuration every GHA Indie Worker server shares.
//!
//! One struct, one env prefix, one place to look. Servers layer their own
//! host-specific settings on top; everything here is common to api-server,
//! web-server, admin-api-server, admin-web-server, mcp-server and the CLI.
//!
//! # Where the names come from
//!
//! Every field maps to exactly one `GHA_INDIE_WORKER_*` environment variable,
//! which is the flags-2-env convention: a flag `--tcp-bind` declared in
//! `.cli-flags.toml` becomes `GHA_INDIE_WORKER_TCP_BIND`. The mapping is
//! mechanical — upper-snake of the flag, prefixed — so [`ENV_VARS`] is the list a
//! `.cli-flags.toml` must agree with, and `env_names_are_unique_and_prefixed`
//! asserts the two never drift.
//!
//! Secrets arrive the same way, decrypted from `env/enc` to `env/dec` by
//! ores-sops before the process starts. **They are never logged**: every secret
//! field is a [`Secret`], whose `Debug` prints `<redacted>` and which has no
//! `Display` at all. The struct's own `Debug` is derived from those, so
//! `tracing::debug!(?config)` is safe by construction rather than by discipline.
//!
//! Nothing here reads a file, opens a socket or runs DDL. `from_env` is the only
//! effect, and it is a pure function of a `&dyn Fn(&str) -> Option<String>` so
//! tests never touch the process environment.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

/// A value that must never reach a log, a trace, a panic message or a snapshot.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct Secret(String);

impl Secret {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The only way to read it. Named so it is obvious in review.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.is_empty() {
            "Secret(<unset>)"
        } else {
            "Secret(<redacted>)"
        })
    }
}

/// A misconfiguration. Servers exit on these before binding a listener.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigError {
    Missing(&'static str),
    Invalid { name: &'static str, reason: String },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Missing(name) => write!(f, "{name} is required"),
            ConfigError::Invalid { name, reason } => write!(f, "{name} is invalid: {reason}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Which deployment this process is.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum Environment {
    Dev,
    #[default]
    Staging,
    Prod,
}

impl Environment {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Environment::Dev => "dev",
            Environment::Staging => "staging",
            Environment::Prod => "prod",
        }
    }

    fn parse(value: &str) -> Result<Self, ConfigError> {
        match value {
            "dev" | "development" => Ok(Environment::Dev),
            "staging" | "stage" => Ok(Environment::Staging),
            "prod" | "production" => Ok(Environment::Prod),
            other => Err(ConfigError::Invalid {
                name: "GHA_INDIE_WORKER_ENV",
                reason: format!("{other} is not dev, staging or prod"),
            }),
        }
    }
}

/// The shared runtime configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeConfig {
    // --- identity -----------------------------------------------------------
    pub service_name: String,
    pub environment: Environment,

    // --- avenue 1: database -------------------------------------------------
    pub database_url: Secret,
    pub database_read_only: bool,
    pub database_max_connections: u32,

    // --- avenue 2: HTTP -----------------------------------------------------
    pub http_bind: String,
    pub request_timeout: Duration,
    pub trusted_proxy_hops: u8,

    // --- avenue 3: TCP and websocket ---------------------------------------
    pub tcp_bind: Option<String>,
    pub ws_path: String,
    pub ws_max_frame_bytes: usize,

    // --- avenue 4: NATS -----------------------------------------------------
    pub nats_url: Option<String>,
    pub nats_subject_prefix: String,

    // --- shared-auth --------------------------------------------------------
    pub shared_auth_base: String,
    pub shared_auth_audience: String,
    pub shared_auth_introspect_secret: Secret,

    // --- coordination -------------------------------------------------------
    pub locks_fiducia_enabled: bool,
    pub locks_pg_advisory_enabled: bool,
    pub locks_require_fencing: bool,
    pub fiducia_base: Option<String>,
    pub fiducia_api_key: Secret,

    // --- observability and chat --------------------------------------------
    pub otel_endpoint: Option<String>,
    pub log_level: String,
    pub chat_api_base: Option<String>,
    pub sync_enabled: bool,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            service_name: "gha-indie-worker".to_owned(),
            environment: Environment::default(),
            database_url: Secret::default(),
            database_read_only: true,
            database_max_connections: 8,
            http_bind: "0.0.0.0:8080".to_owned(),
            request_timeout: Duration::from_secs(30),
            trusted_proxy_hops: 1,
            tcp_bind: None,
            ws_path: "/v1/ws".to_owned(),
            ws_max_frame_bytes: crate::protocol::MAX_FRAME_BYTES,
            nats_url: None,
            nats_subject_prefix: "giw".to_owned(),
            shared_auth_base: String::new(),
            shared_auth_audience: String::new(),
            shared_auth_introspect_secret: Secret::default(),
            locks_fiducia_enabled: false,
            locks_pg_advisory_enabled: false,
            locks_require_fencing: false,
            fiducia_base: None,
            fiducia_api_key: Secret::default(),
            otel_endpoint: None,
            log_level: "info".to_owned(),
            chat_api_base: None,
            sync_enabled: false,
        }
    }
}

/// Every environment variable this module reads, with the field it fills.
/// `.cli-flags.toml` must declare exactly these, and the test below proves the
/// list has no duplicates and keeps the prefix.
pub const ENV_VARS: [(&str, &str); 25] = [
    ("GHA_INDIE_WORKER_SERVICE_NAME", "service_name"),
    ("GHA_INDIE_WORKER_ENV", "environment"),
    ("GHA_INDIE_WORKER_DATABASE_URL", "database_url"),
    ("GHA_INDIE_WORKER_DB_READ_ONLY", "database_read_only"),
    (
        "GHA_INDIE_WORKER_DB_MAX_CONNECTIONS",
        "database_max_connections",
    ),
    ("GHA_INDIE_WORKER_HTTP_BIND", "http_bind"),
    (
        "GHA_INDIE_WORKER_REQUEST_TIMEOUT_SECONDS",
        "request_timeout",
    ),
    ("GHA_INDIE_WORKER_TRUSTED_PROXY_HOPS", "trusted_proxy_hops"),
    ("GHA_INDIE_WORKER_TCP_BIND", "tcp_bind"),
    ("GHA_INDIE_WORKER_WS_PATH", "ws_path"),
    ("GHA_INDIE_WORKER_WS_MAX_FRAME_BYTES", "ws_max_frame_bytes"),
    ("GHA_INDIE_WORKER_NATS_URL", "nats_url"),
    (
        "GHA_INDIE_WORKER_NATS_SUBJECT_PREFIX",
        "nats_subject_prefix",
    ),
    ("GHA_INDIE_WORKER_SHARED_AUTH_BASE", "shared_auth_base"),
    (
        "GHA_INDIE_WORKER_SHARED_AUTH_AUDIENCE",
        "shared_auth_audience",
    ),
    (
        "GHA_INDIE_WORKER_SHARED_AUTH_INTROSPECT_SECRET",
        "shared_auth_introspect_secret",
    ),
    (
        "GHA_INDIE_WORKER_LOCKS_FIDUCIA_ENABLED",
        "locks_fiducia_enabled",
    ),
    (
        "GHA_INDIE_WORKER_LOCKS_PG_ADVISORY_ENABLED",
        "locks_pg_advisory_enabled",
    ),
    (
        "GHA_INDIE_WORKER_LOCKS_REQUIRE_FENCING",
        "locks_require_fencing",
    ),
    ("GHA_INDIE_WORKER_FIDUCIA_BASE", "fiducia_base"),
    ("GHA_INDIE_WORKER_FIDUCIA_API_KEY", "fiducia_api_key"),
    ("GHA_INDIE_WORKER_OTEL_ENDPOINT", "otel_endpoint"),
    ("GHA_INDIE_WORKER_LOG_LEVEL", "log_level"),
    ("GHA_INDIE_WORKER_CHAT_API_BASE", "chat_api_base"),
    ("GHA_INDIE_WORKER_SYNC_ENABLED", "sync_enabled"),
];

/// Names whose values must never be logged, echoed in an error, or serialized.
pub const SECRET_ENV_VARS: [&str; 3] = [
    "GHA_INDIE_WORKER_DATABASE_URL",
    "GHA_INDIE_WORKER_SHARED_AUTH_INTROSPECT_SECRET",
    "GHA_INDIE_WORKER_FIDUCIA_API_KEY",
];

fn flag_bool(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

impl RuntimeConfig {
    /// The env vars this module reads.
    #[must_use]
    pub fn env_names() -> Vec<&'static str> {
        let mut names: Vec<&'static str> = ENV_VARS.iter().map(|(name, _)| *name).collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    /// Read the configuration from an arbitrary lookup.
    ///
    /// `std::env::var` is not called here: pass `|name| std::env::var(name).ok()`
    /// from `main`, and a closure over a map from tests.
    ///
    /// # Errors
    ///
    /// [`ConfigError::Missing`] for a required variable, or
    /// [`ConfigError::Invalid`] when a value does not parse or fails a policy
    /// check (`prod` requires shared-auth and forbids a plaintext database URL).
    pub fn from_lookup(get: &dyn Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let mut config = RuntimeConfig::default();
        let text = |name: &'static str| get(name).filter(|v| !v.trim().is_empty());

        if let Some(value) = text("GHA_INDIE_WORKER_SERVICE_NAME") {
            config.service_name = value;
        }
        if let Some(value) = text("GHA_INDIE_WORKER_ENV") {
            config.environment = Environment::parse(&value)?;
        }

        config.database_url = Secret::new(
            text("GHA_INDIE_WORKER_DATABASE_URL")
                .ok_or(ConfigError::Missing("GHA_INDIE_WORKER_DATABASE_URL"))?,
        );
        let scheme_ok = config.database_url.expose().starts_with("postgres://")
            || config.database_url.expose().starts_with("postgresql://");
        if !scheme_ok {
            return Err(ConfigError::Invalid {
                name: "GHA_INDIE_WORKER_DATABASE_URL",
                // deliberately does not echo the value
                reason: "must use the postgres or postgresql scheme".to_owned(),
            });
        }
        if let Some(value) = text("GHA_INDIE_WORKER_DB_READ_ONLY") {
            config.database_read_only = flag_bool(&value);
        }
        if let Some(value) = text("GHA_INDIE_WORKER_DB_MAX_CONNECTIONS") {
            config.database_max_connections = value.parse().map_err(|_| ConfigError::Invalid {
                name: "GHA_INDIE_WORKER_DB_MAX_CONNECTIONS",
                reason: format!("{value} is not a positive integer"),
            })?;
        }

        if let Some(value) = text("GHA_INDIE_WORKER_HTTP_BIND") {
            config.http_bind = value;
        }
        if let Some(value) = text("GHA_INDIE_WORKER_REQUEST_TIMEOUT_SECONDS") {
            let seconds: u64 = value.parse().map_err(|_| ConfigError::Invalid {
                name: "GHA_INDIE_WORKER_REQUEST_TIMEOUT_SECONDS",
                reason: format!("{value} is not a whole number of seconds"),
            })?;
            config.request_timeout = Duration::from_secs(seconds);
        }
        if let Some(value) = text("GHA_INDIE_WORKER_TRUSTED_PROXY_HOPS") {
            config.trusted_proxy_hops = value.parse().map_err(|_| ConfigError::Invalid {
                name: "GHA_INDIE_WORKER_TRUSTED_PROXY_HOPS",
                reason: format!("{value} is not a small non-negative integer"),
            })?;
        }

        config.tcp_bind = text("GHA_INDIE_WORKER_TCP_BIND");
        if let Some(value) = text("GHA_INDIE_WORKER_WS_PATH") {
            if !value.starts_with('/') {
                return Err(ConfigError::Invalid {
                    name: "GHA_INDIE_WORKER_WS_PATH",
                    reason: "must start with /".to_owned(),
                });
            }
            config.ws_path = value;
        }
        if let Some(value) = text("GHA_INDIE_WORKER_WS_MAX_FRAME_BYTES") {
            let bytes: usize = value.parse().map_err(|_| ConfigError::Invalid {
                name: "GHA_INDIE_WORKER_WS_MAX_FRAME_BYTES",
                reason: format!("{value} is not a byte count"),
            })?;
            if bytes == 0 || bytes > crate::protocol::MAX_FRAME_BYTES {
                return Err(ConfigError::Invalid {
                    name: "GHA_INDIE_WORKER_WS_MAX_FRAME_BYTES",
                    reason: format!("must be 1..={}", crate::protocol::MAX_FRAME_BYTES),
                });
            }
            config.ws_max_frame_bytes = bytes;
        }

        config.nats_url = text("GHA_INDIE_WORKER_NATS_URL");
        if let Some(value) = text("GHA_INDIE_WORKER_NATS_SUBJECT_PREFIX") {
            config.nats_subject_prefix = value;
        }

        config.shared_auth_base = text("GHA_INDIE_WORKER_SHARED_AUTH_BASE").unwrap_or_default();
        config.shared_auth_audience =
            text("GHA_INDIE_WORKER_SHARED_AUTH_AUDIENCE").unwrap_or_default();
        config.shared_auth_introspect_secret =
            Secret::new(text("GHA_INDIE_WORKER_SHARED_AUTH_INTROSPECT_SECRET").unwrap_or_default());

        config.locks_fiducia_enabled = text("GHA_INDIE_WORKER_LOCKS_FIDUCIA_ENABLED")
            .as_deref()
            .is_some_and(flag_bool);
        config.locks_pg_advisory_enabled = text("GHA_INDIE_WORKER_LOCKS_PG_ADVISORY_ENABLED")
            .as_deref()
            .is_some_and(flag_bool);
        config.locks_require_fencing = text("GHA_INDIE_WORKER_LOCKS_REQUIRE_FENCING")
            .as_deref()
            .is_some_and(flag_bool);
        config.fiducia_base = text("GHA_INDIE_WORKER_FIDUCIA_BASE");
        config.fiducia_api_key =
            Secret::new(text("GHA_INDIE_WORKER_FIDUCIA_API_KEY").unwrap_or_default());
        if config.locks_fiducia_enabled
            && (config.fiducia_base.is_none() || config.fiducia_api_key.is_empty())
        {
            return Err(ConfigError::Invalid {
                name: "GHA_INDIE_WORKER_LOCKS_FIDUCIA_ENABLED",
                reason: "fiducia locking needs GHA_INDIE_WORKER_FIDUCIA_BASE and GHA_INDIE_WORKER_FIDUCIA_API_KEY"
                    .to_owned(),
            });
        }

        config.otel_endpoint = text("GHA_INDIE_WORKER_OTEL_ENDPOINT");
        if let Some(value) = text("GHA_INDIE_WORKER_LOG_LEVEL") {
            config.log_level = value;
        }
        config.chat_api_base = text("GHA_INDIE_WORKER_CHAT_API_BASE");
        config.sync_enabled = text("GHA_INDIE_WORKER_SYNC_ENABLED")
            .as_deref()
            .is_some_and(flag_bool);

        config.check_environment_policy()?;
        Ok(config)
    }

    /// Read from the process environment.
    ///
    /// # Errors
    ///
    /// As [`RuntimeConfig::from_lookup`].
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(&|name| std::env::var(name).ok())
    }

    /// Rules that only bind in production.
    ///
    /// # Errors
    ///
    /// [`ConfigError::Missing`] when a production deployment lacks shared-auth,
    /// or [`ConfigError::Invalid`] when it would run without coordination.
    fn check_environment_policy(&self) -> Result<(), ConfigError> {
        if self.environment != Environment::Prod {
            return Ok(());
        }
        if self.shared_auth_base.is_empty() {
            return Err(ConfigError::Missing("GHA_INDIE_WORKER_SHARED_AUTH_BASE"));
        }
        if self.shared_auth_audience.is_empty() {
            return Err(ConfigError::Missing(
                "GHA_INDIE_WORKER_SHARED_AUTH_AUDIENCE",
            ));
        }
        if self.shared_auth_introspect_secret.is_empty() {
            return Err(ConfigError::Missing(
                "GHA_INDIE_WORKER_SHARED_AUTH_INTROSPECT_SECRET",
            ));
        }
        Ok(())
    }

    /// The lock policy this configuration implies.
    #[must_use]
    pub const fn lock_policy(&self) -> crate::locks::LockPolicy {
        crate::locks::LockPolicy {
            fiducia_enabled: self.locks_fiducia_enabled,
            pg_advisory_enabled: self.locks_pg_advisory_enabled,
            require_fencing: self.locks_require_fencing,
        }
    }

    /// A NATS subject in the fleet's shape: `giw.<env>.<domain>.<event>`.
    #[must_use]
    pub fn subject(&self, domain: &str, event: &str) -> String {
        format!(
            "{}.{}.{domain}.{event}",
            self.nats_subject_prefix,
            self.environment.as_str()
        )
    }

    /// A redacted, sorted view for a startup log line. Secrets are elided; this
    /// is the only sanctioned way to print the configuration.
    #[must_use]
    pub fn redacted_summary(&self) -> BTreeMap<&'static str, String> {
        let mut out = BTreeMap::new();
        out.insert("service", self.service_name.clone());
        out.insert("env", self.environment.as_str().to_owned());
        out.insert("httpBind", self.http_bind.clone());
        out.insert(
            "tcpBind",
            self.tcp_bind
                .clone()
                .unwrap_or_else(|| "<disabled>".to_owned()),
        );
        out.insert("wsPath", self.ws_path.clone());
        out.insert("dbReadOnly", self.database_read_only.to_string());
        out.insert("dbUrl", "<redacted>".to_owned());
        out.insert(
            "nats",
            self.nats_url
                .clone()
                .unwrap_or_else(|| "<disabled>".to_owned()),
        );
        out.insert("locksFiducia", self.locks_fiducia_enabled.to_string());
        out.insert(
            "locksPgAdvisory",
            self.locks_pg_advisory_enabled.to_string(),
        );
        out.insert("syncEnabled", self.sync_enabled.to_string());
        out.insert("logLevel", self.log_level.clone());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |name: &str| map.get(name).cloned()
    }

    fn minimal() -> Vec<(&'static str, &'static str)> {
        vec![(
            "GHA_INDIE_WORKER_DATABASE_URL",
            "postgresql://runtime:secret@db.example/giw?sslmode=verify-full",
        )]
    }

    #[test]
    fn the_database_url_is_required_and_scheme_checked() {
        assert_eq!(
            RuntimeConfig::from_lookup(&lookup(&[])).unwrap_err(),
            ConfigError::Missing("GHA_INDIE_WORKER_DATABASE_URL")
        );
        let err = RuntimeConfig::from_lookup(&lookup(&[(
            "GHA_INDIE_WORKER_DATABASE_URL",
            "mysql://nope/db",
        )]))
        .unwrap_err();
        assert!(
            matches!(err, ConfigError::Invalid { name, .. } if name == "GHA_INDIE_WORKER_DATABASE_URL")
        );
    }

    #[test]
    fn an_invalid_value_error_never_echoes_a_secret() {
        let err = RuntimeConfig::from_lookup(&lookup(&[(
            "GHA_INDIE_WORKER_DATABASE_URL",
            "mysql://user:hunter2@db/giw",
        )]))
        .unwrap_err();
        assert!(!err.to_string().contains("hunter2"));
    }

    #[test]
    fn debug_output_redacts_every_secret() {
        let mut pairs = minimal();
        pairs.push((
            "GHA_INDIE_WORKER_SHARED_AUTH_INTROSPECT_SECRET",
            "introspect-hunter2",
        ));
        pairs.push(("GHA_INDIE_WORKER_FIDUCIA_API_KEY", "fiducia-hunter2"));
        let config = RuntimeConfig::from_lookup(&lookup(&pairs)).unwrap();
        let rendered = format!("{config:?}");
        assert!(!rendered.contains("hunter2"), "{rendered}");
        assert!(!rendered.contains("db.example"), "{rendered}");
        assert!(rendered.contains("Secret(<redacted>)"));
        assert!(!config
            .redacted_summary()
            .values()
            .any(|v| v.contains("hunter2")));
    }

    #[test]
    fn defaults_are_the_documented_ones() {
        let config = RuntimeConfig::from_lookup(&lookup(&minimal())).unwrap();
        assert_eq!(config.http_bind, "0.0.0.0:8080");
        assert_eq!(config.ws_path, "/v1/ws");
        assert_eq!(config.request_timeout, Duration::from_secs(30));
        assert!(config.database_read_only);
        assert_eq!(config.tcp_bind, None);
        assert!(!config.lock_policy().is_coordinated());
    }

    #[test]
    fn booleans_accept_the_flags_2_env_spellings() {
        for raw in ["1", "true", "TRUE", "yes", "on"] {
            let mut pairs = minimal();
            pairs.push(("GHA_INDIE_WORKER_SYNC_ENABLED", raw));
            assert!(
                RuntimeConfig::from_lookup(&lookup(&pairs))
                    .unwrap()
                    .sync_enabled,
                "{raw}"
            );
        }
        for raw in ["0", "false", "no", "off", "maybe"] {
            let mut pairs = minimal();
            pairs.push(("GHA_INDIE_WORKER_SYNC_ENABLED", raw));
            assert!(
                !RuntimeConfig::from_lookup(&lookup(&pairs))
                    .unwrap()
                    .sync_enabled,
                "{raw}"
            );
        }
    }

    #[test]
    fn fiducia_locking_requires_its_credentials() {
        let mut pairs = minimal();
        pairs.push(("GHA_INDIE_WORKER_LOCKS_FIDUCIA_ENABLED", "true"));
        let err = RuntimeConfig::from_lookup(&lookup(&pairs)).unwrap_err();
        assert!(
            matches!(err, ConfigError::Invalid { name, .. } if name == "GHA_INDIE_WORKER_LOCKS_FIDUCIA_ENABLED")
        );

        pairs.push(("GHA_INDIE_WORKER_FIDUCIA_BASE", "https://fiducia.cloud"));
        pairs.push(("GHA_INDIE_WORKER_FIDUCIA_API_KEY", "k"));
        let config = RuntimeConfig::from_lookup(&lookup(&pairs)).unwrap();
        assert!(config.lock_policy().fiducia_enabled);
    }

    #[test]
    fn production_requires_shared_auth() {
        let mut pairs = minimal();
        pairs.push(("GHA_INDIE_WORKER_ENV", "prod"));
        assert_eq!(
            RuntimeConfig::from_lookup(&lookup(&pairs)).unwrap_err(),
            ConfigError::Missing("GHA_INDIE_WORKER_SHARED_AUTH_BASE")
        );
        pairs.push((
            "GHA_INDIE_WORKER_SHARED_AUTH_BASE",
            "https://auth.indiebuild.dev",
        ));
        pairs.push(("GHA_INDIE_WORKER_SHARED_AUTH_AUDIENCE", "api"));
        pairs.push(("GHA_INDIE_WORKER_SHARED_AUTH_INTROSPECT_SECRET", "s"));
        assert!(RuntimeConfig::from_lookup(&lookup(&pairs)).is_ok());
    }

    #[test]
    fn staging_does_not_require_shared_auth() {
        let mut pairs = minimal();
        pairs.push(("GHA_INDIE_WORKER_ENV", "staging"));
        assert!(RuntimeConfig::from_lookup(&lookup(&pairs)).is_ok());
    }

    #[test]
    fn frame_bounds_are_clamped_to_the_protocol_ceiling() {
        let mut pairs = minimal();
        pairs.push(("GHA_INDIE_WORKER_WS_MAX_FRAME_BYTES", "99999999"));
        assert!(RuntimeConfig::from_lookup(&lookup(&pairs)).is_err());
        let mut pairs = minimal();
        pairs.push(("GHA_INDIE_WORKER_WS_MAX_FRAME_BYTES", "4096"));
        assert_eq!(
            RuntimeConfig::from_lookup(&lookup(&pairs))
                .unwrap()
                .ws_max_frame_bytes,
            4096
        );
    }

    #[test]
    fn ws_path_must_be_a_path() {
        let mut pairs = minimal();
        pairs.push(("GHA_INDIE_WORKER_WS_PATH", "v1/ws"));
        assert!(RuntimeConfig::from_lookup(&lookup(&pairs)).is_err());
    }

    #[test]
    fn subjects_follow_the_fleet_shape() {
        let mut pairs = minimal();
        pairs.push(("GHA_INDIE_WORKER_ENV", "prod"));
        pairs.push(("GHA_INDIE_WORKER_SHARED_AUTH_BASE", "https://auth"));
        pairs.push(("GHA_INDIE_WORKER_SHARED_AUTH_AUDIENCE", "api"));
        pairs.push(("GHA_INDIE_WORKER_SHARED_AUTH_INTROSPECT_SECRET", "s"));
        let config = RuntimeConfig::from_lookup(&lookup(&pairs)).unwrap();
        assert_eq!(config.subject("runs", "started"), "giw.prod.runs.started");
    }

    #[test]
    fn env_names_are_unique_and_prefixed() {
        let names = RuntimeConfig::env_names();
        for name in &names {
            assert!(name.starts_with("GHA_INDIE_WORKER_"), "{name}");
            assert_eq!(name.to_ascii_uppercase(), **name, "{name}");
        }
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "duplicate env var in ENV_VARS");
    }

    #[test]
    fn every_secret_var_is_declared() {
        let names = RuntimeConfig::env_names();
        for secret in SECRET_ENV_VARS {
            assert!(names.contains(&secret), "{secret} is not in ENV_VARS");
        }
    }

    #[test]
    fn unknown_environment_is_rejected() {
        let mut pairs = minimal();
        pairs.push(("GHA_INDIE_WORKER_ENV", "qa"));
        assert!(RuntimeConfig::from_lookup(&lookup(&pairs)).is_err());
    }
}
