# fork-models.v1.json

The fork's shipped model catalog, compiled into the binary with `include_str!`
by `crates/pa-ai/src/models_fork.rs`.

This file is the source for the fork's built-in providers. Adding or changing a
fork model is an edit here; there is no Rust table to keep in step and no
network or disk read at runtime.

The registry layers overrides on top of this catalog:

1. this shipped asset;
2. `$AGENT_DIR/models/fork-models.v1.json` (operator-editable, read on every
   registry load, same `models.json` schema);
3. `models.json` (highest precedence on a `(provider, id)` conflict).

Each entry is one `Model` in camelCase wire shape. Add the provider id to
`fork_providers()` as well: the registry uses that list to accept operator
overrides for a fork provider.

Tests in `models_fork.rs` pin the provider wiring and the endpoint quirks
(CodeBuddy headers/compat, Muse's `max` levels, OpenCode's low/high/max set).
