//! Project context files (AGENTS.md/CLAUDE.md discovery) and the resource
//! loader: skills, prompt templates, agents files, and system-prompt
//! sources, resolved from configured packages, settings, auto-discovery,
//! and bundled skills through the package manager. Theme loading lives in pa-tui.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::skills::diagnostics::ResourceDiagnostic;
use crate::skills::{
    load_prompt_templates, load_skills, LoadPromptTemplatesOptions, LoadSkillsOptions,
    PromptTemplate, Skill,
};

/// AGENTS.md-family candidates, in priority order.
const CONTEXT_FILE_CANDIDATES: [&str; 4] = ["AGENTS.md", "AGENTS.MD", "CLAUDE.md", "CLAUDE.MD"];

/// Largest context file (AGENTS.md family) admitted into the prompt, in bytes.
///
/// A context file is injected verbatim into the system prompt, so its size is
/// unbounded model input: a 10 MB AGENTS.md (accidental log dump, generated
/// file, or a hostile repo) turns into millions of tokens and fails the turn
/// before the model ever reads it. The cap is generous — 256 KiB is far more
/// than any real instruction file — and truncation is reported, never silent.
pub const MAX_CONTEXT_FILE_BYTES: usize = 256 * 1024;

/// Truncate `content` to at most `limit` bytes on a char boundary.
///
/// Returns the kept text and whether anything was dropped. Cutting on a char
/// boundary matters: a byte slice through a multi-byte scalar would panic or
/// emit a replacement char into the prompt.
#[must_use]
pub fn truncate_context_content(content: &str, limit: usize) -> (String, bool) {
    if content.len() <= limit {
        return (content.to_string(), false);
    }
    let mut end = limit;
    while end > 0 && !content.is_char_boundary(end) {
        end -= 1;
    }
    (content[..end].to_string(), true)
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContextFile {
    pub path: PathBuf,
    pub content: String,
}

fn load_context_file_from_dir(dir: &Path) -> Option<ContextFile> {
    for filename in CONTEXT_FILE_CANDIDATES {
        let file_path = dir.join(filename);
        if let Ok(content) = std::fs::read_to_string(&file_path) {
            // Cap before the content ever reaches the prompt: the file is
            // untrusted input from the working tree.
            let (content, _truncated) = truncate_context_content(&content, MAX_CONTEXT_FILE_BYTES);
            return Some(ContextFile {
                path: file_path,
                content,
            });
        }
    }
    None
}

/// Load the global (agentDir) context file first, then the nearest-to-root ancestor chain of cwd.
#[must_use]
pub fn load_project_context_files(cwd: &Path, agent_dir: &Path) -> Vec<ContextFile> {
    let mut context_files = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    if let Some(global) = load_context_file_from_dir(agent_dir) {
        seen.insert(global.path.clone());
        context_files.push(global);
    }

    // Walk cwd upward, collecting context files; root-most first in output.
    let mut ancestors = Vec::new();
    let mut current = cwd.to_path_buf();
    loop {
        if let Some(found) = load_context_file_from_dir(&current) {
            if seen.insert(found.path.clone()) {
                ancestors.push(found);
            }
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if parent == current {
            break;
        }
        current = parent.to_path_buf();
    }
    ancestors.reverse();
    context_files.extend(ancestors);
    context_files
}

// Session resource resolution (package-manager `resolve()`) feeds the loader below.
pub(crate) mod resolution;

use anyhow::Result;

use crate::packages::BundledSkillsDir;
use crate::settings::SettingsManager;

#[derive(Debug, Default)]
pub struct LoadedResources {
    pub skills: Vec<Skill>,
    pub skill_diagnostics: Vec<ResourceDiagnostic>,
    pub prompts: Vec<PromptTemplate>,
    pub agents_files: Vec<ContextFile>,
    pub system_prompt: Option<String>,
    pub append_system_prompt: Vec<String>,
}

/// Resource loading options (the TS `DefaultResourceLoaderOptions` surface, minus the theme
/// machinery).
#[derive(Default)]
pub struct ResourceLoaderOptions {
    pub cwd: PathBuf,
    pub agent_dir: PathBuf,
    /// Settings manager for package resolution; loaded from disk when `None`.
    pub settings: Option<SettingsManager>,
    /// Built-in skills directory (default: the packaged layout).
    pub bundled_skills_dir: BundledSkillsDir,
    /// Extra force-exclude patterns for built-in skills.
    pub extra_builtin_skill_overrides: Vec<String>,
    pub additional_skill_paths: Vec<String>,
    pub additional_prompt_paths: Vec<String>,
    pub no_skills: bool,
    pub no_prompt_templates: bool,
    pub no_context_files: bool,
    pub system_prompt: Option<String>,
    pub append_system_prompt: Vec<String>,
}

impl ResourceLoaderOptions {
    /// Options with just the working directories set.
    pub fn new(cwd: impl Into<PathBuf>, agent_dir: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            agent_dir: agent_dir.into(),
            ..Default::default()
        }
    }
}

/// Load all session resources: package-manager resolution (configured
/// packages, settings arrays, auto-discovery, bundled skills) feeds the
/// enabled skill and prompt paths.
///
/// # Errors
///
/// Error when a configured package source cannot be parsed, installed, or refreshed.
pub fn load_resources(mut options: ResourceLoaderOptions) -> Result<LoadedResources> {
    let settings = options
        .settings
        .take()
        .unwrap_or_else(|| SettingsManager::create(&options.cwd, &options.agent_dir));
    let resolution = resolution::resolve_session_resources(&options, settings)?;

    let mut resources = LoadedResources::default();

    // Skills: CLI paths + additional paths first, then resolved paths in
    // precedence order; name collisions resolve first-wins in load order.
    let skills_result = if options.no_skills && resolution.skill_paths.is_empty() {
        crate::skills::LoadSkillsResult {
            skills: Vec::new(),
            diagnostics: Vec::new(),
        }
    } else {
        load_skills(&LoadSkillsOptions {
            cwd: options.cwd.clone(),
            agent_dir: options.agent_dir.clone(),
            skill_paths: resolution.skill_paths.clone(),
            include_defaults: false,
        })
    };
    resources.skills = skills_result
        .skills
        .into_iter()
        .map(|mut skill| {
            if let Some(source_info) =
                resolution::find_source_info(&resolution.source_infos, &skill.file_path)
            {
                skill.source_info = source_info;
            }
            skill
        })
        .collect();
    resources.skill_diagnostics = skills_result.diagnostics;
    // Resolution-time warnings (e.g. missing bundled skills directory).
    resources
        .skill_diagnostics
        .extend(resolution.resolved.diagnostics.clone());
    for path in &options.additional_skill_paths {
        if is_local_path(path)
            && !Path::new(path).exists()
            && !resources
                .skill_diagnostics
                .iter()
                .any(|d| diagnostics_path(d) == Some(path.clone()))
        {
            resources.skill_diagnostics.push(ResourceDiagnostic::Error {
                message: "Skill path does not exist".to_string(),
                path: Some(path.clone()),
            });
        }
    }

    // Prompt templates: CLI paths + resolved paths, then additional paths.
    let all_prompts = if options.no_prompt_templates && resolution.prompt_paths.is_empty() {
        Vec::new()
    } else {
        load_prompt_templates(&LoadPromptTemplatesOptions {
            cwd: options.cwd.clone(),
            agent_dir: options.agent_dir.clone(),
            prompt_paths: resolution.prompt_paths.clone(),
            include_defaults: false,
        })
    };
    resources.prompts = dedupe_prompts(all_prompts)
        .into_iter()
        .map(|mut prompt| {
            if let Some(source_info) =
                resolution::find_source_info(&resolution.source_infos, Path::new(&prompt.file_path))
            {
                prompt.source_info = source_info;
            }
            prompt
        })
        .collect();
    for path in &options.additional_prompt_paths {
        if is_local_path(path) && !Path::new(path).exists() {
            resources.skill_diagnostics.push(ResourceDiagnostic::Error {
                message: "Prompt template path does not exist".to_string(),
                path: Some(path.clone()),
            });
        }
    }

    // Agents files (project context).
    if !options.no_context_files {
        resources.agents_files = load_project_context_files(&options.cwd, &options.agent_dir);
    }

    // System prompt: explicit source or discovered file content.
    let system_source = options.system_prompt.clone().or_else(|| {
        discover_system_prompt_file(&options.cwd, &options.agent_dir)
            .map(|p| p.to_string_lossy().to_string())
    });
    resources.system_prompt = system_source.and_then(|source| resolve_prompt_input(&source));

    // Append system prompt: explicit sources or discovered file.
    let append_sources: Vec<String> = if options.append_system_prompt.is_empty() {
        discover_append_system_prompt_file(&options.cwd, &options.agent_dir)
            .map(|path| path.to_string_lossy().to_string())
            .into_iter()
            .collect()
    } else {
        options.append_system_prompt.clone()
    };
    resources.append_system_prompt = append_sources
        .iter()
        .filter_map(|source| resolve_prompt_input(source))
        .collect();

    Ok(resources)
}

fn diagnostics_path(diagnostic: &ResourceDiagnostic) -> Option<String> {
    match diagnostic {
        ResourceDiagnostic::Warning { path, .. } | ResourceDiagnostic::Error { path, .. } => {
            path.clone()
        }
        ResourceDiagnostic::Collision { path, .. } => Some(path.clone()),
    }
}

fn resolve_prompt_input(input: &str) -> Option<String> {
    let path = Path::new(input);
    if path.exists() {
        // Same cap as a context file: this text becomes the system prompt or
        // its appended tail, so an oversized file is the same denial of
        // service by another route.
        let content = std::fs::read_to_string(path).ok()?;
        let (content, _truncated) = truncate_context_content(&content, MAX_CONTEXT_FILE_BYTES);
        return Some(content);
    }
    let (content, _truncated) = truncate_context_content(input, MAX_CONTEXT_FILE_BYTES);
    Some(content)
}

fn is_local_path(path: &str) -> bool {
    let trimmed = path.trim();
    !(trimmed.starts_with("npm:")
        || trimmed.starts_with("git:")
        || trimmed.starts_with("http://")
        || trimmed.starts_with("https://"))
}

/// Later same-name templates win? No: TS dedupe keeps the FIRST of each name.
fn dedupe_prompts(prompts: Vec<PromptTemplate>) -> Vec<PromptTemplate> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for prompt in prompts {
        if seen.insert(prompt.name.clone()) {
            out.push(prompt);
        }
    }
    out
}

/// `cwd/{CONFIG_DIR_NAME}/SYSTEM.md` then agentDir/SYSTEM.md.
#[must_use]
pub fn discover_system_prompt_file(cwd: &Path, agent_dir: &Path) -> Option<PathBuf> {
    let project = cwd.join(crate::settings::CONFIG_DIR_NAME).join("SYSTEM.md");
    if project.exists() {
        return Some(project);
    }
    let global = agent_dir.join("SYSTEM.md");
    if global.exists() {
        return Some(global);
    }
    None
}

/// `cwd/{CONFIG_DIR_NAME}/APPEND_SYSTEM.md` then `agentDir/APPEND_SYSTEM.md`.
#[must_use]
pub fn discover_append_system_prompt_file(cwd: &Path, agent_dir: &Path) -> Option<PathBuf> {
    let project = cwd
        .join(crate::settings::CONFIG_DIR_NAME)
        .join("APPEND_SYSTEM.md");
    if project.exists() {
        return Some(project);
    }
    let global = agent_dir.join("APPEND_SYSTEM.md");
    if global.exists() {
        return Some(global);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn context_files_global_then_ancestors_root_first() {
        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agent");
        let deep = tmp.path().join("a").join("b");
        fs::create_dir_all(&agent_dir).unwrap();
        fs::create_dir_all(&deep).unwrap();
        fs::write(agent_dir.join("AGENTS.md"), "global").unwrap();
        fs::write(tmp.path().join("a").join("AGENTS.md"), "level-a").unwrap();
        fs::write(deep.join("AGENTS.md"), "deep").unwrap();
        let files = load_project_context_files(&deep, &agent_dir);
        let contents: Vec<&str> = files.iter().map(|f| f.content.as_str()).collect();
        assert_eq!(contents, vec!["global", "level-a", "deep"]);
    }

    #[test]
    fn system_prompt_discovery_prefers_project() {
        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agent");
        let project = tmp.path().join("proj");
        fs::create_dir_all(agent_dir.join(".prime").join("agent")).unwrap();
        fs::create_dir_all(project.join(".prime").join("agent")).unwrap();
        fs::write(
            agent_dir.join(".prime").join("agent").join("SYSTEM.md"),
            "global system",
        )
        .unwrap();
        fs::write(
            project.join(".prime").join("agent").join("SYSTEM.md"),
            "project system",
        )
        .unwrap();
        let found = discover_system_prompt_file(&project, &agent_dir).unwrap();
        assert!(found.to_string_lossy().contains("proj"));
        assert_eq!(
            resolve_prompt_input(&found.to_string_lossy()),
            Some("project system".to_string())
        );
    }

    #[test]
    fn resources_load_end_to_end() {
        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agent");
        let cwd = tmp.path().join("proj");
        fs::create_dir_all(agent_dir.join("skills").join("alpha")).unwrap();
        fs::create_dir_all(cwd.join(".prime").join("agent").join("prompts")).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::write(
            agent_dir.join("skills").join("alpha").join("SKILL.md"),
            "---\nname: alpha\ndescription: Alpha skill\n---\nbody",
        )
        .unwrap();
        fs::write(
            cwd.join(".prime")
                .join("agent")
                .join("prompts")
                .join("fix.md"),
            "Fix $1",
        )
        .unwrap();
        fs::write(cwd.join("AGENTS.md"), "Project rules").unwrap();
        fs::write(
            cwd.join(".prime").join("agent").join("SYSTEM.md"),
            "custom system",
        )
        .unwrap();
        let resources = load_resources(ResourceLoaderOptions {
            cwd,
            agent_dir,
            ..Default::default()
        })
        .unwrap();
        assert!(resources.skills.iter().any(|s| s.name == "alpha"));
        assert!(resources.prompts.iter().any(|p| p.name == "fix"));
        assert_eq!(resources.agents_files.len(), 1);
        assert_eq!(resources.agents_files[0].content, "Project rules");
        assert_eq!(resources.system_prompt.as_deref(), Some("custom system"));
        assert!(resources.append_system_prompt.is_empty());
    }

    #[test]
    fn package_provided_skills_and_prompts_load_into_a_session() {
        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agent");
        let cwd = tmp.path().join("proj");
        fs::create_dir_all(&agent_dir).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        let pkg = tmp.path().join("fixture-pkg");
        fs::create_dir_all(pkg.join("skills").join("pack-skill")).unwrap();
        fs::create_dir_all(pkg.join("prompts")).unwrap();
        fs::write(
            pkg.join("package.json"),
            r#"{"name":"fixture-pkg","version":"1.0.0"}"#,
        )
        .unwrap();
        fs::write(
            pkg.join("skills").join("pack-skill").join("SKILL.md"),
            "---\nname: pack-skill\ndescription: From the package\n---\nBody",
        )
        .unwrap();
        fs::write(pkg.join("prompts").join("fix.md"), "Fix $1").unwrap();
        fs::write(
            agent_dir.join("settings.json"),
            serde_json::json!({"packages": ["../fixture-pkg"]}).to_string(),
        )
        .unwrap();

        let resources = load_resources(ResourceLoaderOptions::new(&cwd, &agent_dir)).unwrap();
        let skill = resources
            .skills
            .iter()
            .find(|skill| skill.name == "pack-skill")
            .expect("package skill loaded");
        assert_eq!(skill.source_info.source, "../fixture-pkg");
        assert_eq!(
            skill.source_info.origin,
            crate::skills::SourceOrigin::Package
        );
        let prompt = resources
            .prompts
            .iter()
            .find(|prompt| prompt.name == "fix")
            .expect("package prompt loaded");
        assert_eq!(prompt.source_info.source, "../fixture-pkg");
    }

    #[test]
    fn project_auto_skills_win_name_collisions_over_user_auto() {
        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agent");
        let cwd = tmp.path().join("proj");
        fs::create_dir_all(agent_dir.join("skills").join("alpha")).unwrap();
        fs::create_dir_all(
            cwd.join(".prime")
                .join("agent")
                .join("skills")
                .join("alpha"),
        )
        .unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::write(
            agent_dir.join("skills").join("alpha").join("SKILL.md"),
            "---\nname: alpha\ndescription: User alpha\n---\nBody",
        )
        .unwrap();
        fs::write(
            cwd.join(".prime")
                .join("agent")
                .join("skills")
                .join("alpha")
                .join("SKILL.md"),
            "---\nname: alpha\ndescription: Project alpha\n---\nBody",
        )
        .unwrap();

        let resources = load_resources(ResourceLoaderOptions::new(&cwd, &agent_dir)).unwrap();
        let alpha: Vec<_> = resources
            .skills
            .iter()
            .filter(|skill| skill.name == "alpha")
            .collect();
        assert_eq!(alpha.len(), 1, "same name resolves to one skill");
        assert_eq!(alpha[0].description, "Project alpha");
        assert_eq!(
            alpha[0].source_info.scope,
            crate::skills::SourceScope::Project
        );
        assert!(resources
            .skill_diagnostics
            .iter()
            .any(|d| matches!(d, ResourceDiagnostic::Collision { .. })));
    }

    /// A context file is untrusted repo content injected verbatim into the
    /// prompt, so its size must be bounded. A 10 MB AGENTS.md (an accidental
    /// log dump, a generated file, or a hostile repository) used to become
    /// millions of tokens and fail the turn before the model ran.
    #[test]
    fn an_oversized_context_file_is_truncated() {
        let tmp = tempfile::tempdir().unwrap();
        let huge = "A".repeat(10_000_000);
        std::fs::write(tmp.path().join("AGENTS.md"), &huge).unwrap();

        let file = load_context_file_from_dir(tmp.path()).expect("the file is found");
        assert_eq!(file.content.len(), MAX_CONTEXT_FILE_BYTES);
        assert!(
            file.content.bytes().all(|byte| byte == b'A'),
            "truncation must not corrupt the retained bytes"
        );
    }

    /// A file under the cap passes through byte-identical.
    #[test]
    fn a_normal_context_file_is_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("AGENTS.md"), "Project rules").unwrap();
        let file = load_context_file_from_dir(tmp.path()).expect("the file is found");
        assert_eq!(file.content, "Project rules");
    }

    /// Truncation cuts on a char boundary: slicing through a multi-byte scalar
    /// would panic or inject a replacement character into the prompt.
    #[test]
    fn truncation_respects_char_boundaries() {
        // Each `ж` is two bytes, so a 7-byte limit lands mid-scalar.
        let text = "жжжжж";
        let (kept, truncated) = truncate_context_content(text, 7);
        assert!(truncated);
        assert_eq!(kept, "жжж");
        assert_eq!(kept.len(), 6);
        // A limit that lands exactly on a boundary keeps the full scalar.
        let (kept, truncated) = truncate_context_content(text, 8);
        assert!(truncated);
        assert_eq!(kept, "жжжж");
    }

    /// The cap is a ceiling, not a target: content at or under it is returned
    /// unchanged and reported as not truncated.
    #[test]
    fn truncation_is_a_no_op_at_or_below_the_limit() {
        let text = "A".repeat(MAX_CONTEXT_FILE_BYTES);
        let (kept, truncated) = truncate_context_content(&text, MAX_CONTEXT_FILE_BYTES);
        assert!(!truncated);
        assert_eq!(kept.len(), MAX_CONTEXT_FILE_BYTES);

        let (kept, truncated) = truncate_context_content("short", MAX_CONTEXT_FILE_BYTES);
        assert!(!truncated);
        assert_eq!(kept, "short");
    }

    /// A `SYSTEM.md` / `APPEND_SYSTEM.md` path is prompt input too, and must be
    /// capped by the same rule.
    #[test]
    fn an_oversized_prompt_input_file_is_truncated() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("SYSTEM.md");
        std::fs::write(&path, "B".repeat(5_000_000)).unwrap();
        let content = resolve_prompt_input(path.to_str().unwrap()).expect("resolved");
        assert_eq!(content.len(), MAX_CONTEXT_FILE_BYTES);
    }
}
