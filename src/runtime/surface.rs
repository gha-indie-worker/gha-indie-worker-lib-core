//! Host-based routing: one web server binary serves several public surfaces.
//!
//! `app.` `user.` `org.` and `m.indiebuild.dev` are the same process. Which pages a request may
//! reach — and which login flow it gets — is decided by the `Host` header, here, once, rather
//! than by scattered string comparisons in handlers.
//!
//! This is a security boundary as much as a routing one: `admin.` and `admin-api.` resolve to
//! surfaces that the *product* binary must refuse outright, so a misrouted request cannot fall
//! through to a product handler that happens to share a path.

use core::fmt;

/// A public surface of the product.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Surface {
    /// `app.` — the signed-in application shell for both individuals and org members.
    App,
    /// `user.` — B2C: individual sign-up, sign-in and personal settings.
    User,
    /// `org.` — B2B: organization sign-in, seats, invitations, billing, SSO.
    Org,
    /// `m.` — the mobile web shell: same data, small-screen chrome, no heavy islands.
    Mobile,
    /// `api.` — JSON + WebSocket.
    Api,
    /// `auth.` — the shared-auth customer realm (proxied at the edge; here for completeness).
    Auth,
    /// `admin.` — the admin console. **Never served by the product binary.**
    Admin,
    /// `admin-api.` / `api-admin.` — the admin JSON API. **Never served by the product binary.**
    AdminApi,
    /// `www.` and the apex — the marketing site (static, served by GitHub Pages at the edge).
    Marketing,
}

impl Surface {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Surface::App => "app",
            Surface::User => "user",
            Surface::Org => "org",
            Surface::Mobile => "m",
            Surface::Api => "api",
            Surface::Auth => "auth",
            Surface::Admin => "admin",
            Surface::AdminApi => "admin-api",
            Surface::Marketing => "www",
        }
    }

    /// Surfaces the product web server is allowed to render.
    #[must_use]
    pub const fn is_product_web(self) -> bool {
        matches!(
            self,
            Surface::App | Surface::User | Surface::Org | Surface::Mobile | Surface::Marketing
        )
    }

    /// Surfaces that belong to the admin plane and must 404 anywhere else.
    #[must_use]
    pub const fn is_admin(self) -> bool {
        matches!(self, Surface::Admin | Surface::AdminApi)
    }

    /// Whether an unauthenticated visitor may see this surface's landing page. `App` is the
    /// signed-in shell, so an anonymous request there is redirected to a login surface rather
    /// than rendered.
    #[must_use]
    pub const fn allows_anonymous_landing(self) -> bool {
        matches!(
            self,
            Surface::User | Surface::Org | Surface::Marketing | Surface::Mobile
        )
    }

    /// Where an anonymous request to this surface should be sent to sign in. Individuals go to
    /// `user.`, organizations to `org.`; the mobile shell keeps people on `m.`.
    #[must_use]
    pub const fn login_surface(self) -> Surface {
        match self {
            Surface::Org => Surface::Org,
            Surface::Mobile => Surface::Mobile,
            _ => Surface::User,
        }
    }
}

impl fmt::Display for Surface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Resolve a `Host` header against the org's apex domain.
///
/// Returns `None` for a host that is not ours — including a look-alike such as
/// `app.indiebuild.dev.evil.test`, which must never be treated as `app.`.
#[must_use]
pub fn resolve(host_header: &str, apex: &str) -> Option<Surface> {
    let host = host_header.trim().to_ascii_lowercase();
    // Strip an explicit port; an IPv6 literal in a Host header is bracketed, and we do not serve one.
    let host = host
        .split_once(':')
        .map_or(host.as_str(), |(h, _)| h)
        .trim_end_matches('.');
    let apex = apex.trim().to_ascii_lowercase();
    let apex = apex.trim_end_matches('.');

    if host == apex {
        return Some(Surface::Marketing);
    }
    let label = host.strip_suffix(apex)?.strip_suffix('.')?;
    if label.is_empty() || label.contains('.') {
        // Only a single label is routed: `a.b.indiebuild.dev` is not a surface.
        return None;
    }
    Some(match label {
        "app" => Surface::App,
        "user" => Surface::User,
        "org" => Surface::Org,
        "m" | "mobile" => Surface::Mobile,
        "api" => Surface::Api,
        "auth" => Surface::Auth,
        "admin" => Surface::Admin,
        // Both spellings: `admin-api` is the product-facing name, `api-admin` the fleet-canonical one.
        "admin-api" | "api-admin" => Surface::AdminApi,
        "www" => Surface::Marketing,
        _ => return None,
    })
}

/// What the product web server does with a resolved surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Disposition {
    /// Render this surface.
    Serve(Surface),
    /// Send the visitor to the right login surface (anonymous request to a signed-in shell).
    RedirectToLogin(Surface),
    /// Answer 404. Used for the admin surfaces and for hosts that are not ours: an admin host
    /// reaching the product binary is a routing bug or a probe, and either way it learns nothing.
    NotFound,
}

/// Decide what the **product web server** should do with a request.
#[must_use]
pub fn dispose(host_header: &str, apex: &str, authenticated: bool) -> Disposition {
    match resolve(host_header, apex) {
        None => Disposition::NotFound,
        Some(surface) if surface.is_admin() => Disposition::NotFound,
        Some(Surface::Api | Surface::Auth) => Disposition::NotFound,
        Some(surface) if authenticated || surface.allows_anonymous_landing() => {
            Disposition::Serve(surface)
        }
        Some(surface) => Disposition::RedirectToLogin(surface.login_surface()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const APEX: &str = "indiebuild.dev";

    #[test]
    fn every_declared_subdomain_resolves() {
        for (host, expected) in [
            ("app.indiebuild.dev", Surface::App),
            ("user.indiebuild.dev", Surface::User),
            ("org.indiebuild.dev", Surface::Org),
            ("m.indiebuild.dev", Surface::Mobile),
            ("api.indiebuild.dev", Surface::Api),
            ("auth.indiebuild.dev", Surface::Auth),
            ("admin.indiebuild.dev", Surface::Admin),
            ("admin-api.indiebuild.dev", Surface::AdminApi),
            ("api-admin.indiebuild.dev", Surface::AdminApi),
            ("www.indiebuild.dev", Surface::Marketing),
            ("indiebuild.dev", Surface::Marketing),
        ] {
            assert_eq!(resolve(host, APEX), Some(expected), "host {host}");
        }
    }

    #[test]
    fn case_port_and_trailing_dot_do_not_change_the_answer() {
        assert_eq!(resolve("APP.IndieBuild.DEV:443", APEX), Some(Surface::App));
        assert_eq!(resolve("app.indiebuild.dev.", APEX), Some(Surface::App));
        assert_eq!(resolve("  org.indiebuild.dev  ", APEX), Some(Surface::Org));
    }

    #[test]
    fn look_alike_hosts_are_not_ours() {
        for host in [
            "app.indiebuild.dev.evil.test",
            "evil-indiebuild.dev",
            "indiebuild.dev.attacker.example",
            "a.app.indiebuild.dev",
            "notahost",
            "",
        ] {
            assert_eq!(resolve(host, APEX), None, "host {host} must not resolve");
        }
    }

    #[test]
    fn the_product_binary_refuses_the_admin_surfaces() {
        for host in [
            "admin.indiebuild.dev",
            "admin-api.indiebuild.dev",
            "api-admin.indiebuild.dev",
        ] {
            assert_eq!(
                dispose(host, APEX, true),
                Disposition::NotFound,
                "host {host}"
            );
        }
    }

    #[test]
    fn anonymous_visitors_land_where_they_can_sign_in() {
        assert_eq!(
            dispose("user.indiebuild.dev", APEX, false),
            Disposition::Serve(Surface::User)
        );
        assert_eq!(
            dispose("org.indiebuild.dev", APEX, false),
            Disposition::Serve(Surface::Org)
        );
        assert_eq!(
            dispose("m.indiebuild.dev", APEX, false),
            Disposition::Serve(Surface::Mobile)
        );
        // The app shell is for signed-in people; anonymous goes to the individual login.
        assert_eq!(
            dispose("app.indiebuild.dev", APEX, false),
            Disposition::RedirectToLogin(Surface::User)
        );
        assert_eq!(
            dispose("app.indiebuild.dev", APEX, true),
            Disposition::Serve(Surface::App)
        );
    }

    #[test]
    fn an_org_visitor_is_never_bounced_to_the_individual_login() {
        assert_eq!(Surface::Org.login_surface(), Surface::Org);
    }
}
