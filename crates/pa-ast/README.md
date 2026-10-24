# pa-ast

Structural code analysis: tree-sitter parsing plus ast-grep pattern search.

**FORK-ONLY.** Ported from oh-my-pi's `pi-ast`; do not push upstream.

## Scope

- **Grammars (`language/`)**: native grammars compiled in, plus `.wasm` grammars
  loaded at first use through a shared `WasmStore` (only lexing runs in wasm;
  tree-sitter copies the parse tables into native memory).
- **Parse (`ops.rs`, `language/parsers.rs`, `parse_cache.rs`)**: one parser per
  thread, a cache keyed by (path, mtime, size) so a repeat query does not re-parse.
- **Structural search**: ast-grep patterns over the parse tree.

## Why it matters

The `lsp` symbol layer and the `pa-graph` indexer both read this crate's parse
tree. A change to the grammar set or the wasm loader reaches every
symbol-aware tool, so the tests here are load-bearing for the whole fork.

## Building

The `wasm` feature of `tree-sitter` builds the wasmtime C API, which needs
`cmake` on the build host (the release containers install it for this reason).
