# Evidence capture boundaries

Only explicitly reviewed paths are committed. Generated TS-release binaries,
standalone microbenchmark executables, Cargo target trees, runtime homes, full
frame captures and process snapshots are excluded.

Verifier stdout/stderr is retained as captured. Captured unittest/Cargo output
can contain trailing spaces or an empty final line. Those cached whitespace
warnings are classified as evidence formatting, not normalized away. Source
code and authored reports are separately checked with git diff --check.

Known-current environment/auth secret values were compared in memory against
the reviewed archive, B.1 and CI evidence paths; zero matches were found. Values
were never printed. This is a bounded check, not a proof against all secrets.
Sanitized requests explicitly cannot establish original wire-byte parity.

Native live calls in the B.1 lane: exactly one public OpenCode smoke request.
Model lifecycle parity uses isolated loopback fixtures, not a live provider.
The full gate uses the project's own pinned Rust 1.98.1 environment.
