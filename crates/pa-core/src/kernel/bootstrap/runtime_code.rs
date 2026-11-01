//! The runtime bootstrap code injected into each kernel right after start or
//! restore: binds the `rlm`, `bash`, and MCP surfaces and pre-imports every
//! Python skill.

use super::KernelPythonSkill;

/// Line the runtime bootstrap prints when one or more pre-imported Python skills failed to import.
/// The host scans the bootstrap cell's stdout for this marker.
pub const PYTHON_SKILL_IMPORT_ERROR_REPORT_MARKER: &str =
    "__PRIME_AGENT_PYTHON_SKILL_IMPORT_ERRORS__";

/// Failed skill imports reported by a bootstrap cell: `(import name, import error)` pairs in import
/// order (TS `UnavailablePythonSkills = Record<string, string>`).
pub type UnavailablePythonSkills = Vec<(String, String)>;

/// Extract the unavailable-skill report a bootstrap cell printed, or `None` when it printed none:
/// the marker must be followed by a JSON object with at least one non-empty string value.
#[must_use]
pub fn parse_unavailable_python_skills(stdout: &str) -> Option<UnavailablePythonSkills> {
    let at = stdout.find(PYTHON_SKILL_IMPORT_ERROR_REPORT_MARKER)?;
    let raw = stdout[at + PYTHON_SKILL_IMPORT_ERROR_REPORT_MARKER.len()..].trim();
    let parsed = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(raw).ok()?;
    let errors = parsed
        .into_iter()
        .filter_map(|(name, error)| {
            let error = error.as_str()?;
            (!error.is_empty()).then(|| (name, error.to_string()))
        })
        .collect::<UnavailablePythonSkills>();
    (!errors.is_empty()).then_some(errors)
}

const RLM_BOOTSTRAP_HEADER_CODE: &str =
    "import asyncio\nimport os as _prime_agent_os\n\n_prime_agent_os.environ[\"NO_COLOR\"] = \"1\"";

const RLM_BOOTSTRAP_RUNTIME_HEAD_CODE: &str = "
try:
    import rlm as _prime_agent_rlm_module
    rlm = _prime_agent_rlm_module.rlm
    bash = _prime_agent_rlm_module.bash
    import rlm.mcp as mcp
";

/// One line inside the bootstrap's try block, emitted only when the host
/// registered the [`crate::kernel::shared::BASH_COMPLETED_REQUEST_TYPE`]
/// handler: turns the runtime's bounded `await bash()` default on. Without
/// the notice nothing wakes a cell that ended its turn on a degraded command,
/// so daemonless sessions keep the unbounded await.
const RLM_BOOTSTRAP_BASH_NOTICES_LINE_CODE: &str =
    "    _prime_agent_rlm_module._set_completion_notices_wired(True)\n";

const RLM_BOOTSTRAP_RUNTIME_TAIL_CODE: &str = r#"
except Exception as _prime_agent_rlm_error:
    _PRIME_AGENT_RLM_IMPORT_ERROR = str(_prime_agent_rlm_error)

    class _PrimeAgentMissingRlmMember:
        """A non-callable `rlm` member (`harness`, `factory`) in a degraded kernel.

        Reaching any attribute of one (`rlm.harness.create_memory`) raises the
        uninstalled-runtime diagnostic, so a contract name that exists in a
        healthy kernel does not degrade to an AttributeError that reads as a
        bug in the agent's own code.
        """

        def __init__(self, owner, name):
            self._prime_agent_owner = owner
            self._prime_agent_name = name

        def _raise_missing(self):
            self._prime_agent_owner._raise_missing()

        def __getattr__(self, name):
            if name.startswith("__") and name.endswith("__"):
                # Reflection machinery probes dunders on every value it
                # formats; answering those with the runtime diagnostic would
                # break the printer instead of the cell.
                raise AttributeError(name)
            self._raise_missing()

        def __call__(self, *args, **kwargs):
            self._raise_missing()

        def __repr__(self):
            return f"<prime-agent-runtime not installed: rlm.{self._prime_agent_name}>"

    class _PrimeAgentMissingRlm:
        """The whole `rlm` contract (AGENTS.md "Surface contract") in a kernel
        without prime-agent-runtime: every member names the missing runtime
        instead of raising AttributeError.
        """

        def __init__(self):
            self.harness = _PrimeAgentMissingRlmMember(self, "harness")
            self.factory = _PrimeAgentMissingRlmMember(self, "factory")

        def _raise_missing(self):
            raise RuntimeError(
                "prime-agent-runtime is not installed in this kernel. "
                "Remove ~/.prime/agent/kernel-venv so allspark can rebuild it, or set "
                "PRIME_AGENT_KERNEL_PYTHON to a kernel environment with prime-agent-runtime installed. "
                f"Import error: {_PRIME_AGENT_RLM_IMPORT_ERROR}"
            )

        async def spawn(self, prompt, **kwargs):
            self._raise_missing()

        async def find_models(self, query="", limit=8):
            self._raise_missing()

        async def collect(self, targets=None, *, timeout_ms=0):
            self._raise_missing()

        async def create_session(self, prompt, **kwargs):
            self._raise_missing()

        async def list_subagents(self):
            self._raise_missing()

        async def delete_subagent(self, target):
            self._raise_missing()

        async def progress_note(self, message):
            self._raise_missing()

        async def rename(self, new_name, *, session_id=None):
            self._raise_missing()

        def get_harness_state(self, state_dir=None, *, global_=False, **kwargs):
            self._raise_missing()

        def persistent_shells(self):
            self._raise_missing()

    rlm = _PrimeAgentMissingRlm()
    # `mcp` is bound inside the try, so a degraded kernel must bind it too:
    # otherwise `mcp.list_tools(...)` raises NameError, which names neither the
    # missing runtime nor what to do about it.
    mcp = _PrimeAgentMissingRlmMember(rlm, "mcp")

    def bash(command, yield_after=None, persistent=False):
        rlm._raise_missing()
"#;

/// The code the session injects right after kernel start/restore: binds the `rlm`, `bash`, and MCP
/// surfaces, imports every Python skill. `bash_notices_wired` emits the bounded-await toggle only
/// when the host registers the bash completion notice.
#[must_use]
pub fn build_rlm_bootstrap_code(
    python_skills: &[KernelPythonSkill],
    bash_notices_wired: bool,
) -> String {
    let mut runtime_code = String::from(RLM_BOOTSTRAP_RUNTIME_HEAD_CODE);
    if bash_notices_wired {
        runtime_code.push_str(RLM_BOOTSTRAP_BASH_NOTICES_LINE_CODE);
    }
    runtime_code.push_str(RLM_BOOTSTRAP_RUNTIME_TAIL_CODE);
    let base_code = format!("{RLM_BOOTSTRAP_HEADER_CODE}\n\n{runtime_code}");
    // TS: `[...new Set(pythonSkills.map(...))]` — first-seen order, so the pre-import loop follows
    // the session's skill discovery order.
    let mut import_names: Vec<&str> = Vec::new();
    for skill in python_skills {
        let import_name = skill.import_name.as_str();
        if !import_names.contains(&import_name) {
            import_names.push(import_name);
        }
    }
    if import_names.is_empty() {
        return base_code;
    }
    let imports_json = serde_json::to_string(&import_names).unwrap_or_else(|_| "[]".to_string());
    format!(
        r#"
{base_code}

import importlib as _prime_agent_importlib
import inspect as _prime_agent_inspect
import sys as _prime_agent_sys
import types as _prime_agent_types

class _PrimeAgentCallableSkillModule(_prime_agent_types.ModuleType):
    async def __call__(self, *args, **kwargs):
        result = self.run(*args, **kwargs)
        if _prime_agent_inspect.isawaitable(result):
            return await result
        return result

class _PrimeAgentUnavailableSkill:
    def __init__(self, name, error):
        self.__name__ = name
        self._prime_agent_import_error = error
        self.__doc__ = f"Python skill {{name}} is unavailable: {{error}}"

    async def run(self, *args, **kwargs):
        raise RuntimeError(
            f"Python skill {{self.__name__}} is unavailable in this kernel. "
            f"Import error: {{self._prime_agent_import_error}}"
        )

    async def __call__(self, *args, **kwargs):
        return await self.run()

    def __repr__(self):
        return f"<unavailable Python skill {{self.__name__!r}}: {{self._prime_agent_import_error}}>"

def _prime_agent_wrap_skill_module(module):
    run = getattr(module, "run", None)
    if not callable(run):
        return module
    if isinstance(module, _PrimeAgentCallableSkillModule):
        return module
    wrapped = _PrimeAgentCallableSkillModule(module.__name__)
    wrapped.__dict__.update(module.__dict__)
    try:
        wrapped.__signature__ = _prime_agent_inspect.signature(run)
    except Exception:
        pass
    doc = getattr(run, "__doc__", None)
    if doc:
        wrapped.__doc__ = doc
    _prime_agent_sys.modules[module.__name__] = wrapped
    return wrapped

_PRIME_AGENT_SKILL_IMPORT_ERRORS = {{}}

for _prime_agent_skill_name in {imports_json}:
    try:
        globals()[_prime_agent_skill_name] = _prime_agent_wrap_skill_module(
            _prime_agent_importlib.import_module(_prime_agent_skill_name)
        )
    except Exception as _prime_agent_skill_error:
        # An empty exception message would otherwise be dropped by the
        # host-side parser; fall back to the exception type name.
        _prime_agent_skill_error_text = (
            str(_prime_agent_skill_error) or type(_prime_agent_skill_error).__name__
        )
        _PRIME_AGENT_SKILL_IMPORT_ERRORS[_prime_agent_skill_name] = _prime_agent_skill_error_text
        globals()[_prime_agent_skill_name] = _PrimeAgentUnavailableSkill(
            _prime_agent_skill_name,
            _prime_agent_skill_error_text,
        )

if _PRIME_AGENT_SKILL_IMPORT_ERRORS:
    import json as _prime_agent_json
    print(
        "{PYTHON_SKILL_IMPORT_ERROR_REPORT_MARKER}"
        + _prime_agent_json.dumps(_PRIME_AGENT_SKILL_IMPORT_ERRORS)
    )
"#
    )
    .trim()
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;
    use std::path::PathBuf;

    #[test]
    fn bootstrap_code_without_skills_binds_rlm() {
        let code = build_rlm_bootstrap_code(&[], false);
        assert!(code.contains("import rlm as _prime_agent_rlm_module"));
        assert!(!code.contains("_PrimeAgentUnavailableSkill"));
    }

    /// Marks the probe's JSON report on the probe process's stdout.
    const DEGRADED_PROBE_MARKER: &str = "__PRIME_AGENT_DEGRADED_PROBE__";

    /// Every reachable `rlm` member a cell can touch in a degraded kernel:
    /// `(name, is async call, the expression a cell would write)`.
    const DEGRADED_PROBE_ENTRIES: [(&str, bool, &str); 15] = [
        ("spawn", true, r#"rlm.spawn("task", name="child")"#),
        ("find_models", true, "rlm.find_models()"),
        ("collect", true, "rlm.collect()"),
        ("create_session", true, r#"rlm.create_session("task")"#),
        ("list_subagents", true, "rlm.list_subagents()"),
        ("delete_subagent", true, r#"rlm.delete_subagent("child")"#),
        ("progress_note", true, r#"rlm.progress_note("note")"#),
        ("rename", true, r#"rlm.rename("renamed")"#),
        ("get_harness_state", false, "rlm.get_harness_state()"),
        ("persistent_shells", false, "rlm.persistent_shells()"),
        (
            "harness.create_memory",
            false,
            r#"rlm.harness.create_memory("title", "body")"#,
        ),
        ("factory.run", false, r#"rlm.factory.run("review-sweep")"#),
        ("factory.help", false, "rlm.factory.help()"),
        ("mcp.list_tools", false, r#"mcp.list_tools("server")"#),
        ("bash", false, r#"bash("echo hi")"#),
    ];

    /// The probe run inside the degraded kernel: it touches every entry and
    /// reports `{name: [exception type, message]}`.
    const DEGRADED_PROBE_BODY: &str = r#"
import asyncio as _prime_agent_probe_asyncio
import json as _prime_agent_probe_json


async def _prime_agent_probe_one(awaits, expr):
    value = eval(expr, globals())
    if awaits:
        await value


async def _prime_agent_probe_all():
    outcomes = {}
    for name, awaits, expr in _PROBE_ENTRIES:
        try:
            await _prime_agent_probe_one(awaits, expr)
        except BaseException as error:
            outcomes[name] = [type(error).__name__, str(error)]
        else:
            outcomes[name] = ["NO_ERROR", ""]
    # A member proxy must stay quiet under reflection: a repr or a `hasattr`
    # that raises the diagnostic breaks the tool that was merely looking at
    # the value.
    try:
        assert repr(rlm.harness) == "<prime-agent-runtime not installed: rlm.harness>"
        assert not hasattr(rlm.harness, "__iter__")
    except BaseException as error:
        outcomes["__reflection__"] = [type(error).__name__, str(error)]
    else:
        outcomes["__reflection__"] = ["OK", ""]
    print(_PROBE_MARKER + _prime_agent_probe_json.dumps(outcomes))


_prime_agent_probe_asyncio.run(_prime_agent_probe_all())
"#;

    /// Run the real bootstrap with `rlm` unimportable and report, per contract
    /// member, the exception a cell sees. `-E -S` keep the probe honest: no
    /// `PYTHONPATH` and no site-packages, so no `rlm` installed on this machine
    /// can rescue the import the bootstrap is written to survive.
    fn probe_degraded_kernel(python: &str) -> serde_json::Map<String, serde_json::Value> {
        // Python literals, not JSON: the bootstrap cell's globals carry no
        // `json` import this early, and `True`/`False` keep the table readable.
        let mut entries = String::new();
        for (name, awaits, expr) in DEGRADED_PROBE_ENTRIES {
            let awaits = if awaits { "True" } else { "False" };
            let _ = writeln!(entries, "({name:?}, {awaits}, {expr:?}),");
        }
        let script = format!(
            "{bootstrap}\n\n_PROBE_MARKER = {marker:?}\n_PROBE_ENTRIES = [\n{entries}]\n{DEGRADED_PROBE_BODY}",
            bootstrap = build_rlm_bootstrap_code(&[], false),
            marker = DEGRADED_PROBE_MARKER,
        );
        let output = std::process::Command::new(python)
            .arg("-E")
            .arg("-S")
            .arg("-c")
            .arg(&script)
            .output()
            .expect("run the degraded-kernel probe");
        assert!(
            output.status.success(),
            "the probe must survive the degraded bootstrap: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let at = stdout
            .find(DEGRADED_PROBE_MARKER)
            .unwrap_or_else(|| panic!("no probe report in {stdout}"));
        serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(
            stdout[at + DEGRADED_PROBE_MARKER.len()..].trim(),
        )
        .expect("the probe report is JSON")
    }

    /// Every name the surface contract promises on `rlm` names the missing
    /// runtime in a degraded kernel. The assertion is on the exception type:
    /// an `AttributeError` here is the defect — it reads as a bug in the
    /// agent's own code, so the agent hunts its cell instead of its kernel.
    #[test]
    fn degraded_kernel_names_the_missing_runtime_for_every_contract_member() {
        let Some(python) = ["python3", "python"].into_iter().find(|python| {
            std::process::Command::new(python)
                .arg("-c")
                .arg("")
                .output()
                .is_ok()
        }) else {
            eprintln!("no python on PATH; skipping the degraded-kernel probe");
            return;
        };
        let report = probe_degraded_kernel(python);
        for (name, _, _) in DEGRADED_PROBE_ENTRIES {
            let outcome = report
                .get(name)
                .unwrap_or_else(|| panic!("the probe reported nothing for {name}: {report:?}"));
            let reported = outcome.as_array().expect("a [type, message] pair");
            let kind = reported[0].as_str().unwrap_or_default();
            let message = reported[1].as_str().unwrap_or_default();
            assert_eq!(
                kind, "RuntimeError",
                "rlm.{name} must name the missing runtime: {message}"
            );
            assert!(
                message.contains("prime-agent-runtime is not installed"),
                "rlm.{name} must say what is missing: {message}"
            );
        }
        let reflection = &report["__reflection__"];
        assert_eq!(
            reflection[0], "OK",
            "rlm.harness must survive repr() and hasattr() probing: {reflection:?}"
        );
    }

    /// The stub stands in for the runtime's `_RLMNamespace`, so every member
    /// that namespace exposes must appear in it. `collect`, `progress_note`,
    /// `rename`, `get_harness_state`, `harness`, `factory`, and
    /// `persistent_shells` went missing exactly this way, and a member added to
    /// the runtime later would go missing the same way without this check.
    #[test]
    fn degraded_stub_covers_the_runtime_namespace() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../prime-agent-runtime/src/rlm/__init__.py");
        let Ok(source) = std::fs::read_to_string(&path) else {
            eprintln!("no runtime source at {}; skipping", path.display());
            return;
        };
        let class_at = source
            .find("class _RLMNamespace:")
            .expect("the runtime namespace");
        let body = &source[class_at..];
        let body = &body[..body
            .find("\nrlm = _RLMNamespace()")
            .expect("the namespace instance")];
        let mut members: Vec<&str> = Vec::new();
        for line in body.lines() {
            let Some(rest) = line.strip_prefix("    ") else {
                continue;
            };
            if rest.starts_with(' ') || rest.starts_with('#') {
                continue;
            }
            let name = rest.split_once("def ").map_or_else(
                || rest.split_once(" = ").map(|(name, _)| name),
                |(_, signature)| signature.split('(').next(),
            );
            let Some(name) = name.map(str::trim) else {
                continue;
            };
            if name.starts_with("__") || name == "self" {
                continue;
            }
            members.push(name);
        }
        assert!(
            members.len() >= 10,
            "the namespace parse must find the real API: {members:?}"
        );
        let code = build_rlm_bootstrap_code(&[], false);
        let uncovered: Vec<&&str> = members
            .iter()
            .filter(|member| {
                !code.contains(&format!("def {member}("))
                    && !code.contains(&format!("self.{member} = "))
            })
            .collect();
        assert!(
            uncovered.is_empty(),
            "the degraded stub must cover these rlm members; a name the runtime \
             exposes cannot degrade to AttributeError: {uncovered:?}"
        );
    }

    #[test]
    fn bootstrap_turns_the_bounded_bash_default_on_only_with_notices() {
        let daemonless = build_rlm_bootstrap_code(&[], false);
        let noticed = build_rlm_bootstrap_code(&[], true);
        assert!(
            noticed.contains("_set_completion_notices_wired(True)"),
            "the wired bootstrap must enable the bounded bash default: {noticed}"
        );
        assert!(
            !daemonless.contains("_set_completion_notices_wired"),
            "the daemonless bootstrap must not emit the toggle: {daemonless}"
        );
    }

    #[test]
    fn bootstrap_code_imports_skills() {
        let skills = vec![KernelPythonSkill {
            name: "edit".into(),
            import_name: "edit".into(),
            package_path: PathBuf::from("/pkg/edit"),
            pyproject_path: PathBuf::from("/pkg/edit/pyproject.toml"),
        }];
        let code = build_rlm_bootstrap_code(&skills, false);
        assert!(code.contains(r#"for _prime_agent_skill_name in ["edit"]"#));
        assert!(code.contains("_PrimeAgentUnavailableSkill"));
    }

    /// The TS #2381 parser table: marker+JSON, noise-prefixed marker, no marker, non-JSON payload
    /// all land where the host-side report decides notice vs silence.
    #[test]
    fn parse_unavailable_python_skills_table() {
        let cases: Vec<(String, Option<UnavailablePythonSkills>)> = vec![
            (
                format!(
                    "{PYTHON_SKILL_IMPORT_ERROR_REPORT_MARKER}{{\"websearch\":\"No module named 'websearch'\"}}\n"
                ),
                Some(vec![("websearch".into(), "No module named 'websearch'".into())]),
            ),
            (
                format!("noise\n{PYTHON_SKILL_IMPORT_ERROR_REPORT_MARKER}{{\"edit\":\"boom\"}}"),
                Some(vec![("edit".into(), "boom".into())]),
            ),
            ("some unrelated kernel output".to_string(), None),
            (format!("{PYTHON_SKILL_IMPORT_ERROR_REPORT_MARKER}not json"), None),
            (format!("{PYTHON_SKILL_IMPORT_ERROR_REPORT_MARKER}{{}}"), None),
        ];
        for (stdout, expected) in cases {
            assert_eq!(
                parse_unavailable_python_skills(&stdout),
                expected,
                "{stdout}"
            );
        }
    }

    #[test]
    fn parse_unavailable_python_skills_drops_non_string_and_empty_values() {
        // Non-string values and empty messages drop (TS keeps only
        // non-empty string entries); ordering follows the marker's JSON.
        let stdout = format!(
            "{PYTHON_SKILL_IMPORT_ERROR_REPORT_MARKER}{{\"edit\":\"boom\",\"null_skill\":null,\"list_skill\":[1],\"empty_skill\":\"\",\"websearch\":\"No module named 'websearch'\"}}"
        );
        assert_eq!(
            parse_unavailable_python_skills(&stdout),
            Some(vec![
                ("edit".into(), "boom".into()),
                ("websearch".into(), "No module named 'websearch'".into()),
            ])
        );
        // A report whose every value drops stays silent.
        let stdout = format!("{PYTHON_SKILL_IMPORT_ERROR_REPORT_MARKER}{{\"empty_skill\":\"\"}}");
        assert_eq!(parse_unavailable_python_skills(&stdout), None);
    }
}
