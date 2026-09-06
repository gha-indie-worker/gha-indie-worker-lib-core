//! Which plane a process belongs to, and the boot-time invariants that keep the two apart.
//!
//! `gha-indie-worker` runs a **product** plane (web + api, customer traffic) and an **admin**
//! plane (admin-web + admin-api, super-admins only). The network and IAM separation lives in
//! `gha-indie-worker-infra`; this module is the third, independent layer: a process refuses to
//! start when its environment does not match the plane it was compiled for.
//!
//! Everything here is a pure function of a `(name -> value)` lookup, so the rules are unit-tested
//! without touching the real environment.

use core::fmt;

/// The two planes. There is deliberately no `Both`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Plane {
    Product,
    Admin,
}

impl Plane {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Plane::Product => "product",
            Plane::Admin => "admin",
        }
    }
}

impl fmt::Display for Plane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Environment variable names. Kept as constants so the infra terraform, the k8s manifests, the
/// `.cli-flags.toml` env ignore-list and this crate cannot drift apart silently.
pub mod env {
    pub const PRODUCT_DATABASE_URL: &str = "DATABASE_URL";
    pub const PRODUCT_AUTH_DATABASE_URL: &str = "AUTH_DATABASE_URL";
    pub const ADMIN_DATABASE_URL: &str = "GHA_INDIE_WORKER_ADMIN_DATABASE_URL";
    pub const ADMIN_RDS_URL: &str = "GHA_INDIE_WORKER_ADMIN_RDS_URL";
    pub const PLANE: &str = "GHA_INDIE_WORKER_PLANE";
    pub const SHARED_AUTH_ISSUER: &str = "SHARED_AUTH_ISSUER";
    pub const SHARED_AUTH_ADMIN_ISSUER: &str = "SHARED_AUTH_ADMIN_ISSUER";
    pub const ADMIN_ALLOWLIST: &str = "GHA_INDIE_WORKER_ADMIN_ALLOWLIST";
    pub const ADMIN_ALLOW_PUBLIC_BIND: &str = "GHA_INDIE_WORKER_ADMIN_ALLOW_PUBLIC_BIND";
}

/// Why a process refused to start.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlaneError {
    /// An admin process was given the product database and no admin database: almost always a
    /// copy-pasted env block, and exactly the mistake that would put admin queries on customer data.
    AdminMissingAdminDatabase,
    /// A product process can see an admin connection string. Nothing in the product plane should
    /// ever hold this, whatever it intends to do with it.
    ProductSawAdminDatabase { variable: &'static str },
    /// The admin plane must have its own issuer, distinct from the customer realm's.
    AdminIssuerMissing,
    /// Both issuers are set to the same value, which would let a customer token pass an admin check.
    IssuersIdentical { issuer: String },
    /// The admin plane authorizes by explicit allow-list; an empty one means "nobody", and a
    /// process that starts with it is a process whose admin routes are all dead.
    AdminAllowlistEmpty,
    /// `GHA_INDIE_WORKER_PLANE` disagrees with the plane this binary was built for.
    PlaneMismatch { declared: String, expected: Plane },
}

impl fmt::Display for PlaneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlaneError::AdminMissingAdminDatabase => write!(
                f,
                "{} is set but {} is not: an admin process must never fall back to the product database",
                env::PRODUCT_DATABASE_URL,
                env::ADMIN_DATABASE_URL
            ),
            PlaneError::ProductSawAdminDatabase { variable } => write!(
                f,
                "{variable} is set in a product process: admin connection strings must not be granted to the product plane"
            ),
            PlaneError::AdminIssuerMissing => {
                write!(f, "{} is required on the admin plane", env::SHARED_AUTH_ADMIN_ISSUER)
            }
            PlaneError::IssuersIdentical { issuer } => write!(
                f,
                "{} and {} are both {issuer}: the admin realm must be a different issuer, or a customer token satisfies an admin check",
                env::SHARED_AUTH_ISSUER,
                env::SHARED_AUTH_ADMIN_ISSUER
            ),
            PlaneError::AdminAllowlistEmpty => write!(
                f,
                "{} is empty: the admin plane authorizes by explicit allow-list and would accept nobody",
                env::ADMIN_ALLOWLIST
            ),
            PlaneError::PlaneMismatch { declared, expected } => write!(
                f,
                "{} is {declared:?} but this binary belongs to the {expected} plane",
                env::PLANE
            ),
        }
    }
}

/// What a validated process knows about its own plane.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlaneConfig {
    pub plane: Plane,
    /// Present on the admin plane only.
    pub admin_database_url: Option<String>,
    /// Present on the product plane only.
    pub product_database_url: Option<String>,
    pub auth_database_url: Option<String>,
    pub customer_issuer: Option<String>,
    pub admin_issuer: Option<String>,
    /// Non-empty on the admin plane.
    pub admin_allowlist: Vec<String>,
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

/// Validate the environment for `plane`, using `lookup` instead of the real environment so the
/// rules are testable. Returns the first violation; there is no "warn and continue" path.
///
/// # Errors
/// Any of [`PlaneError`].
pub fn validate<F>(plane: Plane, lookup: F) -> Result<PlaneConfig, PlaneError>
where
    F: Fn(&str) -> Option<String>,
{
    let get = |name: &str| non_empty(lookup(name).as_deref());

    if let Some(declared) = get(env::PLANE) {
        if declared != plane.as_str() {
            return Err(PlaneError::PlaneMismatch {
                declared,
                expected: plane,
            });
        }
    }

    let product_db = get(env::PRODUCT_DATABASE_URL);
    let admin_db = get(env::ADMIN_DATABASE_URL);
    let admin_rds = get(env::ADMIN_RDS_URL);
    let customer_issuer = get(env::SHARED_AUTH_ISSUER);
    let admin_issuer = get(env::SHARED_AUTH_ADMIN_ISSUER);

    if let (Some(customer), Some(admin)) = (customer_issuer.as_deref(), admin_issuer.as_deref()) {
        if normalize_issuer(customer) == normalize_issuer(admin) {
            return Err(PlaneError::IssuersIdentical {
                issuer: admin.to_owned(),
            });
        }
    }

    match plane {
        Plane::Admin => {
            if product_db.is_some() && admin_db.is_none() {
                return Err(PlaneError::AdminMissingAdminDatabase);
            }
            if admin_issuer.is_none() {
                return Err(PlaneError::AdminIssuerMissing);
            }
            let allowlist = parse_allowlist(get(env::ADMIN_ALLOWLIST).as_deref());
            if allowlist.is_empty() {
                return Err(PlaneError::AdminAllowlistEmpty);
            }
            Ok(PlaneConfig {
                plane,
                admin_database_url: admin_db.or(admin_rds),
                product_database_url: None,
                auth_database_url: None,
                customer_issuer,
                admin_issuer,
                admin_allowlist: allowlist,
            })
        }
        Plane::Product => {
            if admin_db.is_some() {
                return Err(PlaneError::ProductSawAdminDatabase {
                    variable: env::ADMIN_DATABASE_URL,
                });
            }
            if admin_rds.is_some() {
                return Err(PlaneError::ProductSawAdminDatabase {
                    variable: env::ADMIN_RDS_URL,
                });
            }
            Ok(PlaneConfig {
                plane,
                admin_database_url: None,
                product_database_url: product_db,
                auth_database_url: get(env::PRODUCT_AUTH_DATABASE_URL),
                customer_issuer,
                admin_issuer: None,
                admin_allowlist: Vec::new(),
            })
        }
    }
}

/// Issuers compare after trimming and dropping a single trailing slash, so
/// `https://auth.example` and `https://auth.example/` are the same issuer — and a config that
/// differs only by that slash does not silently become "two realms".
#[must_use]
pub fn normalize_issuer(issuer: &str) -> &str {
    issuer.trim().trim_end_matches('/')
}

/// Comma-separated allow-list, trimmed, de-duplicated, order preserved.
#[must_use]
pub fn parse_allowlist(raw: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for entry in raw.unwrap_or("").split(',') {
        let entry = entry.trim();
        if entry.is_empty() || out.iter().any(|existing| existing == entry) {
            continue;
        }
        out.push(entry.to_owned());
    }
    out
}

/// A bind address is only allowed to be non-loopback when the operator opted in. Cloud Run and
/// k8s both require `0.0.0.0`, so the opt-in exists — but it must be a deliberate env setting and
/// never the default, so a laptop run cannot accidentally expose the admin console on a LAN.
///
/// # Errors
/// Returns the offending address when it is public and no opt-in was given.
pub fn check_admin_bind(bind: &str, allow_public: bool) -> Result<(), String> {
    if allow_public {
        return Ok(());
    }
    let host = bind.rsplit_once(':').map_or(bind, |(host, _)| host);
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let loopback = host == "localhost" || host == "::1" || host.starts_with("127.");
    if loopback {
        Ok(())
    } else {
        Err(format!(
            "refusing to bind the admin plane to {bind}: set {}=1 only where an ingress boundary (Cloud Run internal ingress, k8s NetworkPolicy) is the real control",
            env::ADMIN_ALLOW_PUBLIC_BIND
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |name: &str| map.get(name).cloned()
    }

    #[test]
    fn admin_with_product_database_and_no_admin_database_refuses_to_start() {
        let err = validate(
            Plane::Admin,
            env_of(&[
                (env::PRODUCT_DATABASE_URL, "postgres://canonical"),
                (
                    env::SHARED_AUTH_ADMIN_ISSUER,
                    "https://auth-admin.indiebuild.dev",
                ),
                (env::ADMIN_ALLOWLIST, "018f2ee1-3ef5-7f4b-9e27-494ce50e8c4f"),
            ]),
        )
        .unwrap_err();
        assert_eq!(err, PlaneError::AdminMissingAdminDatabase);
    }

    #[test]
    fn product_holding_an_admin_connection_string_refuses_to_start() {
        for variable in [env::ADMIN_DATABASE_URL, env::ADMIN_RDS_URL] {
            let err =
                validate(Plane::Product, env_of(&[(variable, "postgres://admin")])).unwrap_err();
            assert_eq!(err, PlaneError::ProductSawAdminDatabase { variable });
        }
    }

    #[test]
    fn identical_issuers_are_rejected_including_a_trailing_slash() {
        let err = validate(
            Plane::Admin,
            env_of(&[
                (env::SHARED_AUTH_ISSUER, "https://auth.indiebuild.dev"),
                (
                    env::SHARED_AUTH_ADMIN_ISSUER,
                    "https://auth.indiebuild.dev/",
                ),
                (env::ADMIN_ALLOWLIST, "a"),
            ]),
        )
        .unwrap_err();
        assert!(matches!(err, PlaneError::IssuersIdentical { .. }));
    }

    #[test]
    fn admin_needs_a_non_empty_allowlist_and_its_own_issuer() {
        let missing_issuer =
            validate(Plane::Admin, env_of(&[(env::ADMIN_ALLOWLIST, "a")])).unwrap_err();
        assert_eq!(missing_issuer, PlaneError::AdminIssuerMissing);

        let empty_allowlist = validate(
            Plane::Admin,
            env_of(&[
                (
                    env::SHARED_AUTH_ADMIN_ISSUER,
                    "https://auth-admin.indiebuild.dev",
                ),
                (env::ADMIN_ALLOWLIST, " , ,"),
            ]),
        )
        .unwrap_err();
        assert_eq!(empty_allowlist, PlaneError::AdminAllowlistEmpty);
    }

    #[test]
    fn declared_plane_must_match_the_binary() {
        let err = validate(Plane::Product, env_of(&[(env::PLANE, "admin")])).unwrap_err();
        assert_eq!(
            err,
            PlaneError::PlaneMismatch {
                declared: "admin".into(),
                expected: Plane::Product
            }
        );
    }

    #[test]
    fn a_healthy_product_environment_validates() {
        let config = validate(
            Plane::Product,
            env_of(&[
                (env::PLANE, "product"),
                (env::PRODUCT_DATABASE_URL, "postgres://canonical"),
                (env::PRODUCT_AUTH_DATABASE_URL, "postgres://auth"),
                (env::SHARED_AUTH_ISSUER, "https://auth.indiebuild.dev"),
            ]),
        )
        .unwrap();
        assert_eq!(config.plane, Plane::Product);
        assert!(config.admin_database_url.is_none());
        assert_eq!(config.auth_database_url.as_deref(), Some("postgres://auth"));
    }

    #[test]
    fn a_healthy_admin_environment_validates_and_keeps_the_allowlist_order() {
        let config = validate(
            Plane::Admin,
            env_of(&[
                (env::ADMIN_DATABASE_URL, "postgres://admin"),
                (env::SHARED_AUTH_ISSUER, "https://auth.indiebuild.dev"),
                (
                    env::SHARED_AUTH_ADMIN_ISSUER,
                    "https://auth-admin.indiebuild.dev",
                ),
                (env::ADMIN_ALLOWLIST, "b, a ,b,c"),
            ]),
        )
        .unwrap();
        assert_eq!(config.admin_allowlist, vec!["b", "a", "c"]);
        assert!(config.product_database_url.is_none());
    }

    #[test]
    fn admin_bind_is_loopback_unless_the_operator_opted_in() {
        assert!(check_admin_bind("127.0.0.1:8787", false).is_ok());
        assert!(check_admin_bind("[::1]:8787", false).is_ok());
        assert!(check_admin_bind("localhost:8787", false).is_ok());
        assert!(check_admin_bind("0.0.0.0:8787", false).is_err());
        assert!(check_admin_bind("0.0.0.0:8787", true).is_ok());
    }
}
