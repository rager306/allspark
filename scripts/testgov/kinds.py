"""The checks. Each one proves a property of the matrix against the tree.

Design rule taken from the reference implementations and from two failures we measured
here: a check must be able to fail. `selftest` runs every check against a fixture that
must make it emit a HIGH.
"""

from __future__ import annotations

import re
from pathlib import Path

from .dictionary import (
    AXES, GATES, LAYERS, LAYER_AXIS_REACH, PLANES, REQUIRED, STATUSES,
    Matrix, Row, load_matrix,
)
from .protocol import Check, CheckResult, Context, Finding, Severity
from .rust_index import RustFn, index_rust, test_names

CONCRETE_STATUSES = frozenset({"killed", "blocked", "gate"})
_PLACEHOLDER = re.compile(r"^(tbd|todo|n/?a|-+|\?+)$", re.IGNORECASE)


def _matrix(ctx: Context) -> tuple[Matrix, list[Finding]]:
    matrix = load_matrix(ctx.root)
    if not matrix.ok:
        return matrix, [Finding("matrix", Severity.HIGH, "matrix-unreadable", matrix.error)]
    return matrix, []


# ── shared index plumbing ───────────────────────────────────────────────────
# The Rust index and the file walk are built ONCE per run and handed to every check. Building
# them per check cost 55 s of 58 s wall clock (measured 2026-10-24).

_RUST_INDEX_CACHE: dict[Path, dict[str, list]] = {}


def _rust_index(ctx: Context, tree):
    """One parse per run, owned by the shared tree walk."""
    if tree is not None:
        return tree.rust_index()
    cached = _RUST_INDEX_CACHE.get(ctx.root)
    if cached is None:
        cached = index_rust(ctx.root)
        _RUST_INDEX_CACHE[ctx.root] = cached
    return cached


def _rust_files(ctx: Context, tree):
    """Every .rs file, from the shared walk when present."""
    from .tree import walk as _walk

    if tree is not None:
        return tree.files(".rs")
    return _walk(ctx.root).files(".rs")


class MatrixShape(Check):
    name = "matrix-shape"
    description = "Required keys, vocabulary, and unique ids."

    def run(self, ctx: Context, *, tree=None) -> CheckResult:
        result = CheckResult(self.name)
        matrix, findings = _matrix(ctx)
        result.findings.extend(findings)
        if not matrix.ok:
            return result
        seen: dict[str, int] = {}
        for row in matrix.rows:
            result.scanned += 1
            where = f"{'doc/testing/test-matrix.yaml'}:{row.line}" if row.line else "doc/testing/test-matrix.yaml"
            missing = [key for key in REQUIRED if key not in row.raw]
            if missing:
                result.findings.append(Finding(
                    self.name, Severity.HIGH, "row-missing-keys",
                    f"{row.id} is missing {missing}", file="doc/testing/test-matrix.yaml", line=row.line))
                continue
            if row.id in seen:
                result.findings.append(Finding(
                    self.name, Severity.HIGH, "row-duplicate-id",
                    f"{row.id} appears twice (first at line {seen[row.id]})",
                    file="doc/testing/test-matrix.yaml", line=row.line))
            seen[row.id] = row.line
            plane = row.get("plane")
            if plane not in PLANES:
                result.findings.append(Finding(
                    self.name, Severity.HIGH, "row-bad-plane",
                    f"{row.id}: plane {plane!r} not in {sorted(PLANES)}",
                    file="doc/testing/test-matrix.yaml", line=row.line))
            layer = row.get("layer")
            if layer not in LAYERS:
                result.findings.append(Finding(
                    self.name, Severity.HIGH, "row-bad-layer",
                    f"{row.id}: layer {layer!r} not in {sorted(LAYERS)}",
                    file="doc/testing/test-matrix.yaml", line=row.line))
            status = row.get("status")
            if status not in STATUSES:
                result.findings.append(Finding(
                    self.name, Severity.HIGH, "row-bad-status",
                    f"{row.id}: status {status!r} not in {sorted(STATUSES)}",
                    file="doc/testing/test-matrix.yaml", line=row.line))
            gate = row.get("gate")
            if gate not in GATES:
                result.findings.append(Finding(
                    self.name, Severity.HIGH, "row-bad-gate",
                    f"{row.id}: gate {gate!r} not in {sorted(GATES)}",
                    file="doc/testing/test-matrix.yaml", line=row.line))
            axes = row.get("axes")
            if not isinstance(axes, dict) or not axes:
                result.findings.append(Finding(
                    self.name, Severity.HIGH, "row-bad-axes",
                    f"{row.id}: axes must be a non-empty mapping of {sorted(AXES)}",
                    file="doc/testing/test-matrix.yaml", line=row.line))
            else:
                unknown = sorted(set(axes) - AXES)
                if unknown:
                    result.findings.append(Finding(
                        self.name, Severity.HIGH, "row-unknown-axis",
                        f"{row.id}: unknown axes {unknown}",
                        file="doc/testing/test-matrix.yaml", line=row.line))
            if not isinstance(row.get("oracle"), list):
                result.findings.append(Finding(
                    self.name, Severity.HIGH, "row-oracle-not-list",
                    f"{row.id}: oracle must be a list (empty is legal for a gap)",
                    file="doc/testing/test-matrix.yaml", line=row.line))
        return result


class OracleExists(Check):
    name = "oracle-exists"
    description = "Every named oracle is a real, non-ignored, non-helper test fn or a governor check."

    @staticmethod
    def _governor_oracles(ctx: Context) -> set[str]:
        """Oracle names that live in this checker rather than in Rust: `snake_case` of each
        Check class, plus the check's own `name`. A governor-plane row has no Rust test to
        point at — its oracle is the check that enforces it."""
        import inspect

        from . import kinds as kinds_module

        names: set[str] = set()
        for _name, obj in inspect.getmembers(kinds_module, inspect.isclass):
            if not issubclass(obj, Check) or obj is Check:
                continue
            check_name = getattr(obj, "name", "")
            if check_name:
                names.add(check_name)
                names.add(check_name.replace("-", "_"))
        return names

    def run(self, ctx: Context, *, tree=None) -> CheckResult:
        result = CheckResult(self.name)
        matrix, findings = _matrix(ctx)
        result.findings.extend(findings)
        if not matrix.ok:
            return result
        # Targeted, not exhaustive: the matrix asks about a handful of names, and a literal
        # prefilter resolves them in 0.22 s instead of building the 19 083-name index (0.82 s).
        # Exa research: ast-grep's biggest win was removing duplicate traversal; the trigram
        # literature's rule is "index once, then O(candidates)".
        from .oracle_index import resolve_names

        wanted: set[str] = set()
        for row in matrix.rows:
            wanted.update(row.oracles)
        index = resolve_names(tree, wanted) if tree is not None else _rust_index(ctx, tree)
        governor = self._governor_oracles(ctx)
        for row in matrix.rows:
            governed = row.get("layer") == "governor"
            for name in row.oracles:
                result.scanned += 1
                if governed and name in governor:
                    continue
                definitions = index.get(name)
                if not definitions:
                    result.findings.append(Finding(
                        self.name, Severity.HIGH, "rust-test-missing",
                        f"{row.id}: {name} is not a fn anywhere in crates/",
                        file="doc/testing/test-matrix.yaml", line=row.line))
                    continue
                qualifying = [entry for entry in definitions if entry.qualifies_as_test()]
                if qualifying:
                    continue
                ignored = [entry for entry in definitions if entry.ignored]
                if ignored:
                    entry = ignored[0]
                    result.findings.append(Finding(
                        self.name, Severity.HIGH, "rust-test-ignored",
                        f"{row.id}: {name} is #[ignore]d, so it proves nothing at gate time",
                        file=entry.file, line=entry.line))
                else:
                    entry = definitions[0]
                    result.findings.append(Finding(
                        self.name, Severity.HIGH, "rust-test-not-a-test",
                        f"{row.id}: {name} has no #[test] attribute",
                        file=entry.file, line=entry.line))
        return result


class KillMutationPresent(Check):
    name = "kill-mutation-present"
    description = "Every row states the mutation that must make it red (anti-vacuity)."

    def run(self, ctx: Context, *, tree=None) -> CheckResult:
        result = CheckResult(self.name)
        matrix, findings = _matrix(ctx)
        result.findings.extend(findings)
        if not matrix.ok:
            return result
        for row in matrix.rows:
            result.scanned += 1
            value = row.get("kill_mutation")
            text = str(value or "").strip()
            if not text or _PLACEHOLDER.match(text) or len(text) < 12:
                result.findings.append(Finding(
                    self.name, Severity.HIGH, "kill-mutation-missing",
                    f"{row.id}: kill_mutation must name the edit that makes this row red",
                    file="doc/testing/test-matrix.yaml", line=row.line))
        return result


class AxisUncovered(Check):
    name = "axis-uncovered"
    description = "A row claiming a status needs an oracle on each axis it names."

    def run(self, ctx: Context, *, tree=None) -> CheckResult:
        result = CheckResult(self.name)
        matrix, findings = _matrix(ctx)
        result.findings.extend(findings)
        if not matrix.ok:
            return result
        for row in matrix.rows:
            result.scanned += 1
            status = row.get("status")
            if status not in CONCRETE_STATUSES:
                continue
            if not row.oracles:
                result.findings.append(Finding(
                    self.name, Severity.HIGH, "axis-uncovered",
                    f"{row.id}: status {status!r} claims work but names no oracle "
                    f"(use `gap` for no oracle, `spec-only` for an unwritten claim)",
                    file="doc/testing/test-matrix.yaml", line=row.line))
        return result


class GovernorRowsCarryACheck(Check):
    """A governor-plane row must name a check that this tool actually implements.

    Measured need: two of this ADR's own first rows claimed the governor proved them while
    naming no oracle, and `blocked` read as "work in progress" when nothing was in progress.
    """

    name = "governor-row-names-a-check"
    description = "A governor row's oracle exists as a Check class."

    def run(self, ctx: Context, *, tree=None) -> CheckResult:
        result = CheckResult(self.name)
        matrix, findings = _matrix(ctx)
        result.findings.extend(findings)
        if not matrix.ok:
            return result
        known = OracleExists._governor_oracles(ctx)
        for row in matrix.rows:
            if row.get("layer") != "governor":
                continue
            result.scanned += 1
            if not any(name in known for name in row.oracles):
                result.findings.append(Finding(
                    self.name, Severity.HIGH, "governor-row-names-no-check",
                    f"{row.id}: no oracle names an implemented check",
                    file="doc/testing/test-matrix.yaml", line=row.line))
        return result


class PlaneCollapse(Check):
    name = "plane-collapse"
    description = "A harness row must not assert product behaviour, and vice versa."

    def run(self, ctx: Context, *, tree=None) -> CheckResult:
        result = CheckResult(self.name)
        matrix, findings = _matrix(ctx)
        result.findings.extend(findings)
        if not matrix.ok:
            return result
        for row in matrix.rows:
            result.scanned += 1
            plane = row.get("plane")
            paths = row.paths
            if plane == "ui":
                # A ui row must cite the surface it renders, not only the data that feeds it.
                if not any(path.startswith("crates/pa-tui") or "pty" in path for path in paths):
                    result.findings.append(Finding(
                        self.name, Severity.HIGH, "plane-collapse",
                        f"{row.id}: ui plane cites no ui/pty surface {paths}",
                        file="doc/testing/test-matrix.yaml", line=row.line))
        return result


class LayerTooCheap(Check):
    name = "layer-too-cheap"
    description = "A row's layer must be able to observe the axes it names."

    def run(self, ctx: Context, *, tree=None) -> CheckResult:
        result = CheckResult(self.name)
        matrix, findings = _matrix(ctx)
        result.findings.extend(findings)
        if not matrix.ok:
            return result
        for row in matrix.rows:
            result.scanned += 1
            layer = row.get("layer")
            reach = LAYER_AXIS_REACH.get(str(layer))
            if reach is None:
                continue
            unreachable = sorted(set(row.axis_names) - reach)
            if unreachable:
                result.findings.append(Finding(
                    self.name, Severity.HIGH, "layer-too-cheap",
                    f"{row.id}: layer {layer!r} cannot observe axes {unreachable}; "
                    f"it reaches {sorted(reach)}",
                    file="doc/testing/test-matrix.yaml", line=row.line))
        return result


class InertGuard(Check):
    """A guard that cannot fail is not a guard.

    Two shapes, both measured in this repository today:
      * an exit-code path that discards the computed code (`run_doctor` -> `handled()`), and
      * a waiver where the producer exempts itself but the consumer does not (`generate
        --fixture` sets allow_small, `verify` does not).
    """

    name = "inert-guard"
    description = "Named guard patterns that cannot fail."

    # (path, must_not_match, category, why): a guard that is FIXED must not fire here.
    # The prepush-gate row was removed when `export ROOT` landed (the operand is now
    # exported, so the guard can fire — the defect is fixed, not waived).
    PATTERNS = (
        (
            "scripts/release/bundle_catalog.py",
            re.compile(r"allow_small\w*\s*=\s*[^\n]*or\s+options\.fixture"),
            "self-waiving-gate",
            "the producer waives its own gate while `verify` does not",
        ),
    )

    def run(self, ctx: Context, *, tree=None) -> CheckResult:
        result = CheckResult(self.name)
        for rel_path, pattern, category, why in self.PATTERNS:
            result.scanned += 1
            text = ctx.read(rel_path)
            if text is None:
                continue
            for number, line in enumerate(text.splitlines(), start=1):
                if pattern.search(line):
                    result.findings.append(Finding(
                        self.name, Severity.HIGH, category,
                        f"{why}: {line.strip()[:110]}", file=rel_path, line=number))
        return result


class UntrackedOracle(Check):
    """A test that no row names is untracked work (LOW: it may be a helper)."""

    name = "untracked-oracle"
    description = "Test fns matching no row's oracle list."

    def run(self, ctx: Context, *, tree=None) -> CheckResult:
        result = CheckResult(self.name)
        matrix, findings = _matrix(ctx)
        result.findings.extend(findings)
        if not matrix.ok:
            return result
        index = _rust_index(ctx, tree)
        tracked: set[str] = set()
        for row in matrix.rows:
            tracked.update(row.oracles)
        # Only names that look like behavioural claims; a helper named `test_` prefix or a
        # fixture constructor is not an oracle and must not spam the report.
        candidates = {
            name for name in test_names(index)
            if not name.startswith("test_") and len(name) > 24
        }
        result.scanned = len(candidates)
        untracked = sorted(candidates - tracked)
        if untracked:
            sample = untracked[:5]
            result.findings.append(Finding(
                self.name, Severity.LOW, "untracked-oracle",
                f"{len(untracked)} behavioural tests name no row, e.g. {sample}"))
        return result


class RowStaleVsDiff(Check):
    """In --changed mode, code on a row's axis changed but the row did not."""

    name = "row-stale-vs-diff"
    description = "A changed code path whose row was not touched."

    def run(self, ctx: Context, *, tree=None) -> CheckResult:
        result = CheckResult(self.name)
        if ctx.changed_paths is None:
            result.note = "no --changed input; skipped"
            return result
        matrix, findings = _matrix(ctx)
        result.findings.extend(findings)
        if not matrix.ok:
            return result
        for row in matrix.rows:
            result.scanned += 1
            code = row.paths
            if not code:
                continue
            touched = [path for path in code if any(path in changed for changed in ctx.changed_paths)]
            if not touched:
                continue
            matrix_touched = any("doc/testing/test-matrix.yaml" in changed for changed in ctx.changed_paths)
            if not matrix_touched:
                result.findings.append(Finding(
                    self.name, Severity.HIGH, "row-stale-vs-diff",
                    f"{row.id}: {touched} changed but the matrix did not",
                    file="doc/testing/test-matrix.yaml", line=row.line))
        return result


class RowFileMissing(Check):
    """A cited path that does not exist is a contract the tree cannot honour.

    `paths` and `axes.code` both cite source files; a rename or deletion that leaves a row
    pointing at a ghost is exactly the drift the set-equality oracle exists to catch. `doc/`
    paths are the contract documents themselves, not sources — the contract may plan a
    document that is not written yet, so only source paths must exist.
    """

    name = "row-file-missing"
    description = "Every cited source path (paths, axes.code) exists; doc/ contract paths exempt."

    def run(self, ctx: Context, *, tree=None) -> CheckResult:
        result = CheckResult(self.name)
        matrix, findings = _matrix(ctx)
        result.findings.extend(findings)
        if not matrix.ok:
            return result
        for row in matrix.rows:
            axes = row.get("axes")
            code = axes.get("code") if isinstance(axes, dict) else None
            cited = [path for path in (code or []) if isinstance(path, str)]
            extra = row.get("paths")
            if isinstance(extra, list):
                cited.extend(path for path in extra if isinstance(path, str))
            for path in cited:
                result.scanned += 1
                if path.startswith("doc/"):
                    continue
                if not (ctx.root / path).exists():
                    result.findings.append(Finding(
                        self.name, Severity.HIGH, "row-file-missing",
                        f"{row.id}: cited path does not exist: {path}", file=path))
        return result


class CapabilityCoverage(Check):
    """A source surface with no row at all (LOW: an inventory, not a score)."""

    name = "capability-coverage"
    description = "Crates whose files appear in no row's code axis."

    def run(self, ctx: Context, *, tree=None) -> CheckResult:
        result = CheckResult(self.name)
        matrix, findings = _matrix(ctx)
        result.findings.extend(findings)
        if not matrix.ok:
            return result
        cited: set[str] = set()
        for row in matrix.rows:
            cited.update(row.paths)
        crates_dir = ctx.root / "crates"
        if not crates_dir.is_dir():
            return result
        seen: dict[str, int] = {}
        for crate in sorted(crates_dir.iterdir()):
            if not crate.is_dir():
                continue
            result.scanned += 1
            files = sorted((crate / "src").rglob("*.rs")) if (crate / "src").is_dir() else []
            if not files:
                continue
            hit = any(cited_path.startswith(f"crates/{crate.name}/") for cited_path in cited)
            if not hit:
                seen[crate.name] = len(files)
        for crate_name, file_count in sorted(seen.items(), key=lambda item: -item[1])[:12]:
            result.findings.append(Finding(
                self.name, Severity.LOW, "crate-uncited",
                f"crates/{crate_name} ({file_count} src files) is named by no row"))

        return result


def by_name() -> dict[str, type[Check]]:
    return {check.name: check for check in ALL_CHECKS}


def _load_toml(path: Path):
    try:
        import tomllib
    except ModuleNotFoundError:  # python < 3.11
        import tomli as tomllib
    return tomllib.loads(path.read_text(encoding="utf-8", errors="replace"))


def _load_yaml(path: Path):
    import yaml
    return yaml.safe_load(path.read_text(encoding="utf-8", errors="replace"))


class NonRustSurfacesParse(Check):
    """Every shipped config is a contract; a malformed one fails at run time, not build time.

    Measured surface: 40 tracked .toml, 17 .yaml + 13 .yml, and the JSON assets the binary
    reads at startup (models.bundled.json, mcp-services.bundled.json, package.json). Cargo
    parses .toml for us; nothing parses the rest before a user's first launch.
    """

    name = "non-rust-surfaces-parse"
    description = "Every tracked .toml/.yaml/.yml and shipped JSON asset parses."

    def run(self, ctx: Context, *, tree=None) -> CheckResult:
        result = CheckResult(self.name)
        import json as json_module

        from .tree import walk as _walk

        shared = tree if tree is not None else _walk(ctx.root)
        targets: list[Path] = []
        for suffix in (".toml", ".yaml", ".yml"):
            targets.extend(path for path in shared.files(suffix) if not _is_scratch(path, ctx.root))
        shipped_json = {
            "package.json", "models.bundled.json", "mcp-services.bundled.json",
            "mcp-service-catalog.v2.json",
        }
        targets.extend(
            path for path in shared.files(".json")
            if path.name in shipped_json and not _is_scratch(path, ctx.root)
        )
        for path in sorted(targets):
            result.scanned += 1
            rel = path.relative_to(ctx.root).as_posix()
            try:
                if path.suffix == ".toml":
                    _load_toml(path)
                elif path.suffix in (".yaml", ".yml"):
                    _load_yaml(path)
                else:
                    json_module.loads(path.read_text(encoding="utf-8", errors="replace"))
            except Exception as error:  # noqa: BLE001 - the message is the finding
                result.findings.append(Finding(
                    self.name, Severity.HIGH, "surface-unparsable",
                    f"{rel} does not parse: {error}", file=rel))
        return result


class SettingsHaveConsumers(Check):
    """A setting that parses but reaches no consumer is a documented lie.

    The class: `AGENTS.md` §'Check what is promised against what exists' names it, and the graph
    audit found 13 such breaks (`graph.auto = "watch"` promised a watcher never connected).
    A field declared in `settings/types.rs` must be read somewhere outside that file.
    """

    name = "settings-have-consumers"
    description = "Every settings field is read outside its declaration file."

    # Fields that exist only to be serialized/round-tripped need an explicit exemption.
    EXEMPT = frozenset({"default", "schema_version", "rest"})

    def run(self, ctx: Context, *, tree=None) -> CheckResult:
        result = CheckResult(self.name)
        types = ctx.read("crates/pa-core/src/settings/types.rs")
        if types is None:
            return result
        import re as re_module

        declared = sorted(set(re_module.findall(r"^\s+pub ([a-z][a-z0-9_]*):", types, re_module.M)))
        # Read sites: every .rs outside the settings module. Comes from the shared walk.
        from .tree import walk as _walk

        shared = tree if tree is not None else _walk(ctx.root)
        corpus: list[str] = []
        for path in shared.files(".rs"):
            rel = shared.rel(path)
            if not rel.startswith("crates/"):
                continue
            if rel.startswith("crates/pa-core/src/settings/"):
                continue
            corpus.append(shared.text(path))
        # ONE pass builds the set of accessed names; every field is then an O(1) lookup.
        # Measured 2026-10-24: 127 fields x a regex over the whole corpus cost 18.5 s — 72% of
        # the governor's wall clock — and threads could not help, because it is one sequential
        # regex sweep rather than 127 independent reads.
        # Three real access shapes in this tree:
        #   `settings.compaction_threshold`  — a public field read directly
        #   `settings.settings().shell_path` — a field read off the snapshot
        #   `settings.get_default_model()`   — an accessor, the dominant shape
        # A same-named function/module/event is NOT a consumer: `edit_hybrid`, `graph_rerank`
        # and `idle_timeout_ms` all had such collisions, and every one was a false positive.
        # FOURTH access shape, found the hard way: compound accessors carry the field as a
        # SUBSTRING — `get_branch_summary_skip_prompt()` for `skip_prompt`,
        # `get_jev_graph_rerank()` for `graph_rerank`, `get_edit_hybrid_enabled()` for
        # `edit_hybrid`. So the corpus is reduced to ONE identifier set (every identifier the
        # corpus mentions), and a field counts as read when any identifier contains it. That is
        # deliberately loose: a false "read" costs a missed dead field, a false "unread" sends
        # a maintainer hunting a consumer that exists.
        accessed_names: set[str] = set()
        identifier = re_module.compile(r"\b[a-z][a-z0-9_]*\b")
        for chunk in corpus:
            accessed_names.update(identifier.findall(chunk))
        unread: list[str] = []
        for field in declared:
            result.scanned += 1
            if field in self.EXEMPT:
                continue
            if not any(field in name for name in accessed_names):
                unread.append(field)
        if unread:
            # FIFTH access shape (found the hard way): the getter-mediated
            # alias. A legacy field can be consumed ONLY through an accessor
            # that reads it inside settings/ — `get_image_model()` falls back
            # to `provider_backup_model`, `get_onboarding_shown()` to
            # `onboarding_completed`. The field identifier never appears
            # outside settings/, but the accessor's own name does. A field is
            # mediated-read when a settings accessor's body mentions it AND
            # that accessor's name is referenced in the outside corpus. The
            # accessor block split rides the module's uniform 4-space `pub fn`
            # layout; a name is only credited when it is actually referenced
            # outside, so an accessor no one calls still reports.
            manager = ctx.read("crates/pa-core/src/settings/manager.rs") or ""
            mediated: set[str] = set()
            blocks = re_module.split(r"\n    (?:pub )?fn ", manager)
            for block in blocks:
                name_match = re_module.match(r"([a-z][a-z0-9_]*)", block)
                if not name_match:
                    continue
                accessor = name_match.group(1)
                if accessor not in accessed_names:
                    continue
                for field in list(unread):
                    if field in block:
                        mediated.add(field)
                        unread.remove(field)
        for field in unread:
            result.findings.append(Finding(
                self.name, Severity.HIGH, "setting-without-consumer",
                f"settings field {field!r} is declared but read nowhere outside settings/",
                file="crates/pa-core/src/settings/types.rs"))
        return result


class ParameterSchemasValidate(Check):
    """A tool's `params_schema()` is the model's contract; it must be a valid JSON Schema shape."""

    name = "parameter-schemas-validate"
    description = "Every params_schema() returns an object schema with properties and required/types."

    TYPE_KEYWORDS = frozenset({"type", "properties", "required", "items", "enum", "anyOf", "oneOf", "allOf", "$ref"})

    def run(self, ctx: Context, *, tree=None) -> CheckResult:
        result = CheckResult(self.name)
        import re as re_module

        for path in _rust_files(ctx, tree):
            if _is_scratch(path, ctx.root):
                continue
            text = path.read_text(encoding="utf-8", errors="replace")
            for match in re_module.finditer(r"fn params_schema\b", text):
                result.scanned += 1
                rel = path.relative_to(ctx.root).as_posix()
                line = text.count("\n", 0, match.start()) + 1
                window = text[match.start():match.start() + 4000]
                if '"type"' not in window and "schema_for" not in window and "json!" not in window:
                    result.findings.append(Finding(
                        self.name, Severity.HIGH, "schema-shape-unknown",
                        "params_schema() returns no recognisable schema literal",
                        file=rel, line=line))
                elif '"object"' not in window and "schema_for" not in window:
                    result.findings.append(Finding(
                        self.name, Severity.HIGH, "schema-not-object",
                        "params_schema() is not an object schema (tools take a JSON object)",
                        file=rel, line=line))
        return result


class WorkflowSurfaceIntact(Check):
    """A CI workflow is a shipped contract too: a YAML that GitHub cannot parse runs nothing."""

    name = "workflow-surface-intact"
    description = "Each .github workflow parses, names its triggers, and every `uses:` step is pinned."

    def run(self, ctx: Context, *, tree=None) -> CheckResult:
        result = CheckResult(self.name)
        import re as re_module

        workflows = ctx.root / ".github" / "workflows"
        if not workflows.is_dir():
            return result
        for path in sorted(workflows.glob("*.y*ml")):
            result.scanned += 1
            rel = path.relative_to(ctx.root).as_posix()
            try:
                document = _load_yaml(path)
            except Exception as error:  # noqa: BLE001
                result.findings.append(Finding(
                    self.name, Severity.HIGH, "workflow-unparsable",
                    f"{rel} does not parse: {error}", file=rel))
                continue
            if not isinstance(document, dict) or not document.get("jobs"):
                result.findings.append(Finding(
                    self.name, Severity.HIGH, "workflow-no-jobs",
                    f"{rel} has no `jobs` mapping", file=rel))
                continue
            text = path.read_text(encoding="utf-8", errors="replace")
            for number, line in enumerate(text.splitlines(), start=1):
                # An unpinned third-party action is a supply-chain hole; `./` and `docker://` skip.
                if "uses:" not in line or "./" in line or "docker://" in line:
                    continue
                if not re_module.search(r"@[0-9a-f]{40}\b|@v\d+", line):
                    result.findings.append(Finding(
                        self.name, Severity.HIGH, "action-unpinned",
                        f"{rel}: workflow step is not pinned to a ref: {line.strip()[:90]}",
                        file=rel, line=number))
        return result


def _is_scratch(path: Path, root: Path) -> bool:
    """Single definition lives in `tree`; re-exported so checks read one policy."""
    from .tree import is_scratch

    return is_scratch(path, root)



class RegistryScheduleAgrees(Check):
    """Every registered check must be REACHABLE and every scheduled name must exist.

    A checker nobody schedules is dead weight that still looks like a guard (it
    is registered, its selftest passes, and it never runs in `ci`). This check is
    the one that notices: the two sides of the contract — `ALL_CHECKS` and
    `fsm.yaml`'s `run_checks` steps — must name the same set, with an explicit
    reason for any intentional exclusion.
    """

    name = "registry-schedule-agrees"
    description = "ALL_CHECKS and the FSM's run_checks steps name the same set."

    # Checks that are DELIBERATELY not scheduled, each with the reason. An entry
    # here is a decision, not an oversight: it must say why the check runs only
    # on demand.
    INTENTIONAL_EXCLUSIONS: dict[str, str] = {}

    def run(self, ctx: Context, *, tree=None) -> CheckResult:
        result = CheckResult(self.name)
        from .kinds import ALL_CHECKS as REGISTERED

        fsm_text = ctx.read("doc/testing/fsm.yaml")
        if fsm_text is None:
            result.findings.append(Finding(
                self.name, Severity.HIGH, "fsm-missing",
                "doc/testing/fsm.yaml is missing: the schedule cannot be read"))
            return result
        import yaml as _yaml
        document = _yaml.safe_load(fsm_text) or {}
        scheduled: set[str] = set()
        for state, spec in (document.get("states") or {}).items():
            if isinstance(spec, dict) and spec.get("kind") == "run_checks":
                scheduled.update(spec.get("checks") or [])
        registered = {check.name for check in REGISTERED}
        result.scanned = len(registered)
        for name in sorted(registered - scheduled):
            if name in self.INTENTIONAL_EXCLUSIONS:
                continue
            result.findings.append(Finding(
                self.name, Severity.HIGH, "check-never-scheduled",
                f"check {name!r} is registered but no FSM step runs it "
                f"(add it to a run_checks step, or state the reason in "
                f"INTENTIONAL_EXCLUSIONS)"))
        for name in sorted(scheduled - registered):
            result.findings.append(Finding(
                self.name, Severity.HIGH, "scheduled-unknown-check",
                f"fsm.yaml schedules {name!r}, which ALL_CHECKS does not define"))
        return result

ALL_CHECKS: tuple[type[Check], ...] = (
    MatrixShape,
    OracleExists,
    KillMutationPresent,
    AxisUncovered,
    GovernorRowsCarryACheck,
    PlaneCollapse,
    LayerTooCheap,
    InertGuard,
    UntrackedOracle,
    RowStaleVsDiff,
    CapabilityCoverage,
    NonRustSurfacesParse,
    SettingsHaveConsumers,
    ParameterSchemasValidate,
    WorkflowSurfaceIntact,
    RowFileMissing,
    RegistryScheduleAgrees,
)
