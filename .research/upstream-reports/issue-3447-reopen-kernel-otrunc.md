# Issue #3447 (reopen): Kernel snapshot restore still O_TRUNCs named `'w'` file handles — the int-fd fix from #3346 never covered them

**Status:** draft / not submitted. **Type:** bug (data loss). **Severity:** P1. **Area:** `prime-agent-runtime/src/rlm/repl.py` (shared with upstream — byte-identical to `main`).

## Summary

`_restore_state` unpickles every per-record snapshot blob through a `GuardedUnpickler` whose only guard refuses a raw **integer** fd (`#3346`). A **named** handle opened in a write mode falls straight through to `dill._dill._create_filehandle`, which revives it with `open(name, mode)` — and `open(path, "w")` truncates the file **by the act of opening it**. Restore then reports success, so a session resume silently empties the user's file. `#3447` reported exactly this; it was auto-closed by the vouch bot with no fix (last touch to `repl.py` is `#3346`).

## Reproduction

```python
import os, dill
d = "/tmp/data.txt"
open(d, "w").write("x" * 2300)          # 2300 bytes on disk

# kernel cell:
handle = open(d, "w")                    # a live, named write handle
handle.write("y" * 2300); handle.flush()

# snapshot (kernel serializes every user name, incl. the open handle)
# -> then restore in a fresh kernel via _restore_state / GuardedUnpickler
```

Empirically reproduced with the exact `GuardedUnpickler` copy, `dill==0.4.1`, CPython 3.11: the file goes **2300 → 0 bytes**, and the restore result reports no `failed` entry. (`'a'` does not truncate but re-arms a live append cursor; `'x'`/`'+'` are write modes too.)

Minimal mechanism, `dill/_dill.py:890`: `f = open(name, mode)` for write modes.

## Evidence (both eras)

| Era | File:line | Code |
|---|---|---|
| TS `v0.9.8` | `prime-agent-runtime/src/rlm/repl.py:996` | `staged[name] = dill.loads(blob)` — **no guard at all**; the snapshot serializes every user name |
| Rust `v0.10.x` / `main` | `prime-agent-runtime/src/rlm/repl.py:1080-1085` | `def refuse_raw_fd(name, *args): if isinstance(name, int): raise …; return create_filehandle(name, *args)` — only int-fd refused |
| Rust | `repl.py:834` (snapshot) | `if name.startswith("_") or name in _ALWAYS_SKIP: continue` — no `io.IOBase` skip, so an open handle is captured |
| Rust | `repl.py:59` | `_ALWAYS_SKIP` = `{rlm, mcp, bash, asyncio, In, Out, get_ipython, exit, quit, open}` — harness names only |

Provenance: the int-fd refusal landed in `#3346` (672085e88, 2026-10-06). `#3082` (2026-09-29, the day of the tag) is the TS-era original. `#3447` (2026-10-09) shows the named-handle hole and was closed **NOT_PLANNED** by the vouch bot. No upstream fix exists after `#3346`; `main` still has the hole.

## Proposed fix

Restore-side: refuse any write-mode reopen and bind an inert stand-in instead, so the variable survives (with its original `name`/`mode`) but every later I/O raises like a closed file. Save-side: never pick file handles at all. The reporter of `#3447` proposed the `/dev/null` reopen; the patch below goes further and keeps the binding inert across further snapshot cycles.

Key fragment (`repl.py`):

```python
_WRITE_MODE_FLAGS = frozenset("wax+")
_VIRTUAL_FILE_NAMES = frozenset({"<stdin>", "<stdout>", "<stderr>", "<tmpfile>", "<fdopen>"})

class _NeutralizedFileHandle:
    closed = True
    def __init__(self, name, mode): self.name, self.mode = name, mode
    def __reduce__(self): return (_NeutralizedFileHandle, (self.name, self.mode))  # sticky across re-snapshot
    def _closed_io(self, *a, **k): raise ValueError("I/O operation on closed file.")
    read = readline = write = flush = truncate = seek = tell = fileno = _closed_io  # …full I/O surface

# inside GuardedUnpickler.find_class, replacing the int-only guard:
def guarded_reopen(file_name, *args):
    if isinstance(file_name, int):
        raise pickle.UnpicklingError(f"refusing to reopen raw file descriptor {file_name} from a snapshot")
    mode = args[0] if args else ""
    if isinstance(mode, str) and _WRITE_MODE_FLAGS.intersection(mode) and file_name not in _VIRTUAL_FILE_NAMES:
        self.neutralized.append({"file": str(file_name)[:200], "mode": mode})
        return _NeutralizedFileHandle(file_name, mode)
    return create_filehandle(file_name, *args)   # read-only reopens live, position preserved
```

`_restore_state` gains a `neutralized` sink (per-record, so the entry carries the variable name) and returns it alongside `restored`/`failed`. Test: `test_restore_neutralizes_named_write_handles` + `…_append_handles` in `prime-agent-runtime/test/test_repl.py` (nested-in-container handle, byte-intact assertion, closed-file semantics, re-snapshot cycle). See the diff in the working tree.

> Alternative rejected in `#3447`: `dill.CONTENTS_FMODE` (fmode=1) at dump — recreates deleted files and leaves live write handles on user paths.

## Impact

- **Irreversible silent data loss** on any session resume/compaction/restart where the agent held an `open(..., "w"/"a"/"x"/"+")` handle: the file is truncated (or a live append cursor is re-aimed at it) while restore reports success.
- Affects both eras identically: the TS-era code had no guard, and the Rust-era int-fd guard does not cover named handles.
- Blast radius is the whole shared Python runtime (`prime-agent-runtime`), i.e. upstream `main` == this fork.

## References

- Upstream `#3447` (named `'w'` handle, NOT_PLANNED/vouch), `#3082` (original, NOT_PLANNED/vouch), `#3346` (int-fd guard, merged 672085e88).
