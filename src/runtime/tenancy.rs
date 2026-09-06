//! Organizations, individuals, seats and invitations — the model behind the two login surfaces.
//!
//! Two ways in, one product:
//!
//! * **B2C** (`user.indiebuild.dev`) — a person signs up for themselves. They get a *personal*
//!   workspace they own outright. No organization, no seat accounting, no admin.
//! * **B2B** (`org.indiebuild.dev`) — an organization signs up, claims a domain, buys seats, and
//!   invites employees. Members join by invitation or by verified domain, hold a role, and
//!   consume a seat.
//!
//! A person can be both: the same identity may own a personal workspace *and* hold a seat in one
//! or more organizations. That is why membership is a separate value from identity, and why the
//! onboarding state machine below is written as explicit transitions rather than a `status`
//! string that any handler can overwrite.

use core::fmt;

/// How an account came to exist. Kept because the two paths bill, support and expire differently.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum AccountKind {
    /// Signed up at `user.` for themselves.
    Individual,
    /// Belongs to an organization that signed up at `org.`.
    Organization,
}

/// What a member may do inside an organization. Ordered: every role implies the ones below it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord, Hash)]
pub enum Role {
    /// Read runs and logs. The default for a newly invited member.
    Viewer,
    /// Trigger and cancel runs, manage runners.
    Member,
    /// Manage seats, invitations, runner groups and policies.
    Admin,
    /// Everything, plus billing and deleting the organization. Exactly one is required at all times.
    Owner,
}

impl Role {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Member => "member",
            Role::Admin => "admin",
            Role::Owner => "owner",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value.trim().to_ascii_lowercase().as_str() {
            "viewer" => Role::Viewer,
            "member" => Role::Member,
            "admin" => Role::Admin,
            "owner" => Role::Owner,
            _ => return None,
        })
    }

    /// Roles are a total order, so "may act as" is a comparison rather than a match table.
    #[must_use]
    pub fn at_least(self, required: Role) -> bool {
        self >= required
    }

    #[must_use]
    pub const fn consumes_seat(self) -> bool {
        // A viewer still occupies a seat: read access to a private build log is worth paying for,
        // and "free viewers" is the loophole every seat-based plan gets abused through.
        true
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A seat ledger. Deliberately a value type: it is computed from memberships and invitations,
/// never stored as a counter that can drift away from the rows it summarizes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Seats {
    pub purchased: u32,
    pub occupied: u32,
    /// Invitations that are sent and still open. They hold a seat so an org cannot over-invite.
    pub reserved: u32,
}

impl Seats {
    #[must_use]
    pub const fn available(self) -> u32 {
        self.purchased
            .saturating_sub(self.occupied.saturating_add(self.reserved))
    }

    #[must_use]
    pub const fn has_room(self) -> bool {
        self.available() > 0
    }
}

/// Why an onboarding step was refused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OnboardingError {
    /// Every seat is taken; the org must buy more before inviting.
    NoSeatsAvailable {
        purchased: u32,
        occupied: u32,
        reserved: u32,
    },
    /// The email is not in a domain this organization has verified, and the org requires it.
    DomainNotAllowed { domain: String },
    /// An invitation was already accepted, revoked, or has expired.
    InvitationNotOpen,
    /// The invitation was minted for a different address.
    InvitationAddressMismatch,
    /// The last owner cannot be demoted or removed; an org without an owner is unadministrable.
    LastOwner,
    /// A member cannot grant a role above their own.
    RoleEscalation { actor: Role, granted: Role },
    /// The address is not shaped like an email we can send to.
    InvalidEmail,
}

impl fmt::Display for OnboardingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OnboardingError::NoSeatsAvailable { purchased, occupied, reserved } => write!(
                f,
                "no seats available: {purchased} purchased, {occupied} occupied, {reserved} reserved by open invitations"
            ),
            OnboardingError::DomainNotAllowed { domain } => {
                write!(f, "{domain} is not a verified domain for this organization")
            }
            OnboardingError::InvitationNotOpen => f.write_str("this invitation is no longer open"),
            OnboardingError::InvitationAddressMismatch => {
                f.write_str("this invitation was issued to a different address")
            }
            OnboardingError::LastOwner => {
                f.write_str("an organization must always have at least one owner")
            }
            OnboardingError::RoleEscalation { actor, granted } => {
                write!(f, "a {actor} cannot grant the {granted} role")
            }
            OnboardingError::InvalidEmail => f.write_str("that does not look like an email address"),
        }
    }
}

/// The life of an invitation. Transitions are the only way to move between states.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum InvitationState {
    Open,
    Accepted,
    Revoked,
    Expired,
}

/// An invitation as the domain sees it (identifiers and timestamps are the caller's concern).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Invitation {
    pub email: String,
    pub role: Role,
    pub state: InvitationState,
    /// Seconds since the epoch.
    pub expires_at: i64,
}

impl Invitation {
    #[must_use]
    pub fn is_open(&self, now: i64) -> bool {
        self.state == InvitationState::Open && now < self.expires_at
    }
}

/// An organization's joining policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JoinPolicy {
    /// Domains the org has proven it controls (DNS TXT or a verified admin mailbox).
    pub verified_domains: Vec<String>,
    /// When true, anyone with an email in a verified domain may join without an invitation.
    /// Convenient, and exactly how a stale employee account keeps access, so it is off by default.
    pub domain_join_enabled: bool,
    /// When true, an invitation is refused unless the address is in a verified domain.
    pub restrict_invitations_to_verified_domains: bool,
    /// Role granted to someone who joins via a verified domain.
    pub default_role: Role,
}

impl Default for JoinPolicy {
    fn default() -> Self {
        Self {
            verified_domains: Vec::new(),
            domain_join_enabled: false,
            restrict_invitations_to_verified_domains: false,
            default_role: Role::Viewer,
        }
    }
}

/// Normalize and validate an email, returning `(normalized, domain)`.
///
/// # Errors
/// [`OnboardingError::InvalidEmail`] when it is not shaped like an address we could deliver to.
pub fn parse_email(raw: &str) -> Result<(String, String), OnboardingError> {
    let value = raw.trim().to_ascii_lowercase();
    let (local, domain) = value.split_once('@').ok_or(OnboardingError::InvalidEmail)?;
    let plausible = !local.is_empty()
        && local.len() <= 64
        && !domain.is_empty()
        && domain.len() <= 255
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains("..")
        && domain
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
        && !value.chars().any(char::is_whitespace);
    if !plausible {
        return Err(OnboardingError::InvalidEmail);
    }
    Ok((value.clone(), domain.to_owned()))
}

/// May `actor` invite `email` as `role` right now?
///
/// # Errors
/// Any of [`OnboardingError`].
pub fn authorize_invitation(
    actor: Role,
    granted: Role,
    email: &str,
    seats: Seats,
    policy: &JoinPolicy,
) -> Result<String, OnboardingError> {
    if !actor.at_least(Role::Admin) {
        return Err(OnboardingError::RoleEscalation { actor, granted });
    }
    if granted > actor {
        return Err(OnboardingError::RoleEscalation { actor, granted });
    }
    let (normalized, domain) = parse_email(email)?;
    if policy.restrict_invitations_to_verified_domains
        && !policy
            .verified_domains
            .iter()
            .any(|d| d.eq_ignore_ascii_case(&domain))
    {
        return Err(OnboardingError::DomainNotAllowed { domain });
    }
    if granted.consumes_seat() && !seats.has_room() {
        return Err(OnboardingError::NoSeatsAvailable {
            purchased: seats.purchased,
            occupied: seats.occupied,
            reserved: seats.reserved,
        });
    }
    Ok(normalized)
}

/// Accept an invitation. Returns the role the new member holds.
///
/// # Errors
/// [`OnboardingError::InvitationNotOpen`], [`OnboardingError::InvitationAddressMismatch`],
/// [`OnboardingError::NoSeatsAvailable`].
pub fn accept_invitation(
    invitation: &Invitation,
    accepting_email: &str,
    seats: Seats,
    now: i64,
) -> Result<Role, OnboardingError> {
    if !invitation.is_open(now) {
        return Err(OnboardingError::InvitationNotOpen);
    }
    let (normalized, _) = parse_email(accepting_email)?;
    if normalized != invitation.email.trim().to_ascii_lowercase() {
        return Err(OnboardingError::InvitationAddressMismatch);
    }
    // The invitation already reserved a seat, so accepting converts reserved -> occupied and
    // needs no *additional* room. Only refuse when the ledger says even the reservation is gone,
    // which means seats were removed after the invitation went out.
    if seats.occupied >= seats.purchased && seats.reserved == 0 {
        return Err(OnboardingError::NoSeatsAvailable {
            purchased: seats.purchased,
            occupied: seats.occupied,
            reserved: seats.reserved,
        });
    }
    Ok(invitation.role)
}

/// May this address join `organization` without an invitation?
///
/// # Errors
/// [`OnboardingError::DomainNotAllowed`], [`OnboardingError::NoSeatsAvailable`].
pub fn authorize_domain_join(
    email: &str,
    seats: Seats,
    policy: &JoinPolicy,
) -> Result<Role, OnboardingError> {
    let (_, domain) = parse_email(email)?;
    let allowed = policy.domain_join_enabled
        && policy
            .verified_domains
            .iter()
            .any(|d| d.eq_ignore_ascii_case(&domain));
    if !allowed {
        return Err(OnboardingError::DomainNotAllowed { domain });
    }
    if !seats.has_room() {
        return Err(OnboardingError::NoSeatsAvailable {
            purchased: seats.purchased,
            occupied: seats.occupied,
            reserved: seats.reserved,
        });
    }
    Ok(policy.default_role)
}

/// Change a member's role, refusing the two changes that break an organization: escalating past
/// your own role, and removing the last owner.
///
/// # Errors
/// [`OnboardingError::RoleEscalation`], [`OnboardingError::LastOwner`].
pub fn authorize_role_change(
    actor: Role,
    current: Role,
    next: Role,
    owner_count: u32,
) -> Result<(), OnboardingError> {
    if !actor.at_least(Role::Admin) || next > actor || current > actor {
        return Err(OnboardingError::RoleEscalation {
            actor,
            granted: next,
        });
    }
    if current == Role::Owner && next != Role::Owner && owner_count <= 1 {
        return Err(OnboardingError::LastOwner);
    }
    Ok(())
}

/// Remove a member.
///
/// # Errors
/// [`OnboardingError::RoleEscalation`], [`OnboardingError::LastOwner`].
pub fn authorize_removal(
    actor: Role,
    target: Role,
    owner_count: u32,
) -> Result<(), OnboardingError> {
    if !actor.at_least(Role::Admin) || target > actor {
        return Err(OnboardingError::RoleEscalation {
            actor,
            granted: target,
        });
    }
    if target == Role::Owner && owner_count <= 1 {
        return Err(OnboardingError::LastOwner);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_788_600_000;

    fn seats(purchased: u32, occupied: u32, reserved: u32) -> Seats {
        Seats {
            purchased,
            occupied,
            reserved,
        }
    }

    fn policy() -> JoinPolicy {
        JoinPolicy {
            verified_domains: vec!["acme.test".into()],
            ..JoinPolicy::default()
        }
    }

    #[test]
    fn roles_are_ordered_and_parse_round_trip() {
        assert!(
            Role::Owner > Role::Admin && Role::Admin > Role::Member && Role::Member > Role::Viewer
        );
        for role in [Role::Viewer, Role::Member, Role::Admin, Role::Owner] {
            assert_eq!(Role::parse(role.as_str()), Some(role));
            assert_eq!(Role::parse(&role.as_str().to_uppercase()), Some(role));
        }
        assert_eq!(Role::parse("superuser"), None);
    }

    #[test]
    fn seat_math_never_underflows() {
        assert_eq!(seats(5, 2, 1).available(), 2);
        assert_eq!(seats(1, 4, 4).available(), 0);
        assert!(!seats(1, 1, 0).has_room());
    }

    #[test]
    fn an_admin_cannot_invite_an_owner() {
        let err = authorize_invitation(
            Role::Admin,
            Role::Owner,
            "a@acme.test",
            seats(10, 1, 0),
            &policy(),
        )
        .unwrap_err();
        assert_eq!(
            err,
            OnboardingError::RoleEscalation {
                actor: Role::Admin,
                granted: Role::Owner
            }
        );
    }

    #[test]
    fn a_member_cannot_invite_at_all() {
        for actor in [Role::Viewer, Role::Member] {
            assert!(matches!(
                authorize_invitation(
                    actor,
                    Role::Viewer,
                    "a@acme.test",
                    seats(10, 0, 0),
                    &policy()
                ),
                Err(OnboardingError::RoleEscalation { .. })
            ));
        }
    }

    #[test]
    fn invitations_reserve_a_seat_so_an_org_cannot_over_invite() {
        let full = seats(3, 1, 2);
        let err = authorize_invitation(Role::Owner, Role::Member, "d@acme.test", full, &policy())
            .unwrap_err();
        assert_eq!(
            err,
            OnboardingError::NoSeatsAvailable {
                purchased: 3,
                occupied: 1,
                reserved: 2
            }
        );
    }

    #[test]
    fn domain_restriction_is_enforced_when_the_org_asks_for_it() {
        let mut p = policy();
        p.restrict_invitations_to_verified_domains = true;
        let err = authorize_invitation(
            Role::Owner,
            Role::Member,
            "x@other.test",
            seats(9, 0, 0),
            &p,
        )
        .unwrap_err();
        assert_eq!(
            err,
            OnboardingError::DomainNotAllowed {
                domain: "other.test".into()
            }
        );
        assert!(
            authorize_invitation(Role::Owner, Role::Member, "X@ACME.test", seats(9, 0, 0), &p)
                .is_ok()
        );
    }

    #[test]
    fn emails_are_normalized_and_junk_is_refused() {
        assert_eq!(
            parse_email("  Alex@ACME.Test ").unwrap(),
            ("alex@acme.test".into(), "acme.test".into())
        );
        for bad in [
            "",
            "no-at-sign",
            "a@b",
            "a@.b.test",
            "a@b..test",
            "a b@acme.test",
            "@acme.test",
        ] {
            assert_eq!(
                parse_email(bad).unwrap_err(),
                OnboardingError::InvalidEmail,
                "input {bad:?}"
            );
        }
    }

    #[test]
    fn accepting_an_invitation_checks_state_expiry_and_address() {
        let invitation = Invitation {
            email: "a@acme.test".into(),
            role: Role::Member,
            state: InvitationState::Open,
            expires_at: NOW + 3600,
        };
        assert_eq!(
            accept_invitation(&invitation, "A@Acme.Test", seats(5, 1, 1), NOW).unwrap(),
            Role::Member
        );
        assert_eq!(
            accept_invitation(&invitation, "someone-else@acme.test", seats(5, 1, 1), NOW)
                .unwrap_err(),
            OnboardingError::InvitationAddressMismatch
        );
        assert_eq!(
            accept_invitation(&invitation, "a@acme.test", seats(5, 1, 1), NOW + 7200).unwrap_err(),
            OnboardingError::InvitationNotOpen
        );
        let revoked = Invitation {
            state: InvitationState::Revoked,
            ..invitation
        };
        assert_eq!(
            accept_invitation(&revoked, "a@acme.test", seats(5, 1, 1), NOW).unwrap_err(),
            OnboardingError::InvitationNotOpen
        );
    }

    #[test]
    fn domain_join_is_off_until_the_org_turns_it_on() {
        let mut p = policy();
        assert!(matches!(
            authorize_domain_join("a@acme.test", seats(5, 0, 0), &p),
            Err(OnboardingError::DomainNotAllowed { .. })
        ));
        p.domain_join_enabled = true;
        assert_eq!(
            authorize_domain_join("a@acme.test", seats(5, 0, 0), &p).unwrap(),
            Role::Viewer
        );
        assert!(matches!(
            authorize_domain_join("a@other.test", seats(5, 0, 0), &p),
            Err(OnboardingError::DomainNotAllowed { .. })
        ));
    }

    #[test]
    fn the_last_owner_cannot_be_demoted_or_removed() {
        assert_eq!(
            authorize_role_change(Role::Owner, Role::Owner, Role::Admin, 1).unwrap_err(),
            OnboardingError::LastOwner
        );
        assert!(authorize_role_change(Role::Owner, Role::Owner, Role::Admin, 2).is_ok());
        assert_eq!(
            authorize_removal(Role::Owner, Role::Owner, 1).unwrap_err(),
            OnboardingError::LastOwner
        );
        assert!(authorize_removal(Role::Owner, Role::Owner, 3).is_ok());
    }

    #[test]
    fn an_admin_cannot_touch_an_owner() {
        assert!(matches!(
            authorize_role_change(Role::Admin, Role::Owner, Role::Viewer, 5),
            Err(OnboardingError::RoleEscalation { .. })
        ));
        assert!(matches!(
            authorize_removal(Role::Admin, Role::Owner, 5),
            Err(OnboardingError::RoleEscalation { .. })
        ));
    }
}
