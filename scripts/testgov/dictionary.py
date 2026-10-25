"""Loads and validates doc/testing/test-matrix.yaml. Shape errors are HIGH, always."""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path

import yaml

MATRIX_PATH = "doc/testing/test-matrix.yaml"
MATRIX_SCHEMA = "allspark-test-matrix/v1"

PLANES = frozenset({"product", "harness", "ui"})
LAYERS = frozenset({
    "unit", "contract", "decision", "integration", "wire", "acp_e2e", "pty_e2e",
    "daemon", "artifact", "bench", "crash", "interleaving", "governor",
})
STATUSES = frozenset({"killed", "characterization", "gap", "blocked", "spec-only", "gate"})
GATES = frozenset({"default", "serial-tail", "advisory"})
AXES = frozenset({"code", "logic", "semantics", "ui", "flow", "data"})

REQUIRED = ("id", "plane", "invariant", "layer", "axes", "oracle", "kill_mutation", "status", "gate", "owner")
# `paths` is citation (which files this row is about); `axes.code` is an ontology claim (the row
# has an oracle that observes code). Conflating them made an artifact row claim it could observe
# source — a real inconsistency the governor caught in this ADR's own first matrix.
OPTIONAL = ("source", "note", "paths")

# Which layers can observe which axes. A row whose layer cannot see a named axis is
# `layer-too-cheap` — no unit test can construct two builds, no snapshot can see emptiness.
LAYER_AXIS_REACH = {
    "unit":          frozenset({"code", "logic", "semantics", "data"}),
    "decision":      frozenset({"code", "logic", "semantics", "flow"}),
    "contract":      frozenset({"code", "logic", "semantics"}),
    "integration":   frozenset({"code", "logic", "semantics", "data"}),
    "wire":          frozenset({"code", "logical", "semantics", "data", "flow"}),
    "acp_e2e":       frozenset({"code", "logic", "semantics", "flow", "data"}),
    "pty_e2e":       frozenset({"code", "logic", "semantics", "ui", "flow", "data"}),
    "daemon":        frozenset({"code", "logic", "semantics", "flow", "data"}),
    "artifact":      frozenset({"semantics", "data", "flow"}),
    "bench":         frozenset({"ui", "flow"}),
    "crash":         frozenset({"logic", "data", "flow"}),
    # A loom/model-checking run exercises ORDERINGS on purpose, so it is the one
    # cheap layer that observes `flow`. Kept separate from `unit` so a plain unit
    # row cannot claim a sequence it never runs.
    "interleaving":  frozenset({"code", "logic", "semantics", "data", "flow"}),
    "governor":      frozenset({"code", "logic", "semantics"}),
}


@dataclass
class Row:
    raw: dict
    line: int = 0

    def get(self, key: str, default=None):
        return self.raw.get(key, default)

    @property
    def id(self) -> str:
        return str(self.raw.get("id", "<missing-id>"))

    @property
    def oracles(self) -> list[str]:
        value = self.raw.get("oracle") or []
        return [str(item) for item in value]

    @property
    def axis_names(self) -> list[str]:
        axes = self.raw.get("axes") or {}
        if not isinstance(axes, dict):
            return []
        return sorted(axes)

    @property
    def paths(self) -> list[str]:
        """Files this row is about: the explicit `paths` key, else the `code` axis."""
        value = self.raw.get("paths")
        if isinstance(value, list):
            return [str(item) for item in value if isinstance(item, str)]
        axes = self.raw.get("axes") or {}
        if not isinstance(axes, dict):
            return []
        return [str(item) for item in (axes.get("code") or []) if isinstance(item, str)]


@dataclass
class Matrix:
    schema: str = ""
    rows: list[Row] = field(default_factory=list)
    error: str = ""

    @property
    def ok(self) -> bool:
        return not self.error


def load_matrix(root: Path) -> Matrix:
    """Parse the matrix. A missing file is an error the caller reports, not a crash."""
    path = root / MATRIX_PATH
    if not path.is_file():
        return Matrix(error=f"{MATRIX_PATH} is missing")
    text = path.read_text(encoding="utf-8", errors="replace")
    try:
        document = yaml.safe_load(text)
    except yaml.YAMLError as error:
        return Matrix(error=f"{MATRIX_PATH} is not valid YAML: {error}")
    if not isinstance(document, dict):
        return Matrix(error=f"{MATRIX_PATH} is not a mapping")
    rows_raw = document.get("rows")
    if not isinstance(rows_raw, list):
        return Matrix(error=f"{MATRIX_PATH} has no `rows` list")
    # Line numbers: `yaml` drops them, so a second pass finds each `- id:` marker.
    lines = text.splitlines()
    id_lines: dict[str, int] = {}
    for number, line in enumerate(lines, start=1):
        stripped = line.strip()
        if stripped.startswith("- id:"):
            id_lines[stripped.split(":", 1)[1].strip()] = number
    rows: list[Row] = []
    for entry in rows_raw:
        if not isinstance(entry, dict):
            continue
        rows.append(Row(raw=entry, line=id_lines.get(str(entry.get("id", "")), 0)))
    return Matrix(schema=str(document.get("schema", "")), rows=rows)
