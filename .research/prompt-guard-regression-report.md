# Prompt guard regression verification

## Scope

- The graph skill is an explicit fork addition, not a relaxation of the TS packaged set.
- `graph.format_call` gets its own documented API bullet, as required by the real surface extractor.
- No runtime constants, dependencies, or prompt cache boundaries changed.

## Verification

Command:
`PA_TS_BINARY=/nonexistent/ts CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2 cargo test -p pa-core --test prompt_guards -- --test-threads=1`

1. Fixed baseline: **5 passed**.
2. Mutation: remove the `graph.format_call` bullet and fork-set union.
3. Mutated run: **3 passed, 2 failed**, exit 101. The failures name missing graph docs and missing graph in the expected skill set.
4. Exact fixes restored. Final rerun: **5 passed**, exit 0 (parent independently collected the completed verifier).

This exercises the existing non-vacuous extractor and packaged skill discovery; no tests of static lint strings added.
