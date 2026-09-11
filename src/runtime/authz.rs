//! Dual-realm authorization: the pure half.
//!
//! `shared-auth` federates two identity realms — a **customer** realm (backed by the Supabase
//! `auth` project and the Neon `auth` database) and an **admin** realm (backed by the admin
//! projects). Both answer RFC 7662-shaped introspection. Everything that decides whether an
//! answer is acceptable lives here as pure functions over a plain [`Introspection`] struct, so
//! the rules are exhaustively tested without a network, a clock skew, or a live issuer.
//!
//! The effectful half — an HTTP client that produces an [`Introspection`] — is bound in the
//! server crates behind the `shared-auth` feature. That is the only part that needs the private
//! `shared-auth-client` crate, so a mistake there cannot weaken the rules in this file.

use core::fmt;

/// Which identity realm a token came from. A token is only ever valid in one.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Realm {
    /// End users and organizations. Never accepted by the admin plane.
    Customer,
    /// Super-admins. Never accepted by the product plane.
    Admin,
}

impl Realm {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Realm::Customer => "customer",
            Realm::Admin => "admin",
        }
    }
}

impl fmt::Display for Realm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The audience a service introspects **for**. Introspecting without an audience is how a token
/// minted for one service gets replayed against another, so every call names one.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Audience {
    Web,
    Api,
    AdminWeb,
    AdminApi,
}

impl Audience {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Audience::Web => "indiebuild-web",
            Audience::Api => "indiebuild-api",
            Audience::AdminWeb => "indiebuild-admin-web",
            Audience::AdminApi => "indiebuild-admin",
        }
    }

    /// The realm this audience belongs to. This is the table that makes "an admin audience can
    /// only be satisfied by an admin-realm token" a type-level fact rather than a convention.
    #[must_use]
    pub const fn realm(self) -> Realm {
        match self {
            Audience::Web | Audience::Api => Realm::Customer,
            Audience::AdminWeb | Audience::AdminApi => Realm::Admin,
        }
    }
}

/// An RFC 7662 introspection answer, reduced to the fields we act on.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Introspection {
    pub active: bool,
    pub issuer: Option<String>,
    /// `aud` may be a single value or a list; both are represented here as a list.
    pub audiences: Vec<String>,
    pub subject: Option<String>,
    /// `realm` claim when present, else the `project` claim.
    pub realm: Option<String>,
    pub scopes: Vec<String>,
    /// Authenticator assurance level (`aal`); absent means 1.
    pub assurance_level: Option<u8>,
    /// Organization the subject acted as, for B2B tokens.
    pub organization: Option<String>,
    /// Seconds since the epoch; `None` means the issuer did not say.
    pub expires_at: Option<i64>,
    pub not_before: Option<i64>,
}

/// What a caller requires of a token before it will act on it.
#[derive(Clone, Debug)]
pub struct Requirements<'a> {
    pub issuer: &'a str,
    pub audience: Audience,
    pub required_scopes: &'a [&'a str],
    /// Minimum authenticator assurance level. Admin routes should ask for 2.
    pub min_assurance_level: u8,
    /// Seconds of tolerated clock skew when checking `exp`/`nbf`.
    pub leeway_seconds: i64,
}

impl<'a> Requirements<'a> {
    #[must_use]
    pub const fn new(issuer: &'a str, audience: Audience) -> Self {
        Self {
            issuer,
            audience,
            required_scopes: &[],
            min_assurance_level: 1,
            leeway_seconds: 30,
        }
    }

    #[must_use]
    pub const fn with_scopes(mut self, scopes: &'a [&'a str]) -> Self {
        self.required_scopes = scopes;
        self
    }

    #[must_use]
    pub const fn with_min_assurance(mut self, level: u8) -> Self {
        self.min_assurance_level = level;
        self
    }
}

/// A token that satisfied every requirement. Constructing one is the only way to prove it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedActor {
    pub subject: String,
    pub realm: Realm,
    pub audience: Audience,
    pub scopes: Vec<String>,
    pub assurance_level: u8,
    pub organization: Option<String>,
}

impl VerifiedActor {
    #[must_use]
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|held| held == scope)
    }
}

/// Why a token was refused. Callers map **every** variant to one uniform 401 with no detail:
/// telling a caller which check failed is an oracle for finding a token that passes more of them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerifyError {
    MissingBearer,
    Inactive,
    Issuer,
    Audience,
    Realm,
    MissingSubject,
    InvalidSubject,
    Expired,
    NotYetValid,
    InsufficientAssurance,
    MissingScope,
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // One string for all of them on purpose; the variant is for logs and tests, not for clients.
        f.write_str("unauthorized")
    }
}

/// Extract a bearer token from an `Authorization` header value.
///
/// Rejects an empty token and anything implausibly long (a 16 KiB cap: real tokens are far
/// smaller, and an unbounded one is free work for an attacker).
///
/// # Errors
/// [`VerifyError::MissingBearer`] when the header is absent or malformed.
pub fn bearer(header: Option<&str>) -> Result<&str, VerifyError> {
    header
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|token| !token.is_empty() && token.len() <= 16 * 1024)
        .ok_or(VerifyError::MissingBearer)
}

/// Project an introspection answer into a [`VerifiedActor`], or refuse it.
///
/// Every check is exact and every one is required. In particular the realm check is not a
/// formality: it is what stops a perfectly valid *customer* token from being accepted by the
/// admin API, which is the single failure that would matter most.
///
/// `now` is passed in rather than read from the clock so expiry is testable.
///
/// # Errors
/// Any of [`VerifyError`].
pub fn project(
    introspection: &Introspection,
    requirements: &Requirements<'_>,
    now: i64,
) -> Result<VerifiedActor, VerifyError> {
    if !introspection.active {
        return Err(VerifyError::Inactive);
    }

    let expected_issuer = normalize(requirements.issuer);
    let issuer = introspection.issuer.as_deref().map(normalize).unwrap_or("");
    if expected_issuer.is_empty() || issuer != expected_issuer {
        return Err(VerifyError::Issuer);
    }

    let expected_audience = requirements.audience.as_str();
    if !introspection
        .audiences
        .iter()
        .any(|aud| aud.trim() == expected_audience)
    {
        return Err(VerifyError::Audience);
    }

    let expected_realm = requirements.audience.realm();
    let realm = introspection.realm.as_deref().map(str::trim).unwrap_or("");
    if realm != expected_realm.as_str() {
        return Err(VerifyError::Realm);
    }

    if let Some(expires_at) = introspection.expires_at {
        if now - requirements.leeway_seconds >= expires_at {
            return Err(VerifyError::Expired);
        }
    }
    if let Some(not_before) = introspection.not_before {
        if now + requirements.leeway_seconds < not_before {
            return Err(VerifyError::NotYetValid);
        }
    }

    let assurance_level = introspection.assurance_level.unwrap_or(1);
    if assurance_level < requirements.min_assurance_level {
        return Err(VerifyError::InsufficientAssurance);
    }

    let subject = introspection
        .subject
        .as_deref()
        .ok_or(VerifyError::MissingSubject)?;
    let subject = valid_subject(subject).ok_or(VerifyError::InvalidSubject)?;

    for required in requirements.required_scopes {
        if !introspection.scopes.iter().any(|held| held == required) {
            return Err(VerifyError::MissingScope);
        }
    }

    Ok(VerifiedActor {
        subject: subject.to_owned(),
        realm: expected_realm,
        audience: requirements.audience,
        scopes: introspection.scopes.clone(),
        assurance_level,
        organization: introspection
            .organization
            .as_deref()
            .map(str::trim)
            .filter(|org| !org.is_empty())
            .map(str::to_owned),
    })
}

/// Membership of the super-admin allow-list, checked **in addition** to a verified admin token.
/// Two independent facts must hold before an admin mutation runs: the token came from the admin
/// realm, and this specific subject is on the list.
#[must_use]
pub fn is_allowlisted(actor: &VerifiedActor, allowlist: &[String]) -> bool {
    actor.realm == Realm::Admin && allowlist.iter().any(|entry| entry == &actor.subject)
}

fn normalize(value: &str) -> &str {
    value.trim().trim_end_matches('/')
}

/// Subjects are opaque identifiers; constrain them so one can never be interpolated somewhere it
/// would change meaning (a log line, a header, a cache key).
fn valid_subject(value: &str) -> Option<&str> {
    let value = value.trim();
    let ok = !value.is_empty()
        && value.len() <= 255
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.' | b'@')
        });
    ok.then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CUSTOMER: &str = "https://auth.indiebuild.dev";
    const ADMIN: &str = "https://auth-admin.indiebuild.dev";
    const NOW: i64 = 1_788_600_000;

    fn customer_token() -> Introspection {
        Introspection {
            active: true,
            issuer: Some(CUSTOMER.into()),
            audiences: vec![Audience::Api.as_str().into()],
            subject: Some("user_01H".into()),
            realm: Some("customer".into()),
            scopes: vec!["runs:read".into()],
            assurance_level: Some(1),
            organization: Some("org_acme".into()),
            expires_at: Some(NOW + 300),
            not_before: Some(NOW - 300),
        }
    }

    fn admin_token() -> Introspection {
        Introspection {
            active: true,
            issuer: Some(ADMIN.into()),
            audiences: vec![Audience::AdminApi.as_str().into()],
            subject: Some("018f2ee1-3ef5-7f4b-9e27-494ce50e8c4f".into()),
            realm: Some("admin".into()),
            scopes: vec!["tenants:write".into()],
            assurance_level: Some(2),
            organization: None,
            expires_at: Some(NOW + 300),
            not_before: None,
        }
    }

    #[test]
    fn a_valid_customer_token_projects() {
        let actor = project(
            &customer_token(),
            &Requirements::new(CUSTOMER, Audience::Api),
            NOW,
        )
        .unwrap();
        assert_eq!(actor.realm, Realm::Customer);
        assert_eq!(actor.organization.as_deref(), Some("org_acme"));
        assert!(actor.has_scope("runs:read"));
    }

    #[test]
    fn a_customer_token_is_refused_by_the_admin_api_even_with_the_right_shape() {
        // Same subject, same scopes, active, unexpired — only the realm and issuer differ.
        let mut token = customer_token();
        token.audiences = vec![Audience::AdminApi.as_str().into()];
        let err = project(&token, &Requirements::new(ADMIN, Audience::AdminApi), NOW).unwrap_err();
        assert_eq!(err, VerifyError::Issuer);

        // And if an attacker could also forge the issuer, the realm claim still stops it.
        token.issuer = Some(ADMIN.into());
        let err = project(&token, &Requirements::new(ADMIN, Audience::AdminApi), NOW).unwrap_err();
        assert_eq!(err, VerifyError::Realm);
    }

    #[test]
    fn an_admin_token_is_refused_by_the_product_api() {
        let err = project(
            &admin_token(),
            &Requirements::new(CUSTOMER, Audience::Api),
            NOW,
        )
        .unwrap_err();
        assert_eq!(err, VerifyError::Issuer);
    }

    #[test]
    fn a_token_for_another_audience_is_refused() {
        let err = project(
            &customer_token(),
            &Requirements::new(CUSTOMER, Audience::Web),
            NOW,
        )
        .unwrap_err();
        assert_eq!(err, VerifyError::Audience);
    }

    #[test]
    fn expiry_and_not_before_are_checked_with_leeway() {
        let token = customer_token();
        let requirements = Requirements::new(CUSTOMER, Audience::Api);
        assert!(project(&token, &requirements, NOW + 299).is_ok());
        assert_eq!(
            project(&token, &requirements, NOW + 400).unwrap_err(),
            VerifyError::Expired
        );
        assert_eq!(
            project(&token, &requirements, NOW - 400).unwrap_err(),
            VerifyError::NotYetValid
        );
    }

    #[test]
    fn admin_routes_can_demand_a_second_factor() {
        let mut token = admin_token();
        token.assurance_level = Some(1);
        let requirements = Requirements::new(ADMIN, Audience::AdminApi).with_min_assurance(2);
        assert_eq!(
            project(&token, &requirements, NOW).unwrap_err(),
            VerifyError::InsufficientAssurance
        );
        token.assurance_level = Some(2);
        assert!(project(&token, &requirements, NOW).is_ok());
    }

    #[test]
    fn missing_scopes_are_refused() {
        let requirements = Requirements::new(ADMIN, Audience::AdminApi)
            .with_scopes(&["tenants:write", "audit:read"]);
        assert_eq!(
            project(&admin_token(), &requirements, NOW).unwrap_err(),
            VerifyError::MissingScope
        );
    }

    #[test]
    fn inactive_and_malformed_subjects_are_refused() {
        let mut token = customer_token();
        token.active = false;
        assert_eq!(
            project(&token, &Requirements::new(CUSTOMER, Audience::Api), NOW).unwrap_err(),
            VerifyError::Inactive
        );

        let mut token = customer_token();
        token.subject = Some("user 01H\nSet-Cookie: x".into());
        assert_eq!(
            project(&token, &Requirements::new(CUSTOMER, Audience::Api), NOW).unwrap_err(),
            VerifyError::InvalidSubject
        );
    }

    #[test]
    fn the_allowlist_is_a_second_independent_gate() {
        let admin = project(
            &admin_token(),
            &Requirements::new(ADMIN, Audience::AdminApi),
            NOW,
        )
        .unwrap();
        let list = vec!["018f2ee1-3ef5-7f4b-9e27-494ce50e8c4f".to_owned()];
        assert!(is_allowlisted(&admin, &list));
        assert!(!is_allowlisted(&admin, &[]));

        // A customer actor never satisfies it, whatever the list says.
        let customer = project(
            &customer_token(),
            &Requirements::new(CUSTOMER, Audience::Api),
            NOW,
        )
        .unwrap();
        assert!(!is_allowlisted(&customer, &["user_01H".to_owned()]));
    }

    #[test]
    fn bearer_parsing_rejects_junk() {
        assert_eq!(bearer(Some("Bearer abc")).unwrap(), "abc");
        assert!(bearer(None).is_err());
        assert!(bearer(Some("Basic abc")).is_err());
        assert!(bearer(Some("Bearer ")).is_err());
        let huge = format!("Bearer {}", "a".repeat(17 * 1024));
        assert!(bearer(Some(&huge)).is_err());
    }

    #[test]
    fn every_audience_maps_to_exactly_one_realm() {
        assert_eq!(Audience::Web.realm(), Realm::Customer);
        assert_eq!(Audience::Api.realm(), Realm::Customer);
        assert_eq!(Audience::AdminWeb.realm(), Realm::Admin);
        assert_eq!(Audience::AdminApi.realm(), Realm::Admin);
    }
}
