#![forbid(unsafe_code)]

//! Query **builders**, behind the `db` feature.
//!
//! # What this module is not
//!
//! * It is **not** migrations. Schema authority is
//!   `declarative-migrations/declarative-migrations` fed by
//!   `gha-indie-worker-interfaces/generated/<slice>/sql/schema.sql`, converged by
//!   `dpm`. Services never run DDL at boot — that is a fleet contract.
//! * It is **not** the entity definitions. Those live in
//!   `gha-indie-worker-orm-core`, which is a separate repository.
//! * It does **not** connect to anything. Every function here returns a
//!   `Statement` or transforms a `Select`; executing it is the caller's job, with
//!   the caller's connection and the caller's transaction.
//!
//! # Why `db` is off by default
//!
//! `gha-indie-worker-lib-core` is client-safe: the CLI, the desktop app and the
//! WASM front end link it for the state machines, the codecs and the validator,
//! and none of them should pull in SeaORM, sqlx or a TLS stack. Servers turn the
//! feature on:
//!
//! ```toml
//! gha-indie-worker-lib-core = { git = "…", features = ["db", "read-write"] }
//! ```
//!
//! # Two shapes
//!
//! [`Page`] and [`fragments`] work on a `Select<E>` for any orm-core entity —
//! generic, so this crate compiles without knowing a single entity type. The
//! per-domain modules return parameterized `Statement`s for the reads that are
//! genuinely SQL-shaped (aggregates, window functions, `information_schema`
//! probes) and that no ORM expresses well.
//!
//! Every statement is built with `Statement::from_sql_and_values`. There is no
//! string interpolation of a caller value anywhere in this module, and the tests
//! assert that the parameter count matches the placeholders.

pub mod identity;
pub mod runs;

use sea_orm::{EntityTrait, QuerySelect, Select};

/// A bounded page. The ceiling exists so a caller cannot ask for the table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Page {
    limit: u64,
    offset: u64,
}

/// The largest page any builder in this module will produce.
pub const MAX_PAGE_SIZE: u64 = 500;
/// The page size used when a caller does not ask for one.
pub const DEFAULT_PAGE_SIZE: u64 = 50;

impl Default for Page {
    fn default() -> Self {
        Self {
            limit: DEFAULT_PAGE_SIZE,
            offset: 0,
        }
    }
}

impl Page {
    /// Clamp `limit` into `1..=MAX_PAGE_SIZE`. A caller cannot escape the bound
    /// by asking louder.
    #[must_use]
    pub fn new(limit: u64, offset: u64) -> Self {
        Self {
            limit: limit.clamp(1, MAX_PAGE_SIZE),
            offset,
        }
    }

    #[must_use]
    pub const fn limit(self) -> u64 {
        self.limit
    }

    #[must_use]
    pub const fn offset(self) -> u64 {
        self.offset
    }

    /// The next page, or `None` when `returned` was short — i.e. the end.
    #[must_use]
    pub fn next(self, returned: usize) -> Option<Self> {
        (returned as u64 >= self.limit).then(|| Self {
            limit: self.limit,
            offset: self.offset.saturating_add(self.limit),
        })
    }
}

/// Fragments over any orm-core entity.
pub mod fragments {
    use super::{EntityTrait, Page, QuerySelect, Select};

    /// Apply a bounded page to any select.
    #[must_use]
    pub fn paginate<E: EntityTrait>(query: Select<E>, page: Page) -> Select<E> {
        query.limit(page.limit()).offset(page.offset())
    }

    /// Fetch one row more than the page, so a caller can tell "there is more"
    /// apart from "that was exactly the last page" without a second count query.
    #[must_use]
    pub fn paginate_probing<E: EntityTrait>(query: Select<E>, page: Page) -> Select<E> {
        query.limit(page.limit() + 1).offset(page.offset())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_size_is_clamped_in_both_directions() {
        assert_eq!(Page::new(0, 0).limit(), 1);
        assert_eq!(Page::new(10_000, 0).limit(), MAX_PAGE_SIZE);
        assert_eq!(Page::new(25, 100).offset(), 100);
        assert_eq!(Page::default().limit(), DEFAULT_PAGE_SIZE);
    }

    #[test]
    fn paging_stops_on_a_short_page() {
        let page = Page::new(50, 0);
        assert_eq!(page.next(50).map(Page::offset), Some(50));
        assert_eq!(page.next(49), None);
        assert_eq!(page.next(0), None);
    }

    #[test]
    fn offsets_saturate_rather_than_wrap() {
        let page = Page::new(50, u64::MAX - 1);
        assert_eq!(page.next(50).map(Page::offset), Some(u64::MAX));
    }
}
