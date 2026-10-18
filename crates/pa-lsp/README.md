# pa-lsp

Native LSP tool for the fork: symbol-aware navigation, diagnostics diff,
symbol-level edits, and a per-project server mux.

**FORK-ONLY.**

## Actions

`status`, `capabilities`, `reload`, `rename_file`, `diagnostics`, `definition`,
`declaration`, `type_definition`, `implementation`, `references`, `hover`,
`symbols`, `find_symbol`, `find_referencing_symbols`, `find_implementations`,
`find_declaration`, `rename`, `replace_symbol_body`, `insert_before_symbol`,
`insert_after_symbol`, `safe_delete_symbol`, `code_actions`.

A symbol is addressed by a `Class/method` name path (`symbol`) or a 1-based
`line`; the name-path matcher is ported from Serena.

## Guarantees

- **A failed lookup is not a negative answer.** `safe_delete_symbol` refuses to
  delete when the reference lookup fails or no server is available, because an
  unavailable answer is not proof that nothing references the symbol. `force:
  true` is the explicit override.
- **`rename_file` never overwrites.** `std::fs::rename` replaces the destination
  silently on POSIX, so the manager refuses an existing destination and refuses
  a move whose import edits the server could not compute.
- **A rejected workspace edit writes nothing.** The whole edit is validated
  before the first file is touched.
- **Every request is bounded.** One `with_timeout` wrapper wraps each LSP
  round-trip, so a server that is indexing cannot hang the tool forever.
- **Diagnostics are attributed to symbols**, not just files, and the ledger
  reports only the diagnostics a change introduced.

## Mux

`mux.rs` (Unix only) brokers one language server per project over a Unix socket,
keyed by a hash of the exact spawn parameters, so two sessions in the same
project share one `rust-analyzer` instead of fighting over the index.

## Consumers

`pa-core::session_engine::engine` owns the `LspManager` for the session's
lifetime; the `lsp` tool holds a second `Arc`.
