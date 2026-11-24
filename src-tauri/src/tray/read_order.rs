//! How the readings of the tray menu's slow facts are ordered (popup_input.rs). Std only, so the rules are tested without the rest of the Panel.
//!
//! Two things go wrong when readings run beside each other, and each has one rule here:
//!   * a request that comes while a reading runs must not be lost when that reading ends: `Coalescer` is ONE atomic state, so "a request is noted" and "the reading ends" cannot pass each other;
//!   * a reading that began before a slider was written must not replace one that began after it (and must not be shown after the write): `Latest` numbers the readings when they BEGIN and keeps
//!     only the one that began last, never one that began before the last write finished. A write holds the readings for its whole length (`hold` .. `release`): nothing that ends meanwhile is kept, and
//!     the readings that began meanwhile are dropped too, because the system shows the new value only after it has settled;
//!   * a state that was composed from an older reading must not be sent after one composed from a newer reading: `Outbox` numbers the states when their composing BEGINS and sends one only if no
//!     later one was sent.

use std::sync::Mutex;
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
    /// slider writes that are under way: no reading is kept while there is one
    holds: u32,
}

impl<T> Latest<T> {
    pub(super) const fn new() -> Self {
        Latest { value: None, next: 1, floor: 0, kept: 0, generation: 0, holds: 0 }
    }

    /// A reading begins: its number. Whatever it finds is as old as this moment.
    pub(super) fn begin(&mut self) -> u64 {
        let ticket = self.next;
        self.next += 1;
        ticket
    }

    /// A reading ended with `value`: kept (true) unless it began before the last write finished or a reading that began after it is kept already (false).
    pub(super) fn offer(&mut self, ticket: u64, value: T) -> bool {
        if self.holds > 0 || ticket < self.floor || ticket <= self.kept {
            return false;
        }
        self.kept = ticket;
        self.generation += 1;
        self.value = Some((value, Instant::now()));
        true
    }

    /// Only readings that begin from now on count: every reading that is under way holds what the write replaced, or what the system has not shown yet.
    fn invalidate(&mut self) {
        self.floor = self.next;
    }

    /// A slider write begins (before the value is written): the readings that are under way are dropped, and so is everything that ends until `release`.
    pub(super) fn hold(&mut self) {
        self.holds += 1;
        self.invalidate();
    }

    /// The write ended (its effect is in place, or it failed): only readings that begin from now on are kept.
    pub(super) fn release(&mut self) {
        self.holds = self.holds.saturating_sub(1);
        self.invalidate();
    }

    /// A slider write is under way: the kept reading is not the state it will leave, so no state made from it is sent.
    pub(super) fn writing(&self) -> bool {
        self.holds > 0
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

/// The order the states are sent to the menu in. A state is composed from the kept reading after `begin`; `deliver` sends it only when no state that began later was sent, so a state made from an older
/// reading never lands after a newer one (the slider would jump back to the value it had).
pub(super) struct Outbox(Mutex<(u64, u64)>);

impl Outbox {
    pub(super) const fn new() -> Self {
        // (the number the next state gets, the number of the last state that was sent)
        Outbox(Mutex::new((1, 0)))
    }

    /// A state is about to be composed: its number.
    pub(super) fn begin(&self) -> u64 {
        let mut held = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let ticket = held.0;
        held.0 += 1;
        ticket
    }

    /// Sends the state of `ticket` with `send` (it returns whether it sent: a send that is held back changes nothing) unless a later one went out already. Sends are one at a time, in the order they get here.
    pub(super) fn deliver(&self, ticket: u64, send: impl FnOnce() -> bool) -> bool {
        let mut held = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if ticket <= held.1 {
            return false;
        }
        let sent = send();
        if sent {
            held.1 = ticket;
        }
        sent
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
    fn a_reading_that_ends_during_the_settle_time_is_not_kept_though_it_began_before_the_write() {
        let mut latest = Latest::new();
        let before = latest.begin();
        latest.hold(); // the write begins
        // the value is written, the system is settling: the reading from before ends now with the old value
        assert!(latest.writing());
        assert!(!latest.offer(before, "slider at the old value"));
        assert_eq!(latest.value(), None);
        latest.release();
        assert!(!latest.writing());
        let after = latest.begin();
        assert!(latest.offer(after, "slider at the new value"));
    }

    #[test]
    fn a_reading_that_begins_while_the_system_settles_is_dropped_too() {
        let mut latest = Latest::new();
        latest.hold();
        // it began after the value was written but before the system showed it: it may read the old one
        let during = latest.begin();
        assert!(!latest.offer(during, "settling"), "ended during the hold");
        let during_late = latest.begin();
        latest.release();
        assert!(!latest.offer(during_late, "settling"), "ended after the release, began before it");
        let after = latest.begin();
        assert!(latest.offer(after, "settled"));
        assert_eq!(latest.value(), Some("settled"));
    }

    #[test]
    fn two_writes_hold_until_both_are_released() {
        let mut latest: Latest<u8> = Latest::new();
        latest.hold();
        latest.hold();
        latest.release();
        assert!(latest.writing(), "the second write is still under way");
        let t = latest.begin();
        assert!(!latest.offer(t, 1));
        latest.release();
        assert!(!latest.writing());
        latest.release(); // a release too many does not hold the readings for ever
        assert!(!latest.writing());
        let t = latest.begin();
        assert!(latest.offer(t, 2));
    }

    #[test]
    fn a_state_composed_from_an_older_reading_is_not_sent_after_a_newer_one() {
        let outbox = Outbox::new();
        // a background push begins composing from the old reading; the write ends and its state begins composing, and is sent first
        let background = outbox.begin();
        let after_write = outbox.begin();
        let mut sent = Vec::new();
        assert!(outbox.deliver(after_write, || {
            sent.push("new");
            true
        }));
        assert!(
            !outbox.deliver(background, || {
                sent.push("old");
                true
            }),
            "the older state arrives last and is dropped"
        );
        assert_eq!(sent, ["new"]);
        // the order the other way round is the normal one: both go out
        let first = outbox.begin();
        let second = outbox.begin();
        assert!(outbox.deliver(first, || true));
        assert!(outbox.deliver(second, || true));
    }

    #[test]
    fn a_send_that_is_held_back_does_not_count_as_sent() {
        let outbox = Outbox::new();
        let held = outbox.begin();
        let later = outbox.begin();
        assert!(!outbox.deliver(later, || false), "a write is under way: nothing was sent");
        assert!(outbox.deliver(held, || true), "the earlier state is still the newest that went out");
    }

    #[test]
    fn states_sent_from_many_threads_never_go_out_in_an_older_order() {
        use std::sync::Arc;
        let outbox = Arc::new(Outbox::new());
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut threads = Vec::new();
        for _ in 0..4 {
            let (outbox, log) = (outbox.clone(), log.clone());
            threads.push(std::thread::spawn(move || {
                for _ in 0..5_000 {
                    let ticket = outbox.begin();
                    outbox.deliver(ticket, || {
                        log.lock().unwrap().push(ticket);
                        true
                    });
                }
            }));
        }
        for t in threads {
            t.join().unwrap();
        }
        let log = log.lock().unwrap();
        assert!(log.windows(2).all(|w| w[0] < w[1]), "a state went out after a newer one");
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
