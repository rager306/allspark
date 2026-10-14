//! Unit tests for the tool-profile module (S1/S2/S4/S5/S6/S8/S17/S18/S19/S20,
//! O8, O10).

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pa_agent::abort::AbortSignal;
use pa_agent::types::{AgentTool, AgentToolResult, AgentToolUpdateCallback, ToolMarker};

use super::bash_minimizer::{artifact_ref, builtin_filters, reduce, BashMinimizer};
use super::disclosure::{Disclosure, DisclosureItem, DisclosureKind, DisclosureLevel};
use super::facades::FacadeRegistry;
use super::hooks::{AntiGrepHook, GuardKind, HookDecision, MIN_DENY_INTERVAL};
use super::leak_recovery::{self, LeakKind, LeakNudgeBudget};
use super::profile::{
    apply_layers, available_markers, filter_by_markers, Context, Mode, ToolsetDelta, ToolsetLayer,
    CONTEXT_CODING, CONTEXT_MINIMAL, CONTEXT_QUERY, MODE_EDITING, MODE_PLANNING,
};
use super::prompt_templates::{anti_hijack_section, render_template, PromptTemplateVars};
use super::registry::{RegistryError, ToolRegistry};
use super::resolve::resolve_profile;
use super::session_registry::SessionRegistry;
use super::task_executor::{SubmitError, TaskError, TaskExecutor};

/// A configurable test tool.
struct MockTool {
    name: &'static str,
    markers: &'static [ToolMarker],
}

impl MockTool {
    fn make(name: &'static str, markers: &'static [ToolMarker]) -> Arc<dyn AgentTool> {
        Arc::new(Self { name, markers })
    }
}

impl AgentTool for MockTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &'static str {
        "mock"
    }

    fn parameters(&self) -> &serde_json::Value {
        &serde_json::Value::Null
    }

    fn markers(&self) -> &[ToolMarker] {
        self.markers
    }

    fn execute(
        self: Arc<Self>,
        _tool_call_id: String,
        _params: serde_json::Value,
        _signal: AbortSignal,
        _on_update: AgentToolUpdateCallback,
    ) -> pa_agent::BoxFut<'static, anyhow::Result<AgentToolResult>> {
        Box::pin(async move { Ok(AgentToolResult::error("mock")) })
    }
}

fn roster() -> Vec<Arc<dyn AgentTool>> {
    vec![
        MockTool::make("bash", &[ToolMarker::CanEdit]),
        MockTool::make("edit", &[ToolMarker::CanEdit]),
        MockTool::make("find_symbol", &[ToolMarker::SymbolicRead]),
        MockTool::make("replace_symbol_body", &[ToolMarker::SymbolicEdit, ToolMarker::CanEdit]),
        MockTool::make("graph", &[ToolMarker::Optional, ToolMarker::DoesNotRequireActiveProject]),
        MockTool::make("experimental", &[ToolMarker::Beta]),
    ]
}

fn names(tools: &[Arc<dyn AgentTool>]) -> Vec<String> {
    tools.iter().map(|t| t.name().to_string()).collect()
}

// ── S1 contexts ────────────────────────────────────────────────────────────

#[test]
fn context_excludes_named_tools() {
    let context = Context::excluding("query", &["edit", "bash"]);
    let kept = context.apply(&roster());
    assert!(!names(&kept).contains(&"edit".to_string()));
    assert!(!names(&kept).contains(&"bash".to_string()));
    assert!(names(&kept).contains(&"find_symbol".to_string()));
}

#[test]
fn context_drops_optional_and_beta_unless_included() {
    let context = Context::default();
    let kept = names(&context.apply(&roster()));
    assert!(!kept.contains(&"graph".to_string()), "optional dropped");
    assert!(!kept.contains(&"experimental".to_string()), "beta dropped");

    let context = Context {
        included_optional_tools: vec!["graph".into()],
        ..Context::default()
    };
    let kept = names(&context.apply(&roster()));
    assert!(kept.contains(&"graph".to_string()));
    assert!(!kept.contains(&"experimental".to_string()));
}

#[test]
fn context_fixed_tools_replaces_surface() {
    let context = Context {
        fixed_tools: vec!["bash".into()],
        ..Context::default()
    };
    assert_eq!(names(&context.apply(&roster())), vec!["bash"]);
}

#[test]
fn context_fixed_tools_conflicts_with_exclusions() {
    let context = Context {
        name: "bad".into(),
        fixed_tools: vec!["bash".into()],
        excluded_tools: vec!["edit".into()],
        ..Context::default()
    };
    assert!(context.validate().is_err());
}

#[test]
fn context_single_project_excludes_activation() {
    let context = Context {
        name: "single".into(),
        single_project: true,
        ..Context::default()
    };
    let delta = context.delta();
    assert!(delta.excluded_tools.contains(&"activate_project".to_string()));
    assert!(delta.excluded_tools.contains(&"get_current_config".to_string()));
}

#[test]
fn context_presets_resolve() {
    assert!(Context::preset(CONTEXT_CODING).is_some());
    assert_eq!(Context::preset(CONTEXT_QUERY).unwrap().excluded_tools.len(), 2);
    assert_eq!(Context::preset(CONTEXT_MINIMAL).unwrap().fixed_tools.len(), 2);
    assert!(Context::preset("nope").is_none());
}

// ── S2 modes ───────────────────────────────────────────────────────────────

#[test]
fn mode_resolve_covers_nine() {
    assert_eq!(Mode::presets().len(), 9);
    for name in [
        "editing",
        "planning",
        "one-shot",
        "interactive",
        "onboarding",
        "no-memories",
        "no-onboarding",
        "benchmark",
        "query-projects",
    ] {
        assert!(Mode::resolve(name).is_some(), "missing mode {name}");
    }
}

#[test]
fn planning_mode_drops_editing_tools() {
    let mode = Mode::resolve(MODE_PLANNING).unwrap();
    let kept = names(&mode.apply(&roster()));
    assert!(!kept.contains(&"edit".to_string()));
    assert!(kept.contains(&"find_symbol".to_string()));
}

#[test]
fn mode_context_allowlist() {
    let mode = Mode {
        name: "editing".into(),
        allowed_contexts: vec![CONTEXT_CODING.into()],
        ..Mode::default()
    };
    assert!(mode.allows_context(CONTEXT_CODING));
    assert!(!mode.allows_context("other"));
    let open = Mode::resolve(MODE_EDITING).unwrap();
    assert!(open.allows_context("anything"));
}

// ── S4 markers ─────────────────────────────────────────────────────────────

#[test]
fn filter_by_required_and_denied_markers() {
    let editable = filter_by_markers(&roster(), &[ToolMarker::CanEdit], &[]);
    assert_eq!(names(&editable), vec!["bash", "edit", "replace_symbol_body"]);

    let read_only = filter_by_markers(&roster(), &[], &[ToolMarker::CanEdit]);
    assert!(!names(&read_only).contains(&"edit".to_string()));
    assert!(names(&read_only).contains(&"find_symbol".to_string()));
}

#[test]
fn available_markers_lists_declared() {
    let markers = available_markers(&roster());
    assert!(markers.contains(&ToolMarker::SymbolicRead));
    assert!(markers.contains(&ToolMarker::Beta));
    assert_eq!(markers.len(), 6);
}

// ── S8 layering ────────────────────────────────────────────────────────────

#[test]
fn layering_applies_in_order_and_inclusion_wins() {
    let layers = vec![
        ToolsetLayer::Config(ToolsetDelta::excluding(vec!["bash".to_string()])),
        ToolsetLayer::Context(Context::excluding(CONTEXT_CODING, &["edit"])),
        ToolsetLayer::BaseMode(Mode::resolve(MODE_PLANNING).unwrap()),
        ToolsetLayer::SingleProject,
        ToolsetLayer::Project(ToolsetDelta {
            excluded_tools: vec!["find_symbol".into()],
            included_tools: vec!["bash".into()],
        }),
    ];
    let kept = names(&apply_layers(&roster(), &layers));
    assert!(!kept.contains(&"edit".to_string()));
    assert!(!kept.contains(&"find_symbol".to_string()));
    // The project layer re-includes bash after config excluded it.
    assert!(kept.contains(&"bash".to_string()));
}

#[test]
fn toolset_layer_associated_apply_matches_free_fn() {
    let layers = vec![ToolsetLayer::Context(Context::excluding(
        CONTEXT_CODING,
        &["edit"],
    ))];
    let via_assoc = names(&ToolsetLayer::apply_layers(&roster(), &layers));
    let via_free = names(&apply_layers(&roster(), &layers));
    assert_eq!(via_assoc, via_free);
}

#[test]
fn layering_with_no_layers_is_identity() {
    let kept = apply_layers(&roster(), &[]);
    assert_eq!(kept.len(), roster().len());
}

// ── S5 registry ────────────────────────────────────────────────────────────

#[test]
fn registry_register_and_instantiate() {
    let mut registry = ToolRegistry::new();
    let tool = MockTool::make("bash", &[]);
    registry.register_tool(Arc::clone(&tool)).unwrap();
    assert!(registry.contains("bash"));
    assert_eq!(registry.instantiate("bash").unwrap().name(), "bash");
    assert!(matches!(registry.instantiate("nope"), Err(RegistryError::Unknown(_))));
}

#[test]
fn registry_duplicate_and_tombstone() {
    let mut registry = ToolRegistry::new();
    registry.register_tool(MockTool::make("bash", &[])).unwrap();
    assert!(matches!(
        registry.register_tool(MockTool::make("bash", &[])),
        Err(RegistryError::Duplicate(_))
    ));
    registry.unregister("bash").unwrap();
    assert!(registry.is_tombstoned("bash"));
    assert_eq!(registry.tombstones(), vec!["bash"]);
    assert!(matches!(
        registry.register_tool(MockTool::make("bash", &[])),
        Err(RegistryError::Tombstoned(_))
    ));
    assert!(registry.lift_tombstone("bash"));
    assert!(registry.register_tool(MockTool::make("bash", &[])).is_ok());
}

#[test]
fn registry_aliases_and_legacy_names() {
    let mut registry = ToolRegistry::with_serena_defaults();
    assert_eq!(registry.legacy_name("replace_regex"), "replace_content");
    assert_eq!(registry.legacy_name("find_symbol"), "find_symbol");

    registry.add_alias("fs", "lsp");
    registry.add_alias("lsp", "code");
    assert_eq!(registry.resolve_alias("fs"), "code");
    assert_eq!(registry.resolve_alias("code"), "code");
}

#[test]
fn registry_rewrites_param_aliases() {
    let registry = ToolRegistry::with_serena_defaults();
    let args = serde_json::json!({ "name_path": "Foo/bar" });
    let rewritten = registry.apply_param_aliases("find_symbol", &args);
    assert_eq!(rewritten["name_path_pattern"], "Foo/bar");
    assert!(rewritten.get("name_path").is_none());

    // A key already under the new name is left untouched.
    let args = serde_json::json!({ "name_path": "old", "name_path_pattern": "new" });
    let rewritten = registry.apply_param_aliases("find_symbol", &args);
    assert_eq!(rewritten["name_path_pattern"], "new");
}

// ── S6 disclosure ──────────────────────────────────────────────────────────

fn disclosure() -> Disclosure {
    let mut disclosure = Disclosure::new();
    disclosure.register(DisclosureItem {
        path: "lsp".into(),
        kind: DisclosureKind::Facade,
        summary: "language-server facade".into(),
        detail: "Language server facade.".into(),
        referenced: Vec::new(),
        niche: false,
    });
    disclosure.register(DisclosureItem {
        path: "lsp.find_symbol".into(),
        kind: DisclosureKind::Method,
        summary: "find a symbol".into(),
        detail: "find_symbol(name_path) -> Symbol".into(),
        referenced: vec!["LanguageServerSymbol".into()],
        niche: false,
    });
    disclosure.register(DisclosureItem {
        path: "LanguageServerSymbol".into(),
        kind: DisclosureKind::Type,
        summary: "a symbol".into(),
        detail: "struct LanguageServerSymbol { name_path }".into(),
        referenced: Vec::new(),
        niche: false,
    });
    disclosure
}

#[test]
fn disclosure_overview_lists_facades_and_methods() {
    let mut disclosure = disclosure();
    let overview = disclosure.expand("", DisclosureLevel::Overview);
    assert!(overview.contains("lsp"));
    assert!(overview.contains("lsp.find_symbol"));
}

#[test]
fn disclosure_deep_expands_referenced_types() {
    let mut disclosure = disclosure();
    let text = disclosure.expand("lsp.find_symbol", DisclosureLevel::Deep);
    assert!(text.contains("find_symbol(name_path)"));
    assert!(text.contains("struct LanguageServerSymbol"));
}

#[test]
fn disclosure_dedups_within_session() {
    let mut disclosure = disclosure();
    let _ = disclosure.expand("LanguageServerSymbol", DisclosureLevel::Full);
    let second = disclosure.expand("LanguageServerSymbol", DisclosureLevel::Full);
    assert!(second.contains("documented earlier in this session"));
    assert!(disclosure.described_names().contains(&"LanguageServerSymbol"));
}

#[test]
fn disclosure_unknown_item() {
    let mut disclosure = disclosure();
    assert!(disclosure
        .expand("nope", DisclosureLevel::Full)
        .contains("Unknown API item"));
}

// ── S20 prompt templates + S7 anti-hijack ──────────────────────────────────

#[test]
fn template_substitutes_tool_names() {
    let vars = PromptTemplateVars::from_surface(
        ["lsp", "edit"],
        &["find_symbol", "edit"],
    );
    let rendered = render_template("Use {{ tool_names[\"find_symbol\"] }} and {{ tool_names['edit'] }}.", &vars);
    assert_eq!(rendered, "Use find_symbol and edit.");
}

#[test]
fn template_maps_canonical_to_effective() {
    let vars = PromptTemplateVars::from_surface(["jet_brains_find_symbol"], &[])
        .with_tool_name("find_symbol", "jet_brains_find_symbol");
    assert_eq!(vars.tool_name("find_symbol"), "jet_brains_find_symbol");
    let rendered = render_template("{{ tool_names[\"find_symbol\"] }}", &vars);
    assert_eq!(rendered, "jet_brains_find_symbol");
}

#[test]
fn template_renders_lists_and_keeps_unknown() {
    let vars = PromptTemplateVars::from_surface(["bash", "edit"], &["edit"])
        .with_markers([ToolMarker::CanEdit]);
    let rendered = render_template(
        "tools: {{ available_tools }}; markers: {{ available_markers }}; {{ nope }}",
        &vars,
    );
    assert!(rendered.contains("bash, edit"));
    assert!(rendered.contains("ToolMarkerCanEdit"));
    assert!(rendered.contains("{{ nope }}"));
}

#[test]
fn anti_hijack_section_is_forbidden_and_maps_tools() {
    let vars = PromptTemplateVars::from_surface(
        ["get_symbols_overview", "find_symbol", "replace_symbol_body", "rename_symbol"],
        &["find_symbol"],
    );
    let section = anti_hijack_section(&vars);
    assert!(section.contains("FORBIDDEN"));
    assert!(section.contains("Disallowed reasoning"));
    assert!(section.contains("`find_symbol`"));
}

// ── S17 hooks ──────────────────────────────────────────────────────────────

#[test]
fn anti_grep_denies_after_three_consecutive_greps() {
    let mut hook = AntiGrepHook::new();
    let now = Instant::now();
    assert_eq!(hook.observe_at(GuardKind::Grep, now), HookDecision::Allow);
    assert_eq!(hook.observe_at(GuardKind::Grep, now), HookDecision::Allow);
    let deny = hook.observe_at(GuardKind::Grep, now);
    assert!(deny.is_deny());
    assert_eq!(hook.deny_count(GuardKind::Grep), 1);
}

#[test]
fn anti_grep_rate_limits_denies() {
    let mut hook = AntiGrepHook::new();
    let start = Instant::now();
    for _ in 0..3 {
        let _ = hook.observe_at(GuardKind::Grep, start);
    }
    // Within the interval, the deny is suppressed.
    let suppressed = hook.observe_at(GuardKind::Grep, start + Duration::from_secs(1));
    assert!(!suppressed.is_deny());
    // After the interval, it fires again.
    let again = hook.observe_at(GuardKind::Grep, start + MIN_DENY_INTERVAL + Duration::from_secs(1));
    assert!(again.is_deny());
}

#[test]
fn anti_grep_read_hard_ceiling_ignores_rate_limit() {
    let mut hook = AntiGrepHook::new();
    let start = Instant::now();
    let _ = hook.observe_at(GuardKind::Read, start);
    let _ = hook.observe_at(GuardKind::Read, start);
    assert!(hook.observe_at(GuardKind::Read, start).is_deny());
    // Fourth consecutive read denies even inside the rate-limit window.
    assert!(hook.observe_at(GuardKind::Read, start).is_deny());
}

#[test]
fn anti_grep_untracked_tool_resets_streaks() {
    let mut hook = AntiGrepHook::new();
    let _ = hook.observe(GuardKind::Grep);
    let _ = hook.observe(GuardKind::Grep);
    assert_eq!(hook.observe_tool("find_symbol"), HookDecision::Allow);
    // Streak reset: two more greps stay under the threshold.
    assert_eq!(hook.observe(GuardKind::Grep), HookDecision::Allow);
    assert_eq!(hook.observe(GuardKind::Grep), HookDecision::Allow);
}

#[test]
fn anti_grep_classifies_tools() {
    assert_eq!(AntiGrepHook::classify("grep"), Some(GuardKind::Grep));
    assert_eq!(AntiGrepHook::classify("read"), Some(GuardKind::Read));
    assert_eq!(AntiGrepHook::classify("find_symbol"), None);
}

// ── S18 task executor ──────────────────────────────────────────────────────

#[tokio::test]
async fn task_executor_serializes_calls() {
    use std::sync::atomic::{AtomicU32, Ordering};
    let executor = TaskExecutor::new(None);
    let concurrent = Arc::new(AtomicU32::new(0));
    let max_seen = Arc::new(AtomicU32::new(0));
    let mut handles = Vec::new();
    for _ in 0..5 {
        let executor = executor.clone();
        let concurrent = Arc::clone(&concurrent);
        let max_seen = Arc::clone(&max_seen);
        handles.push(tokio::spawn(async move {
            executor
                .submit(async move {
                    let active = concurrent.fetch_add(1, Ordering::SeqCst) + 1;
                    max_seen.fetch_max(active, Ordering::SeqCst);
                    tokio::task::yield_now().await;
                    concurrent.fetch_sub(1, Ordering::SeqCst);
                })
                .await
                .unwrap();
        }));
    }
    for handle in handles {
        handle.await.unwrap();
    }
    assert_eq!(max_seen.load(Ordering::SeqCst), 1, "calls must not overlap");
}

#[tokio::test]
async fn task_executor_times_out() {
    let executor = TaskExecutor::new(Some(Duration::from_millis(10)));
    let result = executor
        .submit(async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            1
        })
        .await;
    assert_eq!(result, Err(TaskError::Timeout(Duration::from_millis(10))));
}

#[tokio::test]
async fn task_executor_submit_result_propagates_errors() {
    let executor = TaskExecutor::new(None);
    let ok: Result<u32, SubmitError<&str>> = executor.submit_result(async { Ok(7) }).await;
    assert_eq!(ok.unwrap(), 7);
    let err: Result<u32, SubmitError<&str>> = executor.submit_result(async { Err("boom") }).await;
    assert!(matches!(err, Err(SubmitError::Call("boom"))));
}

// ── S19 session registry ───────────────────────────────────────────────────

#[test]
fn session_registry_touch_and_evict_ttl() {
    let mut registry = SessionRegistry::new(100, Duration::from_hours(48));
    registry.touch("a", 0);
    assert!(registry.contains("a"));
    // Past the 48h TTL: evicted on the next touch.
    let ttl_ms = 48 * 60 * 60 * 1000;
    let eviction = registry.touch("b", ttl_ms);
    assert_eq!(eviction.evicted, vec!["a"]);
    assert!(!registry.contains("a"));
}

#[test]
fn session_registry_lru_cap() {
    let mut registry = SessionRegistry::new(2, Duration::from_hours(48));
    registry.touch("a", 1);
    registry.touch("b", 2);
    let eviction = registry.touch("c", 3);
    assert_eq!(eviction.evicted, vec!["a"]);
    assert_eq!(registry.len(), 2);
    assert_eq!(registry.ids_by_recency(), vec!["c", "b"]);
}

#[test]
fn session_registry_remove_and_clear() {
    let mut registry = SessionRegistry::default();
    registry.touch("a", 1);
    assert!(registry.remove("a"));
    assert!(!registry.remove("a"));
    registry.touch("b", 1);
    registry.clear();
    assert!(registry.is_empty());
}

// ── O8 leak recovery ───────────────────────────────────────────────────────

#[test]
fn leak_detects_and_recovers_harmony() {
    let text = "thinking <|call|>{\"name\":\"bash\",\"arguments\":{\"cmd\":\"ls\"}}<|end|> done";
    assert_eq!(leak_recovery::detect(text), Some(LeakKind::Harmony));
    let recovery = leak_recovery::recover(text).unwrap();
    assert_eq!(recovery.calls.len(), 1);
    assert_eq!(recovery.calls[0].name, "bash");
    assert_eq!(recovery.calls[0].arguments["cmd"], "ls");
    assert!(recovery.cleaned_text.contains("thinking"));
    assert!(!recovery.cleaned_text.contains("<|call|>"));
}

#[test]
fn leak_detects_and_recovers_dsml() {
    let text = "<|DSML|invoke name=\"find_symbol\">{\"name_path\":\"Foo\"}</|DSML|invoke>";
    assert_eq!(leak_recovery::detect(text), Some(LeakKind::Dsml));
    let recovery = leak_recovery::recover(text).unwrap();
    assert_eq!(recovery.calls.len(), 1);
    assert_eq!(recovery.calls[0].name, "find_symbol");
    assert_eq!(recovery.calls[0].arguments["name_path"], "Foo");
}

#[test]
fn leak_no_markup_returns_none() {
    assert!(leak_recovery::detect("plain text").is_none());
    assert!(leak_recovery::recover("plain text").is_none());
}

#[test]
fn leak_nudge_budget_caps() {
    let mut budget = LeakNudgeBudget::new();
    assert!(budget.try_nudge());
    assert!(budget.try_nudge());
    assert!(!budget.try_nudge());
    assert!(budget.exhausted());
    assert_eq!(budget.used(), 2);
}

// ── O10 bash minimizer ─────────────────────────────────────────────────────

#[test]
fn minimizer_filter_table_has_25() {
    assert_eq!(builtin_filters().len(), 25);
}

#[test]
fn minimizer_selects_filter_by_prefix() {
    let minimizer = BashMinimizer::new();
    assert_eq!(minimizer.select_filter("cargo build --release").unwrap().name, "cargo-build");
    assert_eq!(minimizer.select_filter("git status").unwrap().name, "git-status");
    assert!(minimizer.select_filter("echo hi").is_none());
}

#[test]
fn minimizer_reduces_cargo_noise() {
    let minimizer = BashMinimizer::new();
    let output = "Compiling foo v0.1.0\nCompiling bar v0.2.0\nFinished dev profile\nwarning: unused import\nerror[E0425]: cannot find value\n";
    let result = minimizer.minimize("cargo build", output);
    assert!(result.reduced);
    assert!(result.original_ref.unwrap().starts_with("artifact://bash/"));
    assert!(result.output.contains("error[E0425]"));
    assert!(result.output.contains("warning: unused import"));
    assert!(!result.output.contains("Compiling foo"));
}

#[test]
fn minimizer_passthrough_for_unknown_command() {
    let minimizer = BashMinimizer::new();
    let output = "line1\nline2";
    let result = minimizer.minimize("echo hi", output);
    assert!(!result.reduced);
    assert_eq!(result.output, output);
}

#[test]
fn minimizer_reduce_elides_middle() {
    let text: String = (0..10)
        .map(|i| format!("error {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let reduced = reduce(&text, 4);
    assert!(reduced.contains("middle elided"));
    assert_eq!(reduced.lines().count(), 5);
}

#[test]
fn artifact_ref_is_stable() {
    assert_eq!(artifact_ref("hello"), artifact_ref("hello"));
    assert_ne!(artifact_ref("hello"), artifact_ref("world"));
}

// ── S3 facades ─────────────────────────────────────────────────────────────

#[test]
fn facade_registry_has_eight_facades() {
    let registry = FacadeRegistry::with_defaults();
    let mut names = registry.names();
    names.sort_unstable();
    assert_eq!(names, vec!["cfg", "edit", "ext", "fs", "jb", "lsp", "mem", "shell"]);
}

#[test]
fn facade_scope_excludes_facade_and_method() {
    let registry = FacadeRegistry::with_defaults();
    let scoped = registry.apply_scope(
        &["mem".to_string(), "lsp.rename_symbol".to_string()],
        &[],
    );
    assert!(!scoped.iter().any(|f| f.name == "mem"));
    let lsp = scoped.iter().find(|f| f.name == "lsp").unwrap();
    assert!(!lsp.methods.iter().any(|m| m.name == "rename_symbol"));
    assert!(lsp.methods.iter().any(|m| m.name == "find_symbol"));
}

#[test]
fn facade_scope_inclusion_readmits() {
    let registry = FacadeRegistry::with_defaults();
    let scoped = registry.apply_scope(&["mem".to_string()], &["mem".to_string()]);
    assert!(scoped.iter().any(|f| f.name == "mem"));
}

// ── serde round-trip ───────────────────────────────────────────────────────

#[test]
fn context_serde_round_trip() {
    let context = Context {
        name: "coding".into(),
        excluded_tools: vec!["edit".into()],
        single_project: true,
        structured_tool_output: false,
        ..Context::default()
    };
    let json = serde_json::to_string(&context).unwrap();
    let back: Context = serde_json::from_str(&json).unwrap();
    assert_eq!(back, context);
}

#[test]
fn toolset_delta_defaults_structured_output_true() {
    let context: Context = serde_json::from_str("{\"name\":\"x\"}").unwrap();
    assert!(context.structured_tool_output);
}

#[test]
fn marker_set_used_for_availability() {
    let markers: BTreeSet<ToolMarker> = available_markers(&roster());
    assert!(markers.contains(&ToolMarker::Optional));
    assert_eq!(ToolMarker::SymbolicRead.class_name(), "ToolMarkerSymbolicRead");
}

// ── Phase-7 resolve + settings wiring ──────────────────────────────────────

#[test]
fn resolve_profile_none_is_default_surface() {
    let resolved = resolve_profile(None, false);
    assert!(resolved.layers.is_empty());
    assert!(resolved.structured_tool_output);
    assert!(!resolved.single_project);
}

#[test]
fn resolve_profile_context_and_modes_build_layers() {
    let settings = crate::settings::ToolProfileSettings {
        context: Some(CONTEXT_CODING.into()),
        modes: vec![MODE_PLANNING.into(), "unknown-mode".into()],
        excluded_tools: vec!["bash".into()],
        included_tools: vec!["bash".into()],
    };
    let resolved = resolve_profile(Some(&settings), false);
    assert_eq!(resolved.layers.len(), 3, "context + 1 known mode + project delta");
    assert_eq!(resolved.prompt_addenda.len(), 1);
    // Unknown mode is skipped, so planning's editing exclusions apply.
    let kept = names(&apply_layers(&roster(), &resolved.layers));
    assert!(!kept.contains(&"edit".to_string()));
    // Project re-includes bash.
    assert!(kept.contains(&"bash".to_string()));
}

#[test]
fn resolve_profile_single_project_pins() {
    let settings = crate::settings::ToolProfileSettings {
        context: Some(CONTEXT_CODING.into()),
        modes: vec![MODE_EDITING.into()],
        ..crate::settings::ToolProfileSettings::default()
    };
    let resolved = resolve_profile(Some(&settings), true);
    assert!(resolved.single_project);
    let kept = names(&apply_layers(&roster(), &resolved.layers));
    assert!(resolved
        .layers
        .iter()
        .any(|l| matches!(l, ToolsetLayer::SingleProject)));
    assert_ne!(kept.len(), 0);
}

#[test]
fn settings_tool_profile_serde() {
    let settings: crate::settings::Settings = serde_json::from_value(serde_json::json!({
        "toolProfile": {
            "context": "query",
            "modes": ["planning"],
            "excludedTools": ["graph"]
        }
    }))
    .unwrap();
    let profile = settings.tool_profile.unwrap();
    assert_eq!(profile.context.as_deref(), Some("query"));
    assert_eq!(profile.modes, vec!["planning"]);
    assert_eq!(profile.excluded_tools, vec!["graph"]);
}

#[test]
fn settings_round_trip_preserves_tool_profile() {
    let settings: crate::settings::Settings = serde_json::from_value(serde_json::json!({
        "toolProfile": { "context": "minimal" }
    }))
    .unwrap();
    let json = serde_json::to_value(&settings).unwrap();
    assert_eq!(json["toolProfile"]["context"], "minimal");
}
