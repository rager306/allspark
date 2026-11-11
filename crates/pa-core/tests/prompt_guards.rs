// Pedantic-gate dispositions as src/lib.rs (large_futures/too_many_lines/casts).
#![allow(
    clippy::large_futures,
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

//! Prompt guard tests: cache safety (static layers never contain dynamic content; the cached prefix
//! is byte-identical across sessions) and tool surface (the core layer's documented tools match the
//! real registered surface; a tool change without a prompt update fails here).

use std::collections::BTreeSet;
use std::path::Path;

use pa_core::prompts::layers::{CORE_LAYER, OPINIONATED_LAYER, USAGE_LAYER};
use pa_core::prompts::model_prompts::MODEL_PROMPTS_TOML;
use pa_core::prompts::system_prompt::{
    system_prompt_breakdown, BuildSystemPromptOptions, SegmentKind,
};
use pa_core::skills::load_skills_from_dir;

/// The workspace bundled skills directory (source-checkout layout):
/// pa-core lives at `<root>/crates/pa-core`.
fn bundled_skills_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .join("skills")
}

fn sorted_bundled_skills() -> Vec<pa_core::skills::Skill> {
    let skills_dir = bundled_skills_dir();
    let mut loaded = load_skills_from_dir(&skills_dir, "package");
    loaded
        .skills
        .sort_by(|left, right| left.name.cmp(&right.name));
    loaded.skills
}

// Cache safety

/// Strings that only dynamic segments generate. If one of these ever appears in a static layer
/// file, the cached prefix has started leaking per-session content.
const DYNAMIC_ONLY_MARKERS: &[&str] = &[
    "Working directory:",
    "Conversation log:",
    "Recursive agent depth:",
    "Current date:",
    "<available_skills>",
    "Enabled generic MCP servers:",
    "# Project Context",
    "Pre-installed Python packages:",
    "You are a child agent spawned by",
];

#[test]
fn static_layers_contain_no_dynamic_content() {
    for (name, text) in [
        ("core", CORE_LAYER),
        ("usage", USAGE_LAYER),
        ("opinionated", OPINIONATED_LAYER),
        ("model-prompts", MODEL_PROMPTS_TOML),
    ] {
        for marker in DYNAMIC_ONLY_MARKERS {
            assert!(
                !text.contains(marker),
                "static layer {name} contains dynamic-only marker {marker:?}: \
                 dynamic content must stay in the tail, not the cacheable prefix"
            );
        }
    }
}

#[test]
fn cached_prefix_is_stable_across_sessions() {
    let mut options = BuildSystemPromptOptions {
        cwd: "/first/cwd".to_string(),
        messages_path: Some("/first/session.jsonl".to_string()),
        model_prompt_extras: Some("model guidance."),
        skills: sorted_bundled_skills(),
        selected_tools: Some(vec!["python_repl"]),
        ..Default::default()
    };
    let first = system_prompt_breakdown(&options);

    options.cwd = "/completely/different".into();
    options.messages_path = None;
    options.rlm_depth = Some(3);
    options.rlm_parent_agent = Some("orchestrator");
    options.generic_mcp_servers = vec!["slack".into(), "github".into()];
    options.context_files = vec![("AGENTS.md".into(), "Never commit main.".into())];
    options.append_system_prompt = Some("And one more thing.".into());
    let second = system_prompt_breakdown(&options);

    assert_eq!(first.cached_prefix_len, second.cached_prefix_len);
    assert_eq!(
        &first.assembled[..first.cached_prefix_len],
        &second.assembled[..second.cached_prefix_len]
    );
    assert_eq!(
        &first.assembled[..first.cached_prefix_len],
        &format!(
            "{}\n\n{}\n\n{}\n\nmodel guidance.",
            CORE_LAYER.trim(),
            USAGE_LAYER.trim(),
            OPINIONATED_LAYER.trim()
        )
    );

    let first_tail = &first.assembled[first.cached_prefix_len..];
    assert!(first_tail.contains("Current date: "));
    assert!(first_tail.contains("Working directory: /first/cwd"));
    assert!(first_tail.contains("Recursive agent depth: 0 (root)"));
    assert!(first_tail.contains("<available_skills>"));
    assert!(first_tail.contains("Pre-installed Python packages:"));
    let second_tail = &second.assembled[second.cached_prefix_len..];
    assert!(second_tail.contains("/completely/different"));
    assert!(second_tail.contains("Enabled generic MCP servers: `github`, `slack`."));
    assert!(second_tail.contains("Recursive agent depth: 3 (not root)"));
    assert!(second_tail.contains("You are a child agent spawned by orchestrator."));
    assert!(second_tail.contains("# Project Context"));
    // The appended guidance is the last semantic block; the date trails it
    // deliberately so a midnight rollover invalidates one line, not the tail.
    let before_date = second_tail
        .rsplit_once("Current date: ")
        .map_or(second_tail, |(head, _)| head)
        .trim_end();
    assert!(
        before_date.ends_with("And one more thing."),
        "appended guidance must be the last semantic block: {before_date:?}"
    );

    for segment in &first.segments {
        let inside = first
            .assembled
            .find(&segment.text)
            .is_some_and(|at| at < first.cached_prefix_len);
        assert_eq!(
            inside,
            segment.kind == SegmentKind::Static,
            "segment {} on the wrong side of the cache boundary",
            segment.name
        );
    }
}

// Tool surface

/// Host-request types that are host-internal plumbing, not model-facing
/// programmatic tools (the prompt must not document them).
const INTERNAL_HOST_REQUESTS: &[&str] = &[
    "model.info",
    "mcp.config",
    "mcp.refresh",
    "mcp.begin_login",
    // The runtime's detached-bash notice pair (the kernel's bash module
    // fires them; the model never calls them).
    "bash.completed",
    "bash.consumed",
    // The factory module reports run milestones; the model never calls it.
    "factory.progress",
    // The durable record of a run's START (never a model-visible notice): the
    // runtime writes it after a restart so the session can answer for the run.
    "factory.run.started",
];

/// Map one registered host-request type to the prompt token that documents
/// it. `None` when the request is host-internal.
fn prompt_token_for_host_request(request: &str) -> Option<String> {
    if INTERNAL_HOST_REQUESTS.contains(&request) {
        return None;
    }
    Some(
        match request {
            "rlm.run" => "rlm.spawn",
            "rlm.progress.note" => "rlm.progress_note",
            "agent_observe.list" | "agent_message.list_agents" => "agent_observe.list_agents",
            "agent_observe.get" => "agent_observe.get_agent",
            "agent_observe.recent" => "agent_observe.recent_messages",
            "agent_observe.session_messages" => "rlm.session.messages",
            other => other,
        }
        .to_string(),
    )
}

/// The vendored kernel runtime's MCP module: the `mcp` object every REPL binds.
fn runtime_mcp_module() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .join("prime-agent-runtime/src/rlm/mcp.py")
}

/// Kernel MCP entry points that are NOT model-facing, each with the reason it
/// stays out of the prompt. A name here is a waiver from documentation, so the
/// reason is required and the name must still exist in the module.
const MCP_HOST_ONLY: &[(&str, &str)] = &[
    (
        "status",
        "the daemon's connections view; `list_tools` is the model's stricter contract",
    ),
    (
        "close",
        "terminal: a closed registry refuses all later work, and the runtime calls it at host shutdown",
    ),
];

/// Kernel-side programmatic tools implemented by the vendored `prime-agent-runtime`
/// package (generic MCP calls), as prompt tokens.
///
/// Derived from the runtime source rather than listed here: a hardcoded list only
/// pins the functions somebody remembered, so a new `mcp.*` entry point landed
/// undocumented and the guard stayed green. A function this module defines must
/// either be documented or carry a `MCP_HOST_ONLY` waiver.
fn kernel_mcp_tokens() -> BTreeSet<String> {
    let source = std::fs::read_to_string(runtime_mcp_module()).expect("read rlm/mcp.py");
    source
        .lines()
        .filter_map(|line| {
            let signature = line
                .strip_prefix("async def ")
                .or_else(|| line.strip_prefix("def "))?;
            let name = signature
                .split(['(', ':'])
                .next()
                .unwrap_or_default()
                .trim();
            (!name.is_empty() && !name.starts_with('_')).then(|| name.to_string())
        })
        .filter(|name| !MCP_HOST_ONLY.iter().any(|(hidden, _)| hidden == name))
        .map(|name| format!("mcp.{name}"))
        .collect()
}

/// The harness-state API of the vendored runtime (`rlm.harness` CRUD and
/// `rlm.get_harness_state`). Update together with the runtime sidecar. plus its sanctioned terminal turn call
/// (`rlm.end_turn`). Update together with the runtime sidecar.
const HARNESS_TOKENS: &[&str] = &[
    "rlm.end_turn",
    "rlm.harness.create_memory",
    "rlm.harness.update_memory",
    "rlm.harness.delete_memory",
    "rlm.harness.create_prompt_note",
    "rlm.harness.update_prompt_note",
    "rlm.harness.delete_prompt_note",
    "rlm.harness.create_skill",
    "rlm.harness.update_skill",
    "rlm.harness.delete_skill",
    "rlm.harness.create_subagent",
    "rlm.harness.update_subagent",
    "rlm.harness.delete_subagent",
    "rlm.harness.record_refinement",
    "rlm.harness.plan_refinement",
    "rlm.harness.overview",
    "rlm.harness.search",
    "rlm.get_harness_state",
];

/// A host-request type literal: lowercase dotted name (`rlm.collect`).
fn is_request_type_literal(literal: &str) -> bool {
    !literal.is_empty()
        && literal
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_lowercase())
        && literal
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '.')
        && literal.contains('.')
}

/// Remove every `#[cfg(test)]`-annotated item from `source`, so a scan for
/// live registrations cannot pick up test fixtures. Handles the common shapes:
/// the attribute followed by an optional doc/attribute run and then a
/// braced item (`mod tests { .. }`, `fn .. { .. }`) or a `;`-terminated one.
fn strip_cfg_test_items(source: &str) -> String {
    const MARKER: &str = "#[cfg(test)]";
    if !source.contains(MARKER) {
        return source.to_string();
    }
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(pos) = rest.find(MARKER) {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + MARKER.len()..];
        // Skip further attributes and whitespace up to the item's opening
        // brace or terminating semicolon, tracking strings so a `{` inside a
        // literal cannot open the item early.
        let bytes = after.as_bytes();
        let mut i = 0;
        let mut in_str = false;
        let mut escaped = false;
        let mut depth = 0usize;
        let mut opened = false;
        while i < bytes.len() {
            let ch = bytes[i] as char;
            if in_str {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    in_str = false;
                }
                i += 1;
                continue;
            }
            match ch {
                '"' => in_str = true,
                '{' => {
                    depth += 1;
                    opened = true;
                }
                '}' => {
                    depth = depth.saturating_sub(1);
                    if opened && depth == 0 {
                        i += 1;
                        break;
                    }
                }
                ';' if !opened => {
                    i += 1;
                    break;
                }
                _ => {}
            }
            i += 1;
        }
        rest = &after[i..];
    }
    out.push_str(rest);
    out
}

/// Scan the pa-core sources for every host-request type the session engine
/// registers: `handlers.register("<type>", ...)` literals plus
/// `for request_type in [ "<type>", ... ]` loop tables.
fn registered_host_requests() -> BTreeSet<String> {
    let src_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = BTreeSet::new();
    let mut stack = vec![src_root];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            // Test modules register nothing on the live surface: their
            // `.register(...)` calls are fixtures (e.g. a disclosure test that
            // builds a `lsp.find_symbol` item), and counting them would demand
            // prompt documentation for tools the session never exposes.
            if path.file_name().and_then(|name| name.to_str()) == Some("tests.rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("read source");
            let source = strip_cfg_test_items(&source);
            // Direct registrations: the request type is the FIRST argument of each
            // `.register(` — a literal right at the call's head counts; anything
            // after a non-literal first argument is not the request type.
            for remainder in source.split(".register(").skip(1) {
                let rest = remainder.trim_start();
                if rest.starts_with('"') {
                    if let Some(literal) = first_string_literal(rest) {
                        if is_request_type_literal(&literal) {
                            found.insert(literal);
                        }
                    }
                }
            }
            // Loop tables: `for request_type in [ "a.b", ... ]`.
            for remainder in source.split("for request_type in").skip(1) {
                let Some(open) = remainder.find('[') else {
                    continue;
                };
                let Some(close) = remainder[open..].find(']') else {
                    continue;
                };
                for literal in all_string_literals(&remainder[open..open + close]) {
                    if is_request_type_literal(&literal) {
                        found.insert(literal);
                    }
                }
            }
        }
    }
    found
}

fn first_string_literal(text: &str) -> Option<String> {
    let open = text.find('"')?;
    let tail = &text[open + 1..];
    let close = tail.find('"')?;
    Some(tail[..close].to_string())
}

fn all_string_literals(text: &str) -> Vec<String> {
    let mut literals = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('"') {
        let tail = &rest[open + 1..];
        match tail.find('"') {
            Some(close) => {
                literals.push(tail[..close].to_string());
                rest = &tail[close + 1..];
            }
            None => break,
        }
    }
    literals
}

/// Kernel-bound REPL names from the real bootstrap code (`rlm`, `bash`,
/// `mcp`), derived from `build_rlm_bootstrap_code`.
fn kernel_bound_names() -> BTreeSet<String> {
    let code = pa_core::kernel::bootstrap::build_rlm_bootstrap_code(&[], false);
    let mut names = BTreeSet::new();
    for line in code.lines() {
        let line = line.trim_start();
        if line.starts_with("rlm = ") {
            names.insert("rlm".to_string());
        }
        if line.starts_with("bash = ") {
            names.insert("bash".to_string());
        }
        if let Some(rest) = line.strip_prefix("import rlm.mcp as ") {
            names.insert(rest.trim().to_string());
        }
    }
    names
}

/// One bundled Python skill's public module-level functions (column-zero
/// `def`/`async def` only, so nested helpers stay out).
fn python_skill_functions(package_path: &Path) -> Vec<String> {
    let mut functions = Vec::new();
    let mut stack = vec![package_path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("py") {
                continue;
            }
            // A skill's own tests define functions too; they are not part of
            // the importable surface, so counting them would demand prompt
            // documentation for helpers the model never calls.
            if path
                .components()
                .any(|component| component.as_os_str() == "tests")
            {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(&path) else {
                continue;
            };
            for line in source.lines() {
                let after = line
                    .strip_prefix("async def ")
                    .or_else(|| line.strip_prefix("def "));
                if let Some(signature) = after {
                    if let Some(name) = signature.split(['(', ':']).next() {
                        functions.push(name.trim().to_string());
                    }
                }
            }
        }
    }
    functions
}

/// Bundled skills that are auth-gated builtin MCP integrations: they are disabled in sessions whose
/// user is not logged into the integration, so the prompt documents them only through the dynamic
/// skills inventory (and its generic "additional skills may exist" note), not as API surface.
fn auth_gated_bundled_skills() -> BTreeSet<String> {
    pa_core::mcp::BUILTIN_MCP_CATALOG
        .iter()
        .map(|(server, _, _)| server.to_string())
        .collect()
}

/// Conditionally-available bundled skills: the websearch skill rides with its
/// credential (an unset `bundledSkills.websearch` resolves with the selected
/// provider's stored or ambient key), so keyless installs exclude it — it must never be
/// documented as static core-tool surface; the dynamic skills inventory
/// renders it only on installs where it resolves enabled.
fn settings_gated_bundled_skills() -> BTreeSet<String> {
    ["websearch".to_string()].into_iter().collect()
}

/// (import name, public functions) for every bundled Python skill that is
/// always available (auth-gated integrations and the conditionally-available
/// websearch skill excluded).
fn bundled_python_skills() -> Vec<(String, Vec<String>)> {
    let gated = auth_gated_bundled_skills();
    let settings_gated = settings_gated_bundled_skills();
    sorted_bundled_skills()
        .into_iter()
        .filter(|skill| {
            skill
                .python
                .as_ref()
                .is_none_or(|python| !gated.contains(&python.import_name))
        })
        .filter(|skill| !settings_gated.contains(&skill.name))
        .filter_map(|skill| {
            let python = skill.python?;
            let mut functions = python_skill_functions(&python.package_path)
                .into_iter()
                .filter(|name| !name.starts_with('_'))
                .collect::<Vec<_>>();
            functions.sort();
            Some((python.import_name, functions))
        })
        .collect()
}

/// The surface tokens the prompt is allowed to document: kernel-bound names, registered host
/// requests, bundled Python skills, and the runtime-package (harness + generic MCP) API.
fn allowed_surface_tokens() -> BTreeSet<String> {
    let mut allowed = BTreeSet::new();
    allowed.extend(kernel_bound_names());
    for request in registered_host_requests() {
        if let Some(token) = prompt_token_for_host_request(&request) {
            allowed.insert(token);
        }
    }
    for (import, functions) in bundled_python_skills() {
        allowed.insert(import.clone());
        for function in functions {
            allowed.insert(format!("{import}.{function}"));
        }
    }
    allowed.extend(kernel_mcp_tokens());
    for token in HARNESS_TOKENS {
        allowed.insert(token.to_string());
    }
    allowed
}

/// The surface tokens the prompt MUST document. Stricter than the allowed set: skill entry points
/// (`run`/`status`) are callable through the module itself, so the module bullet suffices for them.
fn required_surface_tokens() -> BTreeSet<String> {
    let mut required = BTreeSet::new();
    for request in registered_host_requests() {
        if let Some(token) = prompt_token_for_host_request(&request) {
            required.insert(token);
        }
    }
    for (import, functions) in bundled_python_skills() {
        required.insert(import.clone());
        for function in functions {
            if function == "run" || function == "status" {
                continue;
            }
            required.insert(format!("{import}.{function}"));
        }
    }
    required.extend(kernel_mcp_tokens());
    for token in HARNESS_TOKENS {
        required.insert(token.to_string());
    }
    required
}

/// Surface tokens documented in the core layer: bullet entries whose first
/// backtick token is a lowercase module path.
fn documented_surface_tokens() -> BTreeSet<String> {
    let mut documented = BTreeSet::new();
    for line in CORE_LAYER.lines() {
        let Some(rest) = line.trim_start().strip_prefix("- `") else {
            continue;
        };
        let Some(end) = rest.find('`') else {
            continue;
        };
        let token = rest[..end].split('(').next().unwrap_or_default().trim();
        let valid = token
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_lowercase())
            && token
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '.')
            && !token.is_empty();
        if valid {
            documented.insert(token.to_string());
        }
    }
    documented
}

/// The MCP waivers stay honest in both directions: a waived name the runtime no
/// longer defines is a stale exemption, and a waived name the prompt documents
/// anyway means the prompt and the waiver disagree about who the surface is for.
#[test]
fn mcp_documentation_waivers_stay_honest() {
    let source = std::fs::read_to_string(runtime_mcp_module()).expect("read rlm/mcp.py");
    let documented = documented_surface_tokens();
    for (name, reason) in MCP_HOST_ONLY {
        assert!(
            !reason.is_empty(),
            "MCP_HOST_ONLY waives {name} from documentation without a reason"
        );
        assert!(
            source.contains(&format!("def {name}(")),
            "MCP_HOST_ONLY waives {name}, which the runtime no longer defines"
        );
        assert!(
            !documented.contains(&format!("mcp.{name}")),
            "{name} is waived as host-only but the prompt documents it; drop one of the two"
        );
    }
}

#[test]
fn core_layer_documents_the_real_tool_surface() {
    let documented = documented_surface_tokens();
    let allowed = allowed_surface_tokens();
    let required = required_surface_tokens();

    assert!(
        !required.is_empty() && !documented.is_empty(),
        "surface extraction produced nothing; the guard would be vacuous"
    );

    for token in &documented {
        let root = token.split('.').next().expect("non-empty token");
        let known = allowed.contains(token)
            || allowed.contains(root)
            || allowed
                .iter()
                .any(|candidate| candidate.starts_with(&format!("{root}.")));
        assert!(
            known,
            "prompt documents unknown tool {token:?}: a tool-surface change must \
             update prompts/layers/core.md (and this test's token mapping)"
        );
    }

    for token in &required {
        let documented_form = documented.contains(token)
            || documented
                .iter()
                .any(|entry| entry.starts_with(&format!("{token}.")))
            || documented
                .iter()
                .any(|entry| entry.starts_with(token.as_str()) && entry.contains('.'));
        assert!(
            documented_form,
            "tool {token:?} is on the registered surface but the prompt does not \
             document it: update prompts/layers/core.md"
        );
    }
}

// Packaged-set parity (TS packages/coding-agent/skills)

// FORK-ONLY: additions must be declared without weakening the TS packaged-set check.
const FORK_ADDED_SKILL_SET: &[&str] = &["graph"];

/// The packaged skill set at TS tip `f62dae4d0` (`packages/coding-agent/skills/`): 12 skills — the
/// generic `mcp` doc skill in, the per-service linear/notion pair out (TS removed theirs when the
/// generic MCP surface landed).
const TS_PACKAGED_SKILL_SET: &[&str] = &[
    "agent-message",
    "agent-observe",
    "attach-image",
    "compact",
    "edit",
    "goal",
    "mcp",
    "prime-intellect",
    "refine",
    "rlm-heartbeat",
    "skill-creator",
    "websearch",
];

/// The bundled skills match the TS packaged set plus declared fork additions. The generic `mcp`
/// skill is markdown-only, and the retired per-service pair (linear/notion) is gone.
#[test]
fn bundled_skills_match_the_ts_packaged_set() {
    let skills = sorted_bundled_skills();
    let names: Vec<&str> = skills.iter().map(|skill| skill.name.as_str()).collect();
    let mut expected = TS_PACKAGED_SKILL_SET.to_vec();
    expected.extend_from_slice(FORK_ADDED_SKILL_SET);
    expected.sort_unstable();
    assert_eq!(
        names, expected,
        "the packaged skill set must match the TS set plus the fork additions"
    );
    assert!(
        !names.contains(&"linear") && !names.contains(&"notion"),
        "the per-service MCP skills retired with the generic mcp skill"
    );
    let mcp_skill = skills
        .iter()
        .find(|skill| skill.name == "mcp")
        .expect("the generic mcp skill is packaged");
    assert!(
        mcp_skill.python.is_none(),
        "the mcp skill is documentation for the pre-imported runtime module, not a Python package"
    );
    assert_eq!(
        mcp_skill.description,
        "Use external MCP services generically from Python - search the supported-service catalog, inspect the user's connections, discover live tool schemas, and call tools on any connection (Notion, Linear, Slack, and the rest of the catalog) without per-service packages."
    );
}

/// The bounded-await contract renders wherever the Python REPL does: every session kind, daemon
/// or daemonless, registers the bash completion notice the degrade relies on.
#[test]
fn bounded_await_contract_renders_in_daemon_and_daemonless_sessions() {
    let mut options = BuildSystemPromptOptions {
        cwd: "/w".to_string(),
        messages_path: Some("/log.jsonl".to_string()),
        skills: sorted_bundled_skills(),
        selected_tools: Some(vec!["python_repl"]),
        ..Default::default()
    };
    for daemonless in [false, true] {
        options.daemonless = daemonless;
        let prompt = system_prompt_breakdown(&options).assembled;
        assert!(
            prompt.contains("`r.running` is True")
                && prompt.contains("`r.exit_code` is None"),
            "daemonless={daemonless}: the prompt must carry the bounded-await result contract: {prompt}"
        );
    }
}

/// The generic mcp skill loads through the normal markdown discovery and renders in the
/// `<available_skills>` inventory exactly like any other markdown skill: the `[skill name
/// location]`-style XML the TS `formatSkillsForPrompt` emits, without a `python_import` line.
#[test]
fn generic_mcp_skill_renders_in_the_prompt_inventory() {
    let mut options = BuildSystemPromptOptions {
        cwd: "/w".to_string(),
        messages_path: Some("/log.jsonl".to_string()),
        skills: sorted_bundled_skills(),
        selected_tools: Some(vec!["python_repl"]),
        ..Default::default()
    };
    let breakdown = system_prompt_breakdown(&options);
    let inventory = breakdown
        .segments
        .iter()
        .find(|segment| {
            matches!(segment.kind, SegmentKind::Dynamic)
                && segment.text.contains("<available_skills>")
        })
        .map(|segment| segment.text.clone())
        .expect("the skills inventory segment renders");
    assert!(inventory.contains("<name>mcp</name>"));
    assert!(inventory.contains("<type>markdown</type>"));
    let mcp_block_start = inventory
        .find("<name>mcp</name>")
        .expect("the mcp skill block");
    let mcp_block_end = inventory[mcp_block_start..]
        .find("</skill>")
        .map_or(inventory.len(), |end| mcp_block_start + end);
    let mcp_block = &inventory[mcp_block_start..mcp_block_end];
    assert!(
        !mcp_block.contains("python_import"),
        "a markdown skill never carries a python_import line"
    );
    options.generic_mcp_servers = vec!["notion".into()];
    let second = system_prompt_breakdown(&options);
    assert!(second
        .assembled
        .contains("await mcp.list_tools(\"notion\")"));
}

// Async turn discipline

/// Since #3580 every session kind wakes on bash completion: daemonless runs
/// get a queued `[bash-done]` turn, so the static usage layer must not carry
/// the pre-#3580 rule to await commands before ending the turn — the
/// no-polling rule right below it already ends turns on the done
/// notification, and an unscoped await rule would contradict that wake.
#[test]
fn usage_layer_ends_turns_on_the_done_notification_not_an_await_rule() {
    assert!(
        !USAGE_LAYER.contains("await commands before ending the turn"),
        "every session kind has completion notifications: the await-before-turn-end rule is stale"
    );
    assert!(
        USAGE_LAYER.contains("end the turn and collect the result on the done notification"),
        "the no-polling rule keeps the collect-on-notice turn-end contract"
    );
}

// ─── The review mirror: BUGBOT.md must not silently lose a rule ───────────────
//
// `AGENTS.md` says the Cursor Bugbot rules "mirror the standards in this file — update both
// whenever either changes", and NOTHING checked that. The fourth reverify wave found the
// mirror had already lost four families of rule (style, prompt caps, critical path,
// branding) before a sweep put them back, and the only thing preventing the next loss was
// that somebody remembered. A rule file with no reader is a rule file that rots.
//
// The check is not "the files are equal" — they are written for different readers and word
// the same rule differently. It is: every AGENTS.md section that states a REVIEWABLE
// standard has a counterpart section in BUGBOT.md. The mapping is explicit, because the
// names differ on purpose (`Style and structure` → `Style` + `Module organization`).

/// The AGENTS.md sections that hold a standard a reviewer enforces, with the BUGBOT.md
/// sections that must carry it. A section added to one file without the other is the defect.
const REVIEWED_AGENTS_SECTIONS: &[(&str, &[&str])] = &[
    ("Change hygiene", &["Change hygiene"]),
    ("Lint discipline", &["Lint discipline"]),
    // The fork's two style sections are one AGENTS.md section and two BUGBOT.md ones: the
    // crate/module rules and the formatting rules are written for different readers.
    ("Style and structure", &["Style", "Module organization"]),
    (
        "Prompt-input caps are part of the contract",
        &["Prompt-input caps"],
    ),
    (
        "Performance and the critical path",
        &["Performance and the critical path"],
    ),
    ("Adoption telemetry", &["Adoption telemetry"]),
    ("Live-system safety", &["Live-system safety"]),
    (
        "Parallel agents share one working tree",
        &["Parallel agents share one tree"],
    ),
    ("Branding", &["Branding"]),
    ("Tests", &["Tests"]),
    ("Formal verification", &["Formal verification"]),
];

fn repo_file(relative: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root");
    std::fs::read_to_string(root.join(relative)).unwrap_or_else(|error| {
        panic!("read {relative}: {error}");
    })
}

fn section_titles(markdown: &str) -> BTreeSet<String> {
    markdown
        .lines()
        .filter_map(|line| line.strip_prefix("## "))
        .map(|title| title.trim().to_string())
        .collect()
}

/// Every reviewable AGENTS.md section has a BUGBOT.md counterpart.
///
/// Red on the tree before the sweep that restored the four lost families: `Style`,
/// `Prompt-input caps`, `Performance and the critical path` and `Branding` were all missing.
#[test]
fn every_reviewable_agents_section_has_a_bugbot_counterpart() {
    let bugbot = section_titles(&repo_file(".cursor/BUGBOT.md"));
    let mut missing: Vec<String> = Vec::new();
    for (agents_section, counterparts) in REVIEWED_AGENTS_SECTIONS {
        for required in *counterparts {
            if !bugbot.contains(*required) && !bugbot.contains(&format!("{required} (advisory)")) {
                missing.push(format!(
                    "`{agents_section}` needs a `{required}` section in BUGBOT.md"
                ));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "the review mirror has drifted from AGENTS.md:\n  {}",
        missing.join("\n  ")
    );
}

/// And the reverse: a BUGBOT.md section that names no AGENTS.md standard is a rule nobody
/// maintains. `No legacy or migration scaffolding`, `Comments`, `Generated files` and the
/// rest are deliberately narrower than an AGENTS.md section, so the assertion is that every
/// BUGBOT.md section is either mapped above or listed as an allowed addition.
#[test]
fn every_bugbot_section_is_mapped_or_declared() {
    const STANDALONE: &[&str] = &[
        "No legacy or migration scaffolding",
        "Comments",
        "Style",
        "Prompt-input caps",
        "Module organization",
        "File size",
        "Dependency direction",
        "Repo hygiene",
        "Generated files",
    ];
    let mapped: BTreeSet<&str> = REVIEWED_AGENTS_SECTIONS
        .iter()
        .flat_map(|(_, counterparts)| counterparts.iter().copied())
        .collect();
    let unmapped: Vec<String> = section_titles(&repo_file(".cursor/BUGBOT.md"))
        .into_iter()
        .filter(|title| {
            let base = title.trim_end_matches(" (advisory)");
            !mapped.contains(base) && !STANDALONE.contains(&base)
        })
        .collect();
    assert!(
        unmapped.is_empty(),
        "these BUGBOT.md sections name no AGENTS.md standard and are not declared \
         standalone:\n  {}",
        unmapped.join("\n  ")
    );
}

// ─── Test hygiene: no test redirects a path the whole process shares ────────
//
// Review #26: a daemon test called `std::env::set_var("TMPDIR", …)` to move the default
// socket path into its own scratch. `TMPDIR` is PROCESS-WIDE, so every other test in that
// binary — the TS differential corpus ran concurrently — created its temporary directories
// inside this one's, and this one's cleanup deleted them mid-run.
//
// Two rules, and the split matters:
//   * `TMPDIR` — NEVER. `TempDir::new()` reads it, so moving it moves every neighbour's
//     fixtures, and the damage is silent (their files vanish, not their assertions).
//   * `HOME` — allowed, because some paths are resolved through it and a test that wants
//     them must be able to say so; but only in a file that SERIALISES those tests behind
//     its own mutex, or two of them race with each other.
//
// A scanning test is the honest tier here: the regression IS a pattern in test source, and
// a runtime assertion cannot see it — the test that reintroduces the call is the one that
// breaks, not this one.
#[test]
fn no_test_redirects_a_path_the_whole_process_shares() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root");
    let mut offenders: Vec<String> = Vec::new();
    for entry in walk_rs_files(&root.join("crates")) {
        let Ok(text) = std::fs::read_to_string(&entry) else {
            continue;
        };
        // This file spells the patterns it forbids; scanning itself would always fail.
        if entry
            .file_name()
            .is_some_and(|name| name == "prompt_guards.rs")
        {
            continue;
        }
        // Only TEST code: a `set_var` in a shipped module is a different question.
        if !entry.to_string_lossy().contains("/tests/") && !text.contains("#[cfg(test)]") {
            continue;
        }
        // Only LIVE code: a doc comment that explains why a test does NOT call
        // `set_var("HOME")` must not count as calling it (measured — the first version of
        // this scan flagged exactly that comment). Comment lines are blanked rather than
        // dropped: the paste-seam scan reports line numbers, and they must be the file's.
        let live: String = text
            .lines()
            .map(|line| {
                if line.trim_start().starts_with("//") {
                    ""
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        // A third process-wide seam: the paste fixture path the TUI resolves on every
        // paste. `TempDir`s do not share it, but the VARIABLE does — two tests that point it
        // at their own fixture overwrite each other, and the loser's paste reads a path whose
        // temp dir the winner already removed (review #27). The allow is on the WRITE, not on
        // the file: see [`paste_seam_offenders`].
        let seam = paste_seam_offenders(&live);
        if !seam.is_empty() {
            offenders.push(format!(
                "{}: writes the paste seam outside the seam guard's own statements, so a \
                 parallel test's paste reads a path whose temp dir is already gone ({})",
                entry.display(),
                seam.join(", ")
            ));
        }
        for var in ["TMPDIR", "HOME"] {
            let spelled = [format!("set_var(\"{var}\""), format!("set_var(\'{var}\'")]
                .iter()
                .filter(|form| live.contains(form.as_str()))
                .count();
            let removed = live.contains(&format!("remove_var(\"{var}\""));
            if spelled == 0 && !removed {
                continue;
            }
            if var == "TMPDIR" {
                offenders.push(format!(
                    "{}: mutates TMPDIR, which decides where EVERY test's TempDir lands",
                    entry.display()
                ));
                continue;
            }
            // HOME: allowed only where the mutating tests are serialised against each other.
            let serialised = live.contains("ENV_MUTEX")
                || live.contains("env_lock()")
                || live.contains("ENV_LOCK")
                || live.contains("session_lock");
            if !serialised {
                offenders.push(format!(
                    "{}: mutates HOME with no mutex, so two tests that both move it race",
                    entry.display()
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a test redirects a path the whole process shares (review #26):\n  {}",
        offenders.join("\n  ")
    );
}

/// The paste fixture seam the TUI resolves on every paste.
const PASTE_SEAM_VAR: &str = "PRIME_AGENT_TEST_CLIPBOARD_IMAGE";

/// Writes of the paste seam that are NOT the seam guard's own statements.
///
/// The first version of this scan allowed the whole FILE as soon as it mentioned the guard
/// (`CLIPBOARD_IMAGE_SEAM` / `ClipboardImageSeam`), so a test that kept the guard and then
/// wrote `set_var("PRIME_AGENT_TEST_CLIPBOARD_IMAGE", …)` beside it — a restored literal
/// write — was exempt (review #27's remaining half). The allow is per STATEMENT: a seam
/// write is legal only inside the guard type's own `impl` blocks (`ClipboardImageSeam`'s
/// install, and its `Drop` restore). Every other write is reported here.
///
/// Line-based on purpose: the regression is a pattern in test source, and a write whose
/// guard block cannot be located is an offender (fail closed), so a formatting change can
/// only make the scan stricter.
fn paste_seam_offenders(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    // The seam's spellings in THIS file: the quoted literal, plus any
    // `const NAME: &str = "<seam>"` the file names it by (the usual case).
    let mut names: Vec<String> = vec![format!("\"{PASTE_SEAM_VAR}\"")];
    for line in &lines {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("const ") {
            if rest.contains(&format!("= \"{PASTE_SEAM_VAR}\"")) {
                if let Some((name, _)) = rest.split_once(':') {
                    names.push(name.trim().to_string());
                }
            }
        }
    }
    let names_the_seam =
        |candidate: &str| names.iter().any(|name| candidate.contains(name.as_str()));
    let guarded = guard_statement_lines(&lines);
    let mut offenders = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let write_at = ["set_var(", "remove_var("]
            .iter()
            .filter_map(|call| line.find(call))
            .min();
        let Some(offset) = write_at else {
            continue;
        };
        // The statement may wrap: read to the call's closing parenthesis and look for the
        // seam's name anywhere inside it.
        let mut depth = 0_i32;
        let mut statement = String::new();
        let mut end = index;
        'scan: for (cursor, part) in lines[index..].iter().enumerate() {
            end = index + cursor;
            statement.push_str(part);
            for (position, character) in part.char_indices() {
                if cursor == 0 && position < offset {
                    continue;
                }
                match character {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            break 'scan;
                        }
                    }
                    _ => {}
                }
            }
        }
        if !names_the_seam(&statement) {
            continue;
        }
        if (index..=end).all(|cursor| guarded.contains(&cursor)) {
            continue;
        }
        offenders.push(format!("line {}", index + 1));
    }
    offenders
}

/// The line indices inside the seam guard's own `impl` blocks.
///
/// `impl ClipboardImageSeam` (install) and `impl Drop for ClipboardImageSeam` (restore)
/// are the only places a seam write belongs: both run under the seam mutex. The block is
/// found by brace balance from its header, so the scan does not depend on indentation.
fn guard_statement_lines(lines: &[&str]) -> std::collections::BTreeSet<usize> {
    let mut guarded = std::collections::BTreeSet::new();
    let mut index = 0;
    while index < lines.len() {
        let trimmed = lines[index].trim_start();
        let header = trimmed.starts_with("impl ClipboardImageSeam")
            || trimmed.starts_with("impl Drop for ClipboardImageSeam");
        if !header {
            index += 1;
            continue;
        }
        let mut depth = 0_i32;
        for (cursor, line) in lines[index..].iter().enumerate() {
            for character in line.chars() {
                match character {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    _ => {}
                }
            }
            guarded.insert(index + cursor);
            if depth == 0 {
                index += cursor + 1;
                break;
            }
        }
    }
    guarded
}

/// The guard's own writes are legal; a restored LITERAL write beside them is not.
///
/// The first version of the scan exempted a whole FILE that mentioned the guard, so this
/// pair of statements — the seam guard doing its job, and a test writing the seam by hand
/// anyway — passed (review #27's remaining half). The exemption must be per statement.
#[test]
fn a_restored_literal_paste_seam_write_is_still_an_offender() {
    let fixture = r#"
const TEST_CLIPBOARD_IMAGE_ENV: &str = "PRIME_AGENT_TEST_CLIPBOARD_IMAGE";
static CLIPBOARD_IMAGE_SEAM: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct ClipboardImageSeam {
    _lock: tokio::sync::MutexGuard<'static, ()>,
    previous: Option<std::ffi::OsString>,
}

impl ClipboardImageSeam {
    async fn install(path: &Path) -> Self {
        let lock = CLIPBOARD_IMAGE_SEAM.lock().await;
        let previous = std::env::var_os(TEST_CLIPBOARD_IMAGE_ENV);
        std::env::set_var(TEST_CLIPBOARD_IMAGE_ENV, path);
        Self { _lock: lock, previous }
    }
}

impl Drop for ClipboardImageSeam {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(previous) => std::env::set_var(TEST_CLIPBOARD_IMAGE_ENV, previous),
            None => std::env::remove_var(TEST_CLIPBOARD_IMAGE_ENV),
        }
    }
}

#[test]
fn a_paste_test() {
    std::env::set_var("PRIME_AGENT_TEST_CLIPBOARD_IMAGE", "/tmp/fixture.png");
    let _seam = ClipboardImageSeam::install(Path::new("/tmp/fixture.png"));
}
"#;
    let offenders = paste_seam_offenders(fixture);
    assert_eq!(
        offenders,
        vec!["line 30".to_string()],
        "only the hand-written literal write is an offender: the guard's own install and          Drop statements are the legal ones"
    );
    // And the negative direction, so the assertion above cannot pass by flagging everything:
    // the guard's statements alone are clean.
    let only_the_guard: String = fixture
        .lines()
        .take_while(|line| !line.starts_with("#[test]"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        paste_seam_offenders(&only_the_guard),
        Vec::<String>::new(),
        "the seam guard's own statements are the allow"
    );
}

/// The scan, against the REAL test file that owns the seam: clean as it stands, and an
/// offender the moment a literal write is restored beside the guard.
///
/// The regression's shape is not invented: `interactive_daemon_e2e.rs` is the file the
/// review named, and its guard is what makes it legal today. Reading it (never editing it)
/// proves the allow is narrow — the guard stays, and the write is still caught.
#[test]
fn the_real_paste_seam_file_is_clean_until_a_literal_write_returns() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root");
    let path = root.join("crates/pa-cli/tests/interactive_daemon_e2e.rs");
    let text = std::fs::read_to_string(&path).expect("read the daemon e2e suite");
    assert_eq!(
        paste_seam_offenders(&text),
        Vec::<String>::new(),
        "the seam writes in {path:?} all sit inside the guard's own impl blocks"
    );
    // Restore the pre-review shape: keep everything, add one literal write to a test.
    let restored = text.replace(
        "    let _seam = ClipboardImageSeam::install(&png_path).await;",
        "    std::env::set_var(\"PRIME_AGENT_TEST_CLIPBOARD_IMAGE\", &png_path);\n    \
         let _seam = ClipboardImageSeam::install(&png_path).await;",
    );
    assert_ne!(
        restored, text,
        "the fixture line moved: update this test's anchor"
    );
    let offenders = paste_seam_offenders(&restored);
    assert!(
        !offenders.is_empty(),
        "a literal write restored beside the guard must still be an offender"
    );
    assert_eq!(
        offenders.len(),
        text.matches("let _seam = ClipboardImageSeam::install(&png_path).await;")
            .count(),
        "every clipboard test that restored a literal write is reported: {offenders:?}"
    );
}

/// Every `.rs` file under `dir`, recursively. `target/` is not under `crates/`.
fn walk_rs_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk_rs_files(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
    out
}
