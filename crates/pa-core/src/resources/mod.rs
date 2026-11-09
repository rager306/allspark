//! Project context files (AGENTS.md/CLAUDE.md discovery) and the resource
//! loader: skills, prompt templates, agents files, and system-prompt
//! sources, resolved from configured packages, settings, auto-discovery,
//! and bundled skills through the package manager. Theme loading lives in pa-tui.

use std::collections::HashSet;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::skills::diagnostics::ResourceDiagnostic;
use crate::skills::{
    load_prompt_templates, load_skills, LoadPromptTemplatesOptions, LoadSkillsOptions,
    PromptTemplate, Skill, SkillKind, SourceScope,
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

/// Truncate `content` and MARK the cut in the text the model receives.
///
/// The flag `truncate_context_content` returns was discarded at every prompt-input
/// callsite, so an oversized AGENTS.md, `--system-prompt` file, per-model prompt layer or
/// `/skill` body reached the model as if the author had written it that way (review #2). The
/// contract in AGENTS.md is explicit — "must reuse `truncate_context_content` ... and report
/// the truncation" — and a marker in the text is the only place the MODEL can see it; a
/// diagnostic struct that never reaches the prompt reports to a log, not to the reader.
#[must_use]
pub fn truncate_context_content_marked(content: &str, limit: usize, what: &str) -> String {
    capped_and_marked(content, limit, what, content.len() as u64)
}

/// Cap `raw` at `limit` bytes and build the marker, whose total is the SOURCE size.
///
/// The two numbers differ once the read itself is bounded: `raw` then ends at the cap and
/// `source_bytes` is the whole file, which is the only size worth telling the model (review #4).
fn capped_and_marked(raw: &str, limit: usize, what: &str, source_bytes: u64) -> String {
    let (kept, cut) = truncate_context_content(raw, limit);
    if !cut && source_bytes <= limit as u64 {
        return kept;
    }
    let total = source_bytes.max(kept.len() as u64);
    format!(
        "{kept}\n\n[... {what} TRUNCATED at {limit} bytes of {total} — the rest is NOT in this \
         prompt ...]"
    )
}

/// Longest UTF-8 scalar, in bytes: the slack the bounded read takes past the cap so a scalar
/// the cap cuts through is not mistaken for corrupt input.
const MAX_SCALAR_BYTES: u64 = 4;

/// Read a prompt-input file, capped at `MAX_CONTEXT_FILE_BYTES`, never a special file.
///
/// Two bounds live here, and both are about a file the working tree controls.
///
/// **Size.** Every caller used to read to EOF and truncate afterwards, so the cap bounded the
/// prompt but not the read: a 1 TiB AGENTS.md still tried to buffer 1 TiB, and a file whose
/// bytes past the cap are not UTF-8 was dropped as unreadable even though none of it would
/// have reached the model (review #4).
///
/// **Type.** Opening a FIFO read-only waits for a writer, and these paths come from the
/// working tree and the agent directory, so `mkfifo AGENTS.md` used to hang session start
/// (review #29). The open is therefore the guarded step, not a `metadata` call before it: a
/// stat on a pipe returns at once but the open that follows it blocks, so `O_NONBLOCK` belongs
/// on the open, and the type check rides on the handle it returns.
///
/// Returns the kept text and whether the file was longer than the cap, so a caller that only
/// needs the text as data (frontmatter, walk rules) can still report the cut.
///
/// # Errors
///
/// The open error; `InvalidInput` for anything that is not a regular file; and `InvalidData`
/// when the bytes that would be kept are not UTF-8.
pub fn read_prompt_input_bounded(path: &Path) -> io::Result<(String, bool)> {
    let (text, size) = read_prompt_input(path)?;
    let (kept, cut) = truncate_context_content(&text, MAX_CONTEXT_FILE_BYTES);
    Ok((kept, cut || size > MAX_CONTEXT_FILE_BYTES as u64))
}

/// [`read_prompt_input_bounded`], capped and marked for the prompt.
///
/// # Errors
///
/// As [`read_prompt_input_bounded`].
pub fn read_prompt_input_marked(path: &Path, what: &str) -> io::Result<String> {
    let (text, size) = read_prompt_input(path)?;
    Ok(capped_and_marked(&text, MAX_CONTEXT_FILE_BYTES, what, size))
}

/// The read both public helpers share: `(text up to the cap, the file's real size)`.
fn read_prompt_input(path: &Path) -> io::Result<(String, u64)> {
    let file = open_regular_file(path)?;
    let size = file.metadata()?.len();
    let bytes = read_bounded(file)?;
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        // A scalar straddling the cap is the one tolerable UTF-8 error: those bytes were never
        // going to be kept. Anything invalid inside the kept prefix refuses the file exactly as
        // an unbounded `read_to_string` would have.
        Err(error) => {
            let valid = error.utf8_error().valid_up_to();
            if valid < MAX_CONTEXT_FILE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "stream did not contain valid UTF-8",
                ));
            }
            String::from_utf8(error.into_bytes()[..valid].to_vec())
                .expect("the prefix up to valid_up_to is UTF-8")
        }
    };
    Ok((text, size))
}

/// Read at most the cap plus one UTF-8 scalar's slack, and no more.
///
/// A separate function so the BOUND is testable: the caller sees only the bytes that were
/// kept, and a reader that quietly consumed the whole file looks identical from outside.
/// The test hands this its own reader and reads the position back (review #4's remaining
/// half — the cap bounded the prompt and nothing checked that it bounded the READ).
fn read_bounded(mut source: impl std::io::Read) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    // `take` on the READER itself, not on a `BufReader` wrapped around it: a
    // `BufReader` fills its whole 8 KiB buffer from the underlying reader on every refill,
    // so the file advanced past the cap even though `take` refused to hand the bytes on
    // (measured: 270 336 consumed for a 262 148 cap). Bounded memory either way, but the
    // bound was the buffer's, not the cap's — and `read_to_end` through `take` on a `File`
    // sizes each syscall to what is left, so nothing pays for the missing buffer.
    source
        .by_ref()
        .take(MAX_CONTEXT_FILE_BYTES as u64 + MAX_SCALAR_BYTES)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Open `path` read-only without trusting its type; see [`read_prompt_input_bounded`].
fn open_regular_file(path: &Path) -> io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a regular file",
        ));
    }
    Ok(file)
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContextFile {
    pub path: PathBuf,
    pub content: String,
}

fn load_context_file_from_dir(dir: &Path) -> Option<ContextFile> {
    for filename in CONTEXT_FILE_CANDIDATES {
        let file_path = dir.join(filename);
        // Bounded read, then the cap: the file is untrusted input from the working tree, and a
        // FIFO or an enormous file must cost neither a hang nor a whole-file buffer (review
        // #29, #4). The marker travels IN the text, because a flag nobody reads is a
        // truncation the model cannot notice (review #2).
        if let Ok(content) = read_prompt_input_marked(&file_path, "CONTEXT FILE") {
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
            if skill.source_info.scope == SourceScope::Project && skill.is_python() {
                // Project files can supply pyproject.toml and Python modules.
                // Keep the skill's markdown text, but do not offer or prepare
                // executable code from a repository on session startup.
                skill.kind = SkillKind::Markdown;
                skill.python = None;
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
        // Same cap and the same bounded read as a context file: this text becomes the system
        // prompt or its appended tail, so an oversized file is the same denial of service by
        // another route (review #4).
        return read_prompt_input_marked(path, "PROMPT INPUT").ok();
    }
    Some(truncate_context_content_marked(
        input,
        MAX_CONTEXT_FILE_BYTES,
        "PROMPT INPUT",
    ))
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

    /// Ancestor discovery starts at the global file and then walks the cwd chain
    /// root-most first.
    ///
    /// The walk continues past the fixture to the filesystem root, so the
    /// fixture cannot own every entry: when `TMPDIR` points inside a tree that
    /// carries an `AGENTS.md` (the gate exports `<repo>/.tmp`, and the
    /// repository root has one) those ancestors contribute their own file ahead
    /// of the fixture's. Positions, not the whole list, are therefore what this
    /// test asserts: the global file first, then the fixture's chain root-most
    /// first, ending at the cwd's own file.
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
        assert_eq!(contents.first(), Some(&"global"));
        assert_eq!(&contents[contents.len() - 2..], ["level-a", "deep"]);
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
        // Ancestor discovery walks past the fixture to the filesystem root, so a
        // `TMPDIR` inside a tree that carries its own `AGENTS.md` (the gate
        // exports `<repo>/.tmp`) adds ancestors this fixture does not own. Only
        // the fixture's own entry is asserted, and it is last: the ancestor
        // chain is emitted root-most first, so the cwd's file comes after every
        // ancestor that discovery picks up ahead of it.
        let owned: Vec<&str> = resources
            .agents_files
            .iter()
            .filter(|file| file.path.starts_with(tmp.path()))
            .map(|file| file.content.as_str())
            .collect();
        assert_eq!(owned, vec!["Project rules"]);
        assert_eq!(
            resources
                .agents_files
                .last()
                .map(|file| file.content.as_str()),
            Some("Project rules")
        );
        assert_eq!(resources.system_prompt.as_deref(), Some("custom system"));
        assert_eq!(resources.append_system_prompt.len(), 0);
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
    fn project_python_skills_are_markdown_only_before_kernel_bootstrap() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("project");
        let agent_dir = tmp.path().join("agent");
        let project_skill = cwd.join(".prime/agent/skills/project-code");
        let configured_skill = tmp.path().join("configured/configured-code");
        let ancestor_skill = tmp.path().join(".agents/skills/ancestor-code");
        let user_skill = agent_dir.join("skills/user-code");
        for (dir, name) in [
            (&project_skill, "project-code"),
            (&configured_skill, "configured-code"),
            (&ancestor_skill, "ancestor-code"),
            (&user_skill, "user-code"),
        ] {
            fs::create_dir_all(dir.join("src").join(name.replace('-', "_"))).unwrap();
            fs::write(
                dir.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: Python skill\n---\nBody"),
            )
            .unwrap();
            fs::write(
                dir.join("pyproject.toml"),
                "[project]\nname = \"skill\"\nversion = \"0.1.0\"\n",
            )
            .unwrap();
            fs::write(
                dir.join("src")
                    .join(name.replace('-', "_"))
                    .join("__init__.py"),
                "# would execute if imported\n",
            )
            .unwrap();
        }
        fs::write(
            cwd.join(".prime/agent/settings.json"),
            serde_json::json!({"skills": [configured_skill.display().to_string()] }).to_string(),
        )
        .unwrap();
        let resources = load_resources(ResourceLoaderOptions::new(&cwd, &agent_dir)).unwrap();
        for name in ["project-code", "configured-code", "ancestor-code"] {
            let project = resources
                .skills
                .iter()
                .find(|skill| skill.name == name)
                .unwrap();
            assert_eq!(project.source_info.scope, SourceScope::Project);
            assert_eq!(project.kind, SkillKind::Markdown);
            assert!(project.python.is_none());
        }
        let user = resources
            .skills
            .iter()
            .find(|skill| skill.name == "user-code")
            .unwrap();
        assert_eq!(user.source_info.scope, SourceScope::User);
        assert_eq!(user.kind, SkillKind::Python);
        let imports =
            crate::session_engine::runtime_wiring::kernel_python_skills(&resources.skills)
                .into_iter()
                .map(|skill| skill.import_name)
                .collect::<Vec<_>>();
        assert!(imports.contains(&"user_code".to_string()));
        for name in ["project_code", "configured_code", "ancestor_code"] {
            assert!(!imports.contains(&name.to_string()));
        }
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
        // The kept prefix is byte-exact, and the cut is MARKED in the text the model sees: a
        // discarded flag was a truncation the model could not notice (review #2).
        assert!(
            file.content
                .starts_with(&"A".repeat(MAX_CONTEXT_FILE_BYTES)),
            "truncation must not corrupt the retained bytes"
        );
        assert!(
            file.content.contains("CONTEXT FILE TRUNCATED"),
            "and it must SAY so where the model reads it: {}",
            &file.content[MAX_CONTEXT_FILE_BYTES..]
        );
        assert!(
            file.content.contains("10000000"),
            "naming the size that was cut: {}",
            &file.content[MAX_CONTEXT_FILE_BYTES..]
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
        assert!(
            content.starts_with(&"B".repeat(MAX_CONTEXT_FILE_BYTES)),
            "the kept prefix is byte-exact"
        );
        assert!(
            content.contains("PROMPT INPUT TRUNCATED"),
            "and the cut is marked in the prompt: {}",
            &content[MAX_CONTEXT_FILE_BYTES..]
        );
    }

    /// The read stops at the cap, so bytes past it cannot decide the file's fate.
    ///
    /// The truncated tail is never decoded: this file's bytes past the cap are not UTF-8, and a
    /// loader that read to EOF and truncated afterwards dropped the whole file as unreadable
    /// even though none of that tail would ever reach the prompt (review #4).
    #[test]
    fn bytes_past_the_cap_are_never_read() {
        let tmp = tempfile::tempdir().unwrap();
        let mut content = "A".repeat(MAX_CONTEXT_FILE_BYTES);
        content.push_str("\u{0}\u{0} trailing bytes that are not UTF-8");
        let mut bytes = content.into_bytes();
        bytes.truncate(MAX_CONTEXT_FILE_BYTES + 2);
        bytes[MAX_CONTEXT_FILE_BYTES] = 0xff;
        fs::write(tmp.path().join("AGENTS.md"), &bytes).unwrap();

        let file = load_context_file_from_dir(tmp.path()).expect("found and capped, not dropped");
        assert!(
            file.content
                .starts_with(&"A".repeat(MAX_CONTEXT_FILE_BYTES)),
            "the kept prefix is byte-exact"
        );
        // The marker names the file's REAL size, which the loader learns without reading it.
        assert!(
            file.content.contains(&format!("of {} — ", bytes.len())),
            "the marker must name what was cut: {}",
            &file.content[MAX_CONTEXT_FILE_BYTES..]
        );
    }

    /// A FIFO named `AGENTS.md` is refused before the open.
    ///
    /// Opening a FIFO read-only waits for a writer, and the context-file walk runs on the
    /// session-start path, so a pipe planted in a repository used to hang the loader forever
    /// (review #29).
    #[cfg(unix)]
    #[test]
    fn a_fifo_named_agents_md_is_skipped_without_opening_it() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("AGENTS.md");
        nix::unistd::mkfifo(
            &path,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )
        .unwrap();
        // Keep a writer available so a regression fails the assertion instead of hanging the
        // suite: the nonblocking open only succeeds if the loader opened the FIFO.
        let writer_path = path.clone();
        let writer = std::thread::spawn(move || {
            use std::os::unix::fs::OpenOptionsExt;
            loop {
                if let Ok(file) = std::fs::OpenOptions::new()
                    .write(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(&writer_path)
                {
                    drop(file);
                    break;
                }
                if !writer_path.exists() {
                    break;
                }
                std::thread::yield_now();
            }
        });
        let found = load_context_file_from_dir(tmp.path());
        std::fs::remove_file(&path).unwrap();
        writer.join().unwrap();
        assert!(found.is_none(), "a FIFO is not a context file: {found:?}");
    }

    /// The read stops at the cap, not past it.
    ///
    /// `read_prompt_input` wraps the file in a `BufReader` and takes `cap + MAX_SCALAR`,
    /// so the buffer can hold up to a scalar beyond the cap — that is deliberate (a
    /// multi-byte character straddling the boundary must not fail the file). What must
    /// NOT happen is a read that pulls the whole file into memory anyway, and that is
    /// invisible from the returned string: the test counts the bytes the reader
    /// CONSUMED, through a reader that reports its own position.
    ///
    /// Review #4's remaining half: the capped reader was shared, and nothing checked
    /// that the cap is where the reading actually stopped.
    #[test]
    fn the_read_stops_at_the_cap_not_past_it() {
        let limit = MAX_CONTEXT_FILE_BYTES as u64 + MAX_SCALAR_BYTES;
        let source: Vec<u8> = vec![b'a'; limit as usize * 4];
        // A reader that reports how much of the source it actually handed out.
        let mut cursor = std::io::Cursor::new(source);
        let kept = read_bounded(&mut cursor).unwrap();
        let consumed = cursor.position();
        assert_eq!(kept.len() as u64, limit, "the cap is the amount kept");
        assert_eq!(
            consumed, limit,
            "the read consumed past the cap: {consumed} > {limit}"
        );

        // And the same through the public helper: a file far larger than the cap comes
        // back capped and MARKED, without the whole body being kept.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("huge.md");
        std::fs::write(&path, "a".repeat(MAX_CONTEXT_FILE_BYTES * 8)).unwrap();
        let marked = read_prompt_input_marked(&path, "PROMPT INPUT").unwrap();
        assert!(
            marked.len() < MAX_CONTEXT_FILE_BYTES * 2,
            "{}",
            marked.len()
        );
        assert!(
            marked.contains("TRUNCATED"),
            "a truncated read must say so: {}",
            &marked[..marked.len().min(200)]
        );
    }

    /// A refused candidate must not stop the walk: the NEXT one is still read.
    ///
    /// The FIFO test above proves the hang is gone. It does not prove the loader keeps
    /// going — a loader that refused the FIFO *and returned* would pass it while quietly
    /// dropping a repository whose `AGENTS.md` is a pipe and whose `CLAUDE.md` is fine.
    /// Review #29's remaining half.
    #[cfg(unix)]
    #[test]
    fn a_refused_candidate_falls_back_to_the_next_one() {
        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("AGENTS.md");
        nix::unistd::mkfifo(
            &fifo,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )
        .unwrap();
        // The next candidate in `CONTEXT_FILE_CANDIDATES` order is a real file.
        std::fs::write(tmp.path().join("CLAUDE.md"), "the real instructions\n").unwrap();
        // Keep a writer so a regression fails the assertion instead of hanging.
        let writer_path = fifo.clone();
        let writer = std::thread::spawn(move || {
            use std::os::unix::fs::OpenOptionsExt;
            loop {
                if let Ok(file) = std::fs::OpenOptions::new()
                    .write(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(&writer_path)
                {
                    drop(file);
                    break;
                }
                if !writer_path.exists() {
                    break;
                }
                std::thread::yield_now();
            }
        });

        let found = load_context_file_from_dir(tmp.path());
        std::fs::remove_file(&fifo).unwrap();
        writer.join().unwrap();

        let found = found.expect("the next candidate must be read after a refusal");
        assert_eq!(
            found.path.file_name().unwrap(),
            "CLAUDE.md",
            "the loader fell back to the wrong file: {found:?}"
        );
        assert!(found.content.contains("the real instructions"), "{found:?}");
    }
}
