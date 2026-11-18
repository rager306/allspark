//! The `meta/` definitions: entity fields, relations, dictionaries,
//! projections, and policies.
//!
//! The definitions are human-authored YAML, so every parse error names the
//! file and the field. A definition that silently degrades to a default would
//! change what the store accepts without anyone seeing it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// One field of an entity type.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldSpec {
    /// The field's wire type: `string`, `int`, `bool`, `ref`, or `json`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The dictionary a value must belong to (a `dictionaries.yaml` key).
    #[serde(default)]
    pub r#enum: Option<String>,
    /// Whether the field must be present.
    #[serde(default)]
    pub required: bool,
    /// The entity type a `ref` field points at.
    #[serde(default)]
    pub target: Option<String>,
    /// The value used when the field is absent.
    #[serde(default)]
    pub default: Option<serde_json::Value>,
}

/// One entity type in `entities.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntitySpec {
    pub fields: BTreeMap<String, FieldSpec>,
}

/// One relation in `relations.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelationSpec {
    pub from: String,
    pub to: String,
    /// `one_to_one`, `one_to_many`, `many_to_one`, or `many_to_many`.
    pub cardinality: String,
    #[serde(default)]
    pub required: bool,
    /// `restrict`, `cascade`, or `orphan`. Only `orphan` is legal across
    /// databases: no writer can make a cross-database cascade atomic.
    #[serde(default = "default_on_delete")]
    pub on_delete: String,
    /// `local` or `cross_db`.
    #[serde(default = "default_scope")]
    pub scope: String,
    /// The resolver a cross-database target needs.
    #[serde(default)]
    pub resolver: Option<String>,
}

fn default_on_delete() -> String {
    "restrict".to_string()
}

fn default_scope() -> String {
    "local".to_string()
}

/// One projection in `projections.yaml`: what enters the prompt and how much.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionSpec {
    pub entities: Vec<String>,
    #[serde(default)]
    pub fields: Vec<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    /// The prompt budget for this projection.
    #[serde(default)]
    pub budget_tokens: Option<usize>,
    /// `summary`, `artifact`, `code`, or `history`.
    #[serde(default = "default_hydrate")]
    pub hydrate: String,
}

fn default_hydrate() -> String {
    "summary".to_string()
}

/// The `completion` policy block: the types whose settled state needs proof.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionPolicy {
    /// Entity types that may not reach `completed` without an evidence ref.
    #[serde(default)]
    pub requires_evidence: Vec<String>,
    /// Entity types that may not reach a finished state without a receipt.
    #[serde(default)]
    pub requires_receipt: Vec<String>,
    /// The external facts that must hold before the run reports done.
    #[serde(default)]
    pub requires_external_fact: Vec<String>,
}

/// The `link_check` policy block.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkCheckPolicy {
    /// `validate_target` or `none`.
    #[serde(default = "default_on_write")]
    pub on_write: String,
    /// `mark_stale` or `ignore`.
    #[serde(default = "default_on_read")]
    pub on_read: String,
    /// `report_dangling` or `silent`.
    #[serde(default = "default_repair")]
    pub repair: String,
}

impl Default for LinkCheckPolicy {
    fn default() -> Self {
        Self {
            on_write: default_on_write(),
            on_read: default_on_read(),
            repair: default_repair(),
        }
    }
}

fn default_on_write() -> String {
    "validate_target".to_string()
}

fn default_on_read() -> String {
    "mark_stale".to_string()
}

fn default_repair() -> String {
    "report_dangling".to_string()
}

/// `policies.yaml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoliciesSpec {
    /// The single continuation owner order, highest priority first.
    #[serde(default)]
    pub continuation_owner: Vec<String>,
    #[serde(default)]
    pub completion: CompletionPolicy,
    #[serde(default)]
    pub link_check: LinkCheckPolicy,
}

/// One `meta/` file: its declared version plus whichever section it holds.
///
/// All five files share this shape so a stray section in the wrong file is a
/// validation error instead of a silently ignored key.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct MetaFile {
    version: u64,
    #[serde(default)]
    entities: Option<BTreeMap<String, EntitySpec>>,
    #[serde(default)]
    relations: Option<BTreeMap<String, RelationSpec>>,
    #[serde(default)]
    dictionaries: Option<BTreeMap<String, Vec<String>>>,
    #[serde(default)]
    projections: Option<BTreeMap<String, ProjectionSpec>>,
    #[serde(default)]
    continuation_owner: Option<Vec<String>>,
    #[serde(default)]
    completion: Option<CompletionPolicy>,
    #[serde(default)]
    link_check: Option<LinkCheckPolicy>,
}

impl MetaFile {
    /// The sections this file declares, by name.
    fn declared_sections(&self) -> Vec<&'static str> {
        let mut sections = Vec::new();
        if self.entities.is_some() {
            sections.push("entities");
        }
        if self.relations.is_some() {
            sections.push("relations");
        }
        if self.dictionaries.is_some() {
            sections.push("dictionaries");
        }
        if self.projections.is_some() {
            sections.push("projections");
        }
        if self.continuation_owner.is_some() {
            sections.push("continuation_owner");
        }
        if self.completion.is_some() {
            sections.push("completion");
        }
        if self.link_check.is_some() {
            sections.push("link_check");
        }
        sections
    }
}

/// The loaded `meta/` directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    /// The `meta_version` every file agreed on.
    pub version: u64,
    pub entities: BTreeMap<String, EntitySpec>,
    pub relations: BTreeMap<String, RelationSpec>,
    pub dictionaries: BTreeMap<String, Vec<String>>,
    pub projections: BTreeMap<String, ProjectionSpec>,
    pub policies: PoliciesSpec,
}

const VALID_KINDS: [&str; 5] = ["string", "int", "bool", "ref", "json"];
const VALID_CARDINALITY: [&str; 4] = ["one_to_one", "one_to_many", "many_to_one", "many_to_many"];
const VALID_ON_DELETE: [&str; 3] = ["restrict", "cascade", "orphan"];
const VALID_SCOPE: [&str; 2] = ["local", "cross_db"];
const VALID_HYDRATE: [&str; 4] = ["summary", "artifact", "code", "history"];

impl Meta {
    /// Load every file from `dir` and validate the definitions against each
    /// other.
    ///
    /// # Errors
    ///
    /// Returns an error naming the file and field when a definition cannot be
    /// parsed, when a file's `version` disagrees with another's, or when a
    /// definition refers to an entity, dictionary, or resolver that does not
    /// exist.
    pub fn load(dir: &Path) -> Result<Self> {
        // Each file holds one section. Reading them through one shape means a
        // stray `relations:` block in `entities.yaml` is caught here rather
        // than silently dropped by an unknown-key rule.
        let entities = read_file(dir, "entities.yaml", &["entities"])?;
        let relations = read_file(dir, "relations.yaml", &["relations"])?;
        let dictionaries = read_file(dir, "dictionaries.yaml", &["dictionaries"])?;
        let projections = read_file(dir, "projections.yaml", &["projections"])?;
        let policies = read_file(
            dir,
            "policies.yaml",
            &["continuation_owner", "completion", "link_check"],
        )?;

        let version = entities.version;
        for (name, found) in [
            ("relations.yaml", relations.version),
            ("dictionaries.yaml", dictionaries.version),
            ("projections.yaml", projections.version),
            ("policies.yaml", policies.version),
        ] {
            if found != version {
                bail!(
                    "{name} declares version {found}, but entities.yaml declares {version}; \
                     every meta file must agree"
                );
            }
        }

        let meta = Self {
            version,
            entities: entities.entities.unwrap_or_default(),
            relations: relations.relations.unwrap_or_default(),
            dictionaries: dictionaries.dictionaries.unwrap_or_default(),
            projections: projections.projections.unwrap_or_default(),
            policies: PoliciesSpec {
                continuation_owner: policies.continuation_owner.unwrap_or_default(),
                completion: policies.completion.unwrap_or_default(),
                link_check: policies.link_check.unwrap_or_default(),
            },
        };
        meta.validate()?;
        Ok(meta)
    }

    /// Check every cross-reference a definition can carry.
    ///
    /// # Errors
    ///
    /// Returns an error naming the offending definition and value.
    pub fn validate(&self) -> Result<()> {
        self.validate_entities()?;
        self.validate_relations()?;
        self.validate_projections()?;
        self.validate_policies()
    }

    fn validate_entities(&self) -> Result<()> {
        for (type_name, entity) in &self.entities {
            if entity.fields.is_empty() {
                bail!("entity `{type_name}` declares no fields");
            }
            for (field_name, field) in &entity.fields {
                self.validate_field(type_name, field_name, field)?;
            }
        }
        Ok(())
    }

    fn validate_field(&self, type_name: &str, field_name: &str, field: &FieldSpec) -> Result<()> {
        if !VALID_KINDS.contains(&field.kind.as_str()) {
            bail!(
                "entity `{type_name}` field `{field_name}` has unknown type `{}`: expected one \
                 of {}",
                field.kind,
                VALID_KINDS.join(", ")
            );
        }
        if let Some(dictionary) = &field.r#enum
            && !self.dictionaries.contains_key(dictionary)
        {
            bail!(
                "entity `{type_name}` field `{field_name}` names dictionary `{dictionary}`, \
                     which dictionaries.yaml does not declare"
            );
        }
        match (field.kind.as_str(), field.target.as_deref()) {
            ("ref", None) => {
                bail!("entity `{type_name}` field `{field_name}` is a ref but names no target")
            }
            ("ref", Some(target)) if !self.entities.contains_key(target) => {
                bail!("entity `{type_name}` field `{field_name}` targets unknown entity `{target}`")
            }
            ("ref", Some(_)) | (_, None) => Ok(()),
            (_, Some(target)) => bail!(
                "entity `{type_name}` field `{field_name}` names target `{target}` but is typed \
                 `{}`, not `ref`",
                field.kind
            ),
        }
    }

    fn validate_relations(&self) -> Result<()> {
        for (name, relation) in &self.relations {
            self.validate_relation(name, relation)?;
        }
        Ok(())
    }

    fn validate_relation(&self, name: &str, relation: &RelationSpec) -> Result<()> {
        for (side, entity) in [("from", &relation.from), ("to", &relation.to)] {
            if !self.entities.contains_key(entity) {
                bail!(
                    "relation `{name}` has {side} entity `{entity}`, which entities.yaml does \
                     not declare"
                );
            }
        }
        for (label, value, valid) in [
            (
                "cardinality",
                relation.cardinality.as_str(),
                VALID_CARDINALITY.as_slice(),
            ),
            (
                "on_delete",
                relation.on_delete.as_str(),
                VALID_ON_DELETE.as_slice(),
            ),
            ("scope", relation.scope.as_str(), VALID_SCOPE.as_slice()),
        ] {
            if !valid.contains(&value) {
                bail!(
                    "relation `{name}` has unknown {label} `{value}`: expected one of {}",
                    valid.join(", ")
                );
            }
        }
        if relation.scope != "cross_db" {
            return Ok(());
        }
        // No writer can make a cross-database delete atomic, so a cascade there
        // would promise a guarantee the store cannot keep.
        if relation.on_delete != "orphan" {
            bail!(
                "relation `{name}` is cross_db with on_delete `{}`; only `orphan` is legal \
                 across databases because no writer can make it atomic",
                relation.on_delete
            );
        }
        if relation.resolver.as_deref().is_none_or(str::is_empty) {
            bail!("relation `{name}` is cross_db but names no resolver");
        }
        Ok(())
    }

    fn validate_projections(&self) -> Result<()> {
        for (name, projection) in &self.projections {
            if projection.entities.is_empty() {
                bail!("projection `{name}` names no entities");
            }
            for entity in &projection.entities {
                if !self.entities.contains_key(entity) {
                    bail!(
                        "projection `{name}` names entity `{entity}`, which entities.yaml does \
                         not declare"
                    );
                }
            }
            if !VALID_HYDRATE.contains(&projection.hydrate.as_str()) {
                bail!(
                    "projection `{name}` has unknown hydrate `{}`: expected one of {}",
                    projection.hydrate,
                    VALID_HYDRATE.join(", ")
                );
            }
            if projection.budget_tokens == Some(0) {
                bail!("projection `{name}` declares a zero token budget");
            }
        }
        Ok(())
    }

    fn validate_policies(&self) -> Result<()> {
        for entity in self
            .policies
            .completion
            .requires_evidence
            .iter()
            .chain(&self.policies.completion.requires_receipt)
        {
            if !self.entities.contains_key(entity) {
                bail!(
                    "policies.yaml names entity `{entity}`, which entities.yaml does not declare"
                );
            }
        }
        // Duplicate continuation owners would make the priority order
        // ambiguous exactly where it must be single.
        let mut seen = BTreeSet::new();
        for owner in &self.policies.continuation_owner {
            if !seen.insert(owner.as_str()) {
                bail!("policies.yaml lists continuation owner `{owner}` twice");
            }
        }
        Ok(())
    }
}

fn read_file(dir: &Path, name: &str, allowed: &[&str]) -> Result<MetaFile> {
    let path = dir.join(name);
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let file: MetaFile =
        serde_yaml::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    let declared = file.declared_sections();
    if declared.is_empty() {
        bail!(
            "{} declares no section; expected one of {}",
            path.display(),
            allowed.join(", ")
        );
    }
    for section in declared {
        if !allowed.contains(&section) {
            bail!(
                "{} declares `{section}`, which does not belong in this file; expected {}",
                path.display(),
                allowed.join(", ")
            );
        }
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_meta(dir: &std::path::Path, files: &[(&str, &str)]) {
        for (name, body) in files {
            std::fs::write(dir.join(name), body).unwrap();
        }
    }

    const ENTITIES: &str = "\
version: 1
entities:
  plan_item:
    fields:
      title: { type: string, required: true }
      status: { type: string, enum: plan_status, default: pending }
      evidence_id: { type: ref, target: evidence }
  evidence:
    fields:
      summary: { type: string, required: true }
";

    const RELATIONS: &str = "\
version: 1
relations:
  item_has_evidence:
    from: plan_item
    to: evidence
    cardinality: many_to_one
";

    const DICTIONARIES: &str = "\
version: 1
dictionaries:
  plan_status: [pending, in_progress, completed, blocked]
";

    const PROJECTIONS: &str = "\
version: 1
projections:
  plan_context:
    entities: [plan_item]
    limit: 5
    budget_tokens: 800
";

    const POLICIES: &str = "\
version: 1
continuation_owner: [goal, queued_input, compaction, plan]
completion:
  requires_evidence: [plan_item]
";

    fn valid_files() -> [(&'static str, &'static str); 5] {
        [
            ("entities.yaml", ENTITIES),
            ("relations.yaml", RELATIONS),
            ("dictionaries.yaml", DICTIONARIES),
            ("projections.yaml", PROJECTIONS),
            ("policies.yaml", POLICIES),
        ]
    }

    #[test]
    fn a_complete_meta_directory_loads() {
        let dir = tempfile::tempdir().unwrap();
        write_meta(dir.path(), &valid_files());

        let meta = Meta::load(dir.path()).expect("the definitions are valid");
        assert_eq!(meta.version, 1);
        assert_eq!(meta.entities.len(), 2);
        assert!(meta.entities["plan_item"].fields["title"].required);
        assert_eq!(
            meta.entities["plan_item"].fields["status"]
                .r#enum
                .as_deref(),
            Some("plan_status")
        );
        assert_eq!(meta.relations["item_has_evidence"].to, "evidence");
        assert_eq!(meta.dictionaries["plan_status"].len(), 4);
        assert_eq!(meta.projections["plan_context"].limit, Some(5));
        assert_eq!(
            meta.policies.completion.requires_evidence,
            vec!["plan_item".to_string()]
        );
        assert_eq!(meta.policies.continuation_owner.len(), 4);
    }

    #[test]
    fn a_mismatched_version_names_the_file() {
        let dir = tempfile::tempdir().unwrap();
        write_meta(
            dir.path(),
            &[
                ("entities.yaml", ENTITIES),
                ("relations.yaml", RELATIONS),
                ("dictionaries.yaml", DICTIONARIES),
                ("projections.yaml", "version: 2\nprojections: {}\n"),
                ("policies.yaml", POLICIES),
            ],
        );

        let error = Meta::load(dir.path()).unwrap_err().to_string();
        assert!(error.contains("projections.yaml"), "{error}");
        assert!(error.contains("version 2"), "{error}");
    }

    #[test]
    fn a_cross_database_cascade_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        write_meta(
            dir.path(),
            &[
                ("entities.yaml", ENTITIES),
                (
                    "relations.yaml",
                    "\
version: 1
relations:
  evidence_about_code:
    from: evidence
    to: plan_item
    cardinality: many_to_one
    scope: cross_db
    on_delete: cascade
    resolver: code_ref
",
                ),
                ("dictionaries.yaml", DICTIONARIES),
                ("projections.yaml", PROJECTIONS),
                ("policies.yaml", POLICIES),
            ],
        );

        let error = Meta::load(dir.path()).unwrap_err().to_string();
        assert!(error.contains("only `orphan` is legal"), "{error}");
    }

    #[test]
    fn a_cross_database_relation_without_a_resolver_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        write_meta(
            dir.path(),
            &[
                ("entities.yaml", ENTITIES),
                (
                    "relations.yaml",
                    "\
version: 1
relations:
  evidence_about_code:
    from: evidence
    to: plan_item
    cardinality: many_to_one
    scope: cross_db
    on_delete: orphan
",
                ),
                ("dictionaries.yaml", DICTIONARIES),
                ("projections.yaml", PROJECTIONS),
                ("policies.yaml", POLICIES),
            ],
        );

        let error = Meta::load(dir.path()).unwrap_err().to_string();
        assert!(error.contains("names no resolver"), "{error}");
    }

    #[test]
    fn an_unknown_dictionary_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        write_meta(
            dir.path(),
            &[
                (
                    "entities.yaml",
                    &ENTITIES.replace("enum: plan_status", "enum: missing_status"),
                ),
                ("relations.yaml", RELATIONS),
                ("dictionaries.yaml", DICTIONARIES),
                ("projections.yaml", PROJECTIONS),
                ("policies.yaml", POLICIES),
            ],
        );

        let error = Meta::load(dir.path()).unwrap_err().to_string();
        assert!(error.contains("missing_status"), "{error}");
    }

    #[test]
    fn a_ref_without_a_target_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        write_meta(
            dir.path(),
            &[
                (
                    "entities.yaml",
                    "\
version: 1
entities:
  plan_item:
    fields:
      evidence_id: { type: ref }
",
                ),
                ("relations.yaml", RELATIONS),
                ("dictionaries.yaml", DICTIONARIES),
                ("projections.yaml", PROJECTIONS),
                ("policies.yaml", POLICIES),
            ],
        );

        let error = Meta::load(dir.path()).unwrap_err().to_string();
        assert!(error.contains("names no target"), "{error}");
    }

    #[test]
    fn a_section_in_the_wrong_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        write_meta(
            dir.path(),
            &[
                (
                    "entities.yaml",
                    &format!(
                        "{ENTITIES}relations:\n  stray:\n    from: plan_item\n    to: evidence\n    cardinality: many_to_one\n"
                    ),
                ),
                ("relations.yaml", RELATIONS),
                ("dictionaries.yaml", DICTIONARIES),
                ("projections.yaml", PROJECTIONS),
                ("policies.yaml", POLICIES),
            ],
        );

        let error = Meta::load(dir.path()).unwrap_err().to_string();
        assert!(error.contains("does not belong in this file"), "{error}");
    }

    #[test]
    fn a_duplicate_continuation_owner_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        write_meta(
            dir.path(),
            &[
                ("entities.yaml", ENTITIES),
                ("relations.yaml", RELATIONS),
                ("dictionaries.yaml", DICTIONARIES),
                ("projections.yaml", PROJECTIONS),
                (
                    "policies.yaml",
                    "version: 1\ncontinuation_owner: [goal, goal]\n",
                ),
            ],
        );

        let error = Meta::load(dir.path()).unwrap_err().to_string();
        assert!(error.contains("twice"), "{error}");
    }

    #[test]
    fn a_projection_over_an_unknown_entity_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        write_meta(
            dir.path(),
            &[
                ("entities.yaml", ENTITIES),
                ("relations.yaml", RELATIONS),
                ("dictionaries.yaml", DICTIONARIES),
                (
                    "projections.yaml",
                    "version: 1\nprojections:\n  bad:\n    entities: [ghost]\n",
                ),
                ("policies.yaml", POLICIES),
            ],
        );

        let error = Meta::load(dir.path()).unwrap_err().to_string();
        assert!(error.contains("ghost"), "{error}");
    }

    #[test]
    fn an_entity_without_fields_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        write_meta(
            dir.path(),
            &[
                (
                    "entities.yaml",
                    "version: 1\nentities:\n  empty:\n    fields: {}\n",
                ),
                ("relations.yaml", RELATIONS),
                ("dictionaries.yaml", DICTIONARIES),
                ("projections.yaml", PROJECTIONS),
                ("policies.yaml", POLICIES),
            ],
        );

        let error = Meta::load(dir.path()).unwrap_err().to_string();
        assert!(error.contains("declares no fields"), "{error}");
    }
}
