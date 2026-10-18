# pa-mem

Low-level RAM layout structures, ported safely from pogocache.

**FORK-ONLY.** The upstream C uses manual layout tricks; this port keeps the
same memory behaviour inside `#![forbid(unsafe_code)]` where the shape allows it.

## Scope

- Sharded maps and the allocation layout the cache relies on.

## Note

This crate is a leaf: it depends on nothing in the workspace and nothing in the
workspace depends on it yet. It exists so the layout work is available when a
cache needs it, and is kept behind its own tests rather than dead-code-allowed
in a consumer.
