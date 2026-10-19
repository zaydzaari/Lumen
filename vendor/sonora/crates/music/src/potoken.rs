//! The hand-off between the client that needs a proof-of-origin token and whatever mints it.
//!
//! YouTube will not serve a stream to a flagged address without one, and minting one takes a real
//! browser engine, which this crate has none of. So the client records the identifier it needs a
//! token for and carries on with a cold start token, the app's browser window notices the ask and
//! mints, and the next track gets the real thing. Nothing here knows what a browser is, and the
//! app side never names a provider.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long a minted token is handed out before the client asks for another. Google's integrity
/// token lasts twelve hours, so this never outlives the minter that produced it.
const KEEP: Duration = Duration::from_secs(6 * 60 * 60);
/// How long a failed mint is left alone before another is tried, and how far that grows.
///
/// A failure is worth retrying soon: the likeliest one is a page that did not finish loading, and
/// the next attempt fixes it. A browser Google will not attest fails every time instead, and the
/// doubling is what stops that from opening a window every few seconds for the life of the run.
/// Nothing waits on the hold, but a stream the token was meant for cannot finish on a cold start
/// token, so the wait is the user's music and is kept short at the start.
const SOONEST: Duration = Duration::from_secs(5);
const SLOWEST: Duration = Duration::from_secs(5 * 60);

#[derive(Default)]
struct Shared {
    /// What the client last asked for and has no token for.
    wanted: Option<String>,
    minted: HashMap<String, Minted>,
}

struct Minted {
    /// The token itself, or nothing when the last attempt failed.
    token: Option<String>,
    at: Instant,
    /// How many attempts in a row have failed. Zero once one has landed.
    failures: u32,
}

fn shared() -> &'static Mutex<Shared> {
    static SHARED: OnceLock<Mutex<Shared>> = OnceLock::new();
    SHARED.get_or_init(Mutex::default)
}

/// The token for `binding`, if one has been minted and is still fresh. Records the ask otherwise,
/// which is what [`wanted`] hands the app, and answers `None` straight away: a track must never
/// wait on a browser window.
pub fn token(binding: &str) -> Option<String> {
    let mut shared = shared().lock().ok()?;
    if let Some(minted) = shared.minted.get(binding) {
        let kept = match minted.token.is_some() {
            true => KEEP,
            false => hold(minted.failures),
        };
        if minted.at.elapsed() < kept {
            return minted.token.clone();
        }
    }
    shared.wanted = Some(binding.to_string());
    None
}

/// The identifier a client is waiting on a token for, taken rather than read: one ask leads to one
/// attempt, and a failed attempt is held off by [`failed`] rather than by asking again at once.
pub fn wanted() -> Option<String> {
    shared().lock().ok()?.wanted.take()
}

/// Files a freshly minted token. It is handed out for the next six hours, and the failures
/// behind it are forgotten, so a run that recovers starts its next hold from the beginning.
pub fn give(binding: &str, token: String) {
    let Ok(mut shared) = shared().lock() else {
        return;
    };
    shared.minted.insert(
        binding.to_string(),
        Minted {
            token: Some(token),
            at: Instant::now(),
            failures: 0,
        },
    );
}

/// Records that minting for `binding` failed. The next attempt waits [`SOONEST`], and each
/// failure after that doubles the wait up to [`SLOWEST`].
pub fn failed(binding: &str) {
    let Ok(mut shared) = shared().lock() else {
        return;
    };
    let failures = shared
        .minted
        .get(binding)
        .map_or(0, |minted| minted.failures);
    shared.minted.insert(
        binding.to_string(),
        Minted {
            token: None,
            at: Instant::now(),
            failures: failures.saturating_add(1),
        },
    );
}

/// How long to leave a binding alone after `failures` attempts in a row have failed.
fn hold(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(16);
    SOONEST.saturating_mul(1u32 << doublings).min(SLOWEST)
}
