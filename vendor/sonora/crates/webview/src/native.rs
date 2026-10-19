//! What the three native backends share: how big the window opens, how a raw source url is read,
//! and how one cookie round trip is driven. Every backend reads cookies asynchronously and answers
//! `poll` from whatever the last read left behind, so the state machine is the same in all of them
//! and only the call that starts a read differs.

use crate::Cookie;

/// How big the sign-in window opens, and how small it may be dragged. Each backend spells these in
/// its own toolkit's units.
pub(crate) const WIDTH: i32 = 520;
pub(crate) const HEIGHT: i32 = 720;
pub(crate) const MIN_WIDTH: i32 = 400;
pub(crate) const MIN_HEIGHT: i32 = 500;

/// The state of one cookie round trip.
pub(crate) enum Fetch {
    Idle,
    InFlight,
    Done(Vec<Cookie>),
}

/// What a backend should do about the read `poll` just asked for.
pub(crate) enum Reading {
    /// A read finished; this is what it found.
    Done(Vec<Cookie>),
    /// A read is already out. Wait for it.
    Waiting,
    /// Nothing is out, and the slot is now claimed. Start one, and give the slot back with
    /// `Fetch::Idle` if it cannot be started after all.
    Start,
}

impl Fetch {
    /// Answers a poll and claims the slot in the same move, so two polls cannot start two reads.
    pub(crate) fn take(&mut self) -> Reading {
        match std::mem::replace(self, Self::InFlight) {
            Self::Done(cookies) => {
                *self = Self::Idle;
                Reading::Done(cookies)
            }
            Self::InFlight => Reading::Waiting,
            Self::Idle => Reading::Start,
        }
    }
}

/// Extracts the host from the absolute HTTP(S) urls a webview reports as its source. macOS reads it
/// off `NSURL` instead and never comes here.
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub(crate) fn host(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = authority
        .strip_prefix('[')
        .and_then(|host| host.split_once(']').map(|(host, _)| host))
        .unwrap_or_else(|| authority.split(':').next().unwrap_or(authority));
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}
