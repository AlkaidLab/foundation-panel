//! How the readings of the tray menu's slow facts are ordered (popup_input.rs). Std only, so the rules are tested without the rest of the Panel.
//!
//! Two things go wrong when readings run beside each other, and each has one rule here:
//!   * a request that comes while a reading runs must not be lost when that reading ends: `Coalescer` is ONE atomic state, so "a request is noted" and "the reading ends" cannot pass each other;
//!   * a reading that began before a slider was written must not replace one that began after it (and must not be shown after the write): `Latest` numbers the readings when they BEGIN and keeps
//!     only the one that began last, never one that began before the last write finished.

use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

const IDLE: u8 = 0;
const RUNNING: u8 = 1;
/// A reading runs and one more request came in meanwhile: it reads once more when it ends.
const AGAIN: u8 = 2;

/// One reader at a time; requests that come while it runs are merged into one more round.
pub(super) struct Coalescer(AtomicU8);

impl Coalescer {
    pub(super) const fn new() -> Self {
        Coalescer(AtomicU8::new(IDLE))
    }

    /// A reading is wanted. True: nobody reads, the caller starts the reader. False: a reader runs (or is about to run another round) and takes this request with it.
    pub(super) fn request(&self) -> bool {
        let mut seen = self.0.load(Ordering::SeqCst);
        loop {
            let (next, start) = match seen {
                IDLE => (RUNNING, true),
                _ => (AGAIN, false),
            };
            match self.0.compare_exchange(seen, next, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => return start,
                Err(now) => seen = now,
            }
        }
    }

    /// The reader finished a round. True: a request came during it, the reader reads again (the state is `RUNNING` again before it does). False: the reader is released; the next request starts a new one.
    pub(super) fn round_done(&self) -> bool {
        match self.0.compare_exchange(RUNNING, IDLE, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => false,
            Err(_) => {
                self.0.store(RUNNING, Ordering::SeqCst);
                true
            }
        }
    }
}

/// The newest reading and the order the readings began in.
pub(super) struct Latest<T> {
    value: Option<(T, Instant)>,
    /// the number the next reading gets
    next: u64,
    /// readings numbered below this began before the last write finished: their answer may hold what the write replaced
    floor: u64,
    /// the number of the reading that is kept
    kept: u64,
    /// how many readings were kept
    generation: u64,
}

impl<T> Latest<T> {
    pub(super) const fn new() -> Self {
        Latest { value: None, next: 1, floor: 0, kept: 0, generation: 0 }
    }

    /// A reading begins: its number. Whatever it finds is as old as this moment.
    pub(super) fn begin(&mut self) -> u64 {
        let ticket = self.next;
        self.next += 1;
        ticket
    }

    /// A reading ended with `value`: kept (true) unless it began before the last write finished or a reading that began after it is kept already (false).
    pub(super) fn offer(&mut self, ticket: u64, value: T) -> bool {
        if ticket < self.floor || ticket <= self.kept {
            return false;
        }
        self.kept = ticket;
        self.generation += 1;
        self.value = Some((value, Instant::now()));
        true
    }

    /// A write finished and its effect is in place: only readings that begin from now on show it.
    pub(super) fn invalidate(&mut self) {
        self.floor = self.next;
    }

    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    /// How old the kept reading is; None when there is none.
    pub(super) fn age(&self) -> Option<Duration> {
        self.value.as_ref().map(|(_, at)| at.elapsed())
    }
}

impl<T: Clone> Latest<T> {
    pub(super) fn value(&self) -> Option<T> {
        self.value.as_ref().map(|(value, _)| value.clone())
    }
}

/// What a reading was made under. The target of the display sliders follows these, so a reading made under other facts shows a display that is not the target any more.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Basis {
    pub vdd_active: bool,
    /// a stream is on or paused (the session holds its display)
    pub session: bool,
}

/// The display a write is meant for is the one the menu shows: `shown` is the device of the target in the state the menu was last sent (None: it shows no display controls), `now` the device the write
/// would change. Err when they are not the same display (a write must never land on another display than the one on screen).
pub(super) fn same_target(shown: Option<&str>, now: Option<&str>) -> Result<(), ()> {
    match (shown, now) {
        (Some(a), Some(b)) if a == b => Ok(()),
        _ => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_while_the_reader_runs_is_merged_and_read_once_more() {
        let gate = Coalescer::new();
        assert!(gate.request(), "idle: the caller starts the reader");
        assert!(!gate.request(), "a reader runs: the request joins it");
        assert!(!gate.request(), "two requests are one more round");
        assert!(gate.round_done(), "a request came during the round: read again");
        assert!(!gate.round_done(), "no request since: released");
        assert!(gate.request(), "after the release the next request starts a reader");
    }

    #[test]
    fn a_request_that_comes_after_the_last_check_is_not_lost() {
        // the order the review found: the reader had checked for requests and was about to release when the request came
        let gate = Coalescer::new();
        assert!(gate.request());
        assert!(!gate.round_done(), "the reader checked: nothing asked, released");
        // the request comes now: nobody runs, so it starts a reader itself instead of leaving a note nobody reads
        assert!(gate.request(), "a request after the release starts the reader");
        // and the other order: the request is noted first, the reader finds it when it checks
        assert!(!gate.request());
        assert!(gate.round_done());
    }

    #[test]
    fn requests_and_round_ends_from_many_threads_never_run_two_readers_nor_leave_a_note_without_one() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicUsize;
        let gate = Arc::new(Coalescer::new());
        let readers = Arc::new(AtomicUsize::new(0));
        let mut threads = Vec::new();
        for _ in 0..4 {
            let (gate, readers) = (gate.clone(), readers.clone());
            threads.push(std::thread::spawn(move || {
                for _ in 0..20_000 {
                    if gate.request() {
                        // this thread is the reader: it reads until nothing is asked
                        loop {
                            assert_eq!(readers.fetch_add(1, Ordering::SeqCst), 0, "two readers at once");
                            readers.fetch_sub(1, Ordering::SeqCst);
                            if !gate.round_done() {
                                break;
                            }
                        }
                    }
                }
            }));
        }
        for t in threads {
            t.join().unwrap();
        }
        // nothing runs now: a request is noted only when a reader is there to take it, so this one must start a reader
        assert!(gate.request(), "a request was noted but no reader is there to take it");
    }

    #[test]
    fn a_reading_that_began_earlier_does_not_replace_one_that_began_later() {
        let mut latest = Latest::new();
        let early = latest.begin();
        let late = latest.begin();
        assert!(latest.offer(late, "new"));
        assert!(!latest.offer(early, "old"), "it began first and ended last");
        assert_eq!(latest.value(), Some("new"));
        assert_eq!(latest.generation(), 1);
    }

    #[test]
    fn a_reading_that_began_before_a_write_finished_is_not_kept_whenever_it_ends() {
        let mut latest = Latest::new();
        // a background reading begins and reads the slider's old value; the slider is written; the state after the write is read
        let background = latest.begin();
        latest.invalidate();
        let after_write = latest.begin();
        // the background reading ends FIRST: it must not show the old value even for a moment
        assert!(!latest.offer(background, "slider at the old value"));
        assert_eq!(latest.value(), None);
        assert!(latest.offer(after_write, "slider at the new value"));
        assert_eq!(latest.value(), Some("slider at the new value"));
    }

    #[test]
    fn the_reading_after_a_write_is_kept_even_when_an_older_one_was_kept_before_the_write() {
        let mut latest = Latest::new();
        let before = latest.begin();
        assert!(latest.offer(before, "before"));
        latest.invalidate();
        let after = latest.begin();
        assert!(latest.offer(after, "after"));
        assert_eq!(latest.value(), Some("after"));
        assert_eq!(latest.generation(), 2);
    }

    #[test]
    fn an_empty_latest_has_no_age_and_a_kept_one_has() {
        let mut latest: Latest<u8> = Latest::new();
        assert!(latest.age().is_none());
        let t = latest.begin();
        assert!(latest.offer(t, 1));
        assert!(latest.age().is_some());
    }

    #[test]
    fn a_write_is_for_the_display_the_menu_shows() {
        assert!(same_target(Some(r"\\.\DISPLAY2"), Some(r"\\.\DISPLAY2")).is_ok());
        assert!(same_target(Some(r"\\.\DISPLAY2"), Some(r"\\.\DISPLAY1")).is_err(), "the second display's stream ended: the target is the primary one now");
        assert!(same_target(None, Some(r"\\.\DISPLAY1")).is_err(), "the menu shows no display controls");
        assert!(same_target(Some(r"\\.\DISPLAY2"), None).is_err(), "the target is gone");
    }
}
