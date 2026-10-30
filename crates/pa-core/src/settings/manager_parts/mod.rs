//! `SettingsManager`: one type, split by the domain each accessor belongs to.
//!
//! It was a single 2 091-line file with 93 methods in one `impl`: retries sat beside theme
//! colours and the fork's tool gates, so no reader could hold "what does this manager own"
//! in their head. Each submodule opens its own `impl SettingsManager` block; Rust allows
//! the same type to be extended across modules, so the split costs nothing at the call
//! sites — `SettingsManager::get_theme` is still the path everywhere.
//!
//! | module | owns |
//! |---|---|
//! | `core` | load/save, raw scope access, error surfacing |
//! | `defaults` | model/provider defaults, recent models |
//! | `ui` | presentation: theme, tree filter, detail, session dir |
//! | `runtime` | retries, compaction, transport, concurrency, tracing |
//! | `telemetry` | the telemetry gate and its notice |
//! | `onboarding` | onboarding flags, package/resource arrays |
//! | `tools` | the fork's native-tool gates and routing |
//! | `jev` | the `[jev]` typed-judgment settings |

mod core;
mod defaults;
mod jev;
mod onboarding;
mod runtime;
mod telemetry;
mod tools;
mod ui;
