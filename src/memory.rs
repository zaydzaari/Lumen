//! Working-set trimming.
//!
//! On Windows the graphics driver (OpenGL ICD) keeps tens of MB of pages in the
//! process working set that are only touched at start-up. Like Discord and other
//! desktop apps, Lumen asks Windows to trim the working set at quiet moments:
//! shortly after start-up, when the window is minimised or hidden to the tray, and
//! after a minute without input. Pages that are still used are faulted back in
//! from RAM immediately, so this costs nothing visible. Other platforms: no-op.

use std::time::{Duration, Instant};

const AFTER_START: Duration = Duration::from_secs(10);
const AFTER_IDLE: Duration = Duration::from_secs(60);
const MIN_GAP: Duration = Duration::from_secs(30);

/// Trim the process working set. Returns `true` when the OS accepted it.
pub fn trim() -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::ProcessStatus::K32EmptyWorkingSet;
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        // SAFETY: GetCurrentProcess returns a pseudo-handle that is always valid
        // for the calling process; K32EmptyWorkingSet only reads it.
        unsafe { K32EmptyWorkingSet(GetCurrentProcess()) != 0 }
    }
    #[cfg(not(windows))]
    {
        release_heap();
        false
    }
}

/// Return freed heap memory to the OS. glibc keeps freed blocks in its arenas
/// (so a burst of image decoding shows up as RSS for good); `malloc_trim`
/// releases them. Windows' heap already returns large blocks, so this is a no-op
/// there and on other platforms.
pub fn release_heap() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        extern "C" {
            fn malloc_trim(pad: usize) -> i32;
        }
        // SAFETY: malloc_trim only walks glibc's own allocator state.
        unsafe {
            malloc_trim(0);
        }
    }
}

/// Decides when to trim. Pure logic, so it is unit-tested on every platform.
pub struct Trimmer {
    started: Instant,
    last_input: Instant,
    last_trim: Option<Instant>,
    trimmed_for_idle: bool,
    trimmed_for_hidden: bool,
    did_startup: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Trim,
    /// Check again after this long (to schedule a single wake-up).
    Wait(Duration),
    Nothing,
}

impl Trimmer {
    pub fn new(now: Instant) -> Self {
        Self {
            started: now,
            last_input: now,
            last_trim: None,
            trimmed_for_idle: false,
            trimmed_for_hidden: false,
            did_startup: false,
        }
    }

    pub fn tick(&mut self, now: Instant, had_input: bool, hidden: bool) -> Decision {
        if had_input {
            self.last_input = now;
            self.trimmed_for_idle = false;
        }
        if !hidden {
            self.trimmed_for_hidden = false;
        }
        let gap_ok = self
            .last_trim
            .is_none_or(|t| now.duration_since(t) >= MIN_GAP);
        let mut want = false;
        if !self.did_startup && now.duration_since(self.started) >= AFTER_START {
            self.did_startup = true;
            want = true;
        }
        if hidden && !self.trimmed_for_hidden {
            self.trimmed_for_hidden = true;
            want = true;
        }
        if !self.trimmed_for_idle && now.duration_since(self.last_input) >= AFTER_IDLE {
            self.trimmed_for_idle = true;
            want = true;
        }
        if want && (gap_ok || hidden) {
            self.last_trim = Some(now);
            return Decision::Trim;
        }
        // Next interesting moment.
        let mut next: Option<Duration> = None;
        let mut consider = |d: Duration| next = Some(next.map_or(d, |n| n.min(d)));
        if !self.did_startup {
            consider((self.started + AFTER_START).saturating_duration_since(now));
        }
        if !self.trimmed_for_idle {
            consider((self.last_input + AFTER_IDLE).saturating_duration_since(now));
        }
        match next {
            Some(d) => Decision::Wait(d.max(Duration::from_millis(250))),
            None => Decision::Nothing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_after_start_then_idle_then_hidden() {
        let t0 = Instant::now();
        let mut t = Trimmer::new(t0);
        assert!(matches!(t.tick(t0, false, false), Decision::Wait(_)));
        assert_eq!(
            t.tick(t0 + Duration::from_secs(11), false, false),
            Decision::Trim
        );
        // Input keeps it awake.
        assert!(matches!(
            t.tick(t0 + Duration::from_secs(20), true, false),
            Decision::Wait(_)
        ));
        assert_eq!(
            t.tick(t0 + Duration::from_secs(81), false, false),
            Decision::Trim
        );
        // Only once per idle period.
        assert_eq!(
            t.tick(t0 + Duration::from_secs(200), false, false),
            Decision::Nothing
        );
        // Hiding the window trims immediately.
        assert_eq!(
            t.tick(t0 + Duration::from_secs(201), false, true),
            Decision::Trim
        );
        assert_eq!(
            t.tick(t0 + Duration::from_secs(202), false, true),
            Decision::Nothing
        );
    }

    #[test]
    fn respects_minimum_gap_while_visible() {
        let t0 = Instant::now();
        let mut t = Trimmer::new(t0);
        assert_eq!(
            t.tick(t0 + Duration::from_secs(10), false, false),
            Decision::Trim
        );
        // Idle trigger arrives later but the start-up trim was recent → defer.
        let mut t2 = Trimmer::new(t0);
        t2.last_trim = Some(t0 + Duration::from_secs(59));
        t2.did_startup = true;
        assert!(matches!(
            t2.tick(t0 + Duration::from_secs(61), false, false),
            Decision::Wait(_) | Decision::Nothing
        ));
    }
}
