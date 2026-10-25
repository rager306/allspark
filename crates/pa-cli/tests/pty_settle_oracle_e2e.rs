//! The quiescence oracle's own contract (ADR-0001): `wait_for_stable`
//! resolves only when the child has actually gone quiet — not on the first
//! read, not while the child streams — and its failure path lands the
//! artifacts. A settle primitive that cannot fail is not a check.

#![cfg(unix)]

mod support;

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use support::{open_harness_pty, PtyReader};

/// Spawn `sh -c <script>` on the pty's slave; the harness reader holds the
/// master.
fn spawn_on_pty(script: &str) -> PtyReader {
    let (master, slave) = open_harness_pty();
    let child = Command::new("sh")
        .arg("-c")
        .arg(script)
        .stdin(slave.try_clone().expect("slave stdin"))
        .stdout(slave.try_clone().expect("slave stdout"))
        .stderr(Stdio::from(slave.try_clone().expect("slave stderr")))
        .spawn()
        .expect("child spawn");
    std::mem::forget(child); // the test outlives the child or the shell exits; no reaping needed
    drop(slave);
    PtyReader::new(master)
}

/// A silent child that printed once: the settle resolves shortly after the
/// print, well under the window.
#[test]
fn settle_resolves_once_the_stream_goes_quiet() {
    let mut reader = spawn_on_pty("echo one-shot; sleep 30");
    let started = Instant::now();
    reader.wait_for_stable(300, "one-shot settle", 5_000);
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(3),
        "the settle took {elapsed:?}; the quiet window is 300ms"
    );
    assert!(
        std::str::from_utf8(&reader.output)
            .unwrap()
            .contains("one-shot"),
        "the settle must have read the child's output"
    );
}

/// A streaming child (a line every 100ms) must keep the settle from
/// resolving: the oracle panics at the window, and the panic path drops the
/// artifacts. The inverted oracle (resolve on first read) fails this test.
#[test]
fn settle_refuses_while_the_child_streams_and_dumps_artifacts() {
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let mut reader = spawn_on_pty("while true; do echo tick-$(date +%s%N); sleep 0.1; done");
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        reader.wait_for_stable(500, "streaming settle", 2_000);
    }));
    assert!(
        result.is_err(),
        "a streaming child must not settle; the oracle resolved early"
    );
    // The artifacts landed (the raw stream names a tick).
    let after = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let mut found = false;
    let root = support::artifacts_dir();
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.contains("streaming settle") {
                continue;
            }
            // The stamp suffix: parse the trailing unix ms.
            let Some(stamp) = name.rsplit('-').next().and_then(|s| s.parse::<u128>().ok()) else {
                continue;
            };
            if !(before..=after).contains(&stamp) {
                continue;
            }
            let raw = root.join(entry.file_name()).join("raw.log");
            if let Ok(text) = std::fs::read_to_string(&raw) {
                if text.contains("tick-") {
                    found = true;
                }
            }
        }
    }
    assert!(
        found,
        "the settle's failure path must dump the raw stream artifact"
    );
}
