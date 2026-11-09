//! OSC 9;4 terminal progress: the tab bar's spinner while a turn is running.
//!
//! `terminal.showTerminalProgress` (default off) was persisted, displayed in the
//! settings menu, and read by nothing (review #42): enabling it changed no byte the
//! terminal ever saw. This module is the consumer. It follows the TS contract
//! (`packages/tui/src/terminal.ts`): `ESC ] 9 ; 4 ; 3 BEL` for indeterminate
//! progress and `ESC ] 9 ; 4 ; 0 ; BEL` to clear, written once per TRANSITION —
//! the run loop calls [`sync`] with the same state on every iteration, and a repeat
//! writes nothing, because a per-frame emit would flood the terminal.

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};

/// The turn started: an indeterminate progress indicator.
pub(crate) const ACTIVE_SEQUENCE: &[u8] = b"\x1b]9;4;3\x07";
/// The turn ended, or the surface is leaving: clear the indicator.
pub(crate) const CLEAR_SEQUENCE: &[u8] = b"\x1b]9;4;0;\x07";

static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Mark the turn's state on the terminal, gated by the
/// `terminal.showTerminalProgress` setting and debounced to transitions.
///
/// `enabled == false` also CLEARS an indicator this process started, so turning the
/// setting off mid-turn cannot leave a spinner spinning in the tab bar.
pub(crate) fn sync(enabled: bool, active: bool) {
    let terminal = std::io::stdout().is_terminal();
    let mut out = std::io::stdout();
    let _ = sync_to(&mut out, terminal, enabled, active);
}

/// Clear the indicator unconditionally (surface teardown and suspend): the shell
/// prompt that follows must not carry the turn's spinner.
pub(crate) fn release() {
    let terminal = std::io::stdout().is_terminal();
    let mut out = std::io::stdout();
    let _ = sync_to(&mut out, terminal, false, false);
}

/// The byte-level contract, against any writer: what the terminal receives for one
/// (setting, turn state) pair.
pub(crate) fn sync_to(
    out: &mut impl Write,
    terminal: bool,
    enabled: bool,
    active: bool,
) -> std::io::Result<()> {
    let wanted = enabled && active;
    if wanted == ACTIVE.swap(wanted, Ordering::SeqCst) {
        return Ok(());
    }
    if !terminal {
        return Ok(());
    }
    let sequence = if wanted {
        ACTIVE_SEQUENCE
    } else {
        CLEAR_SEQUENCE
    };
    out.write_all(sequence)?;
    out.flush()
}

#[cfg(test)]
pub(crate) static STATE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
fn active() -> bool {
    ACTIVE.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(terminal: bool, enabled: bool, active: bool) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        sync_to(&mut out, terminal, enabled, active).expect("the write accepts");
        out
    }

    #[test]
    fn the_setting_decides_whether_anything_is_written() {
        let _lock = match STATE_TEST_LOCK.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        ACTIVE.store(false, Ordering::SeqCst);
        // The same running turn: off writes nothing, on writes the indicator. This is
        // the difference the setting exists for (review #42).
        assert_eq!(run(true, false, true), Vec::<u8>::new());
        assert!(!active());
        assert_eq!(run(true, true, true), ACTIVE_SEQUENCE);
        assert!(active());
        // The turn ends: the clear sequence, once.
        assert_eq!(run(true, true, false), CLEAR_SEQUENCE);
        assert!(!active());
        // A repeated state writes nothing (the run loop calls this every iteration).
        assert_eq!(run(true, true, false), Vec::<u8>::new());
        assert_eq!(run(true, false, false), Vec::<u8>::new());
    }

    #[test]
    fn a_non_terminal_stdout_keeps_the_state_without_writing() {
        let _lock = match STATE_TEST_LOCK.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        ACTIVE.store(false, Ordering::SeqCst);
        assert_eq!(run(false, true, true), Vec::<u8>::new());
        assert!(
            active(),
            "the state still tracks, so the debounce stays correct"
        );
        assert_eq!(run(false, true, false), Vec::<u8>::new());
        assert!(!active());
    }

    #[test]
    fn turning_the_setting_off_mid_turn_clears_the_indicator() {
        let _lock = match STATE_TEST_LOCK.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        ACTIVE.store(false, Ordering::SeqCst);
        assert_eq!(run(true, true, true), ACTIVE_SEQUENCE);
        // The user toggles the row off while the turn runs: the spinner must stop.
        assert_eq!(run(true, false, true), CLEAR_SEQUENCE);
        assert!(!active());
    }
}
