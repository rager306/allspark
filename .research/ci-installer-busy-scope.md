# Installer fixture Text-file-busy scope checkpoint

The approved native `make check` (Rust1.98.1, DEBUG0/JOBS2) failed before
`prime-agent update --rollback` could run. `Command::output()` could not execute
the freshly copied temporary payload: OS26 `ExecutableFileBusy`. The binary
finished8 passed/1 failed in3.75s; fmt and all-target workspace Clippy passed.
Safe failure receipt: `ci-fullgate-1.98.1-installer-busy-receipt.json`.

The sandbox path is per-test unique. The installer/update production path did
not execute, so no production installer defect is inferred. A concurrent
fork may inherit a transient writable descriptor held during `std::fs::copy`;
this is under diagnosis, not a demonstrated historical scheduling cause yet.

No source patch, exec retry, fixed sleep, timeout increase, assertion removal,
production unsafe, dependency change, cache cleanup or push is authorized by
this checkpoint. Root has not started another full gate while this exact
failure is being investigated. A fix must own the fixture publication/FD
boundary and retain the real rollback assertions.
