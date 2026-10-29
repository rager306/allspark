//! The venv dir-layout concern: the override-aware
//! kernel venv dir, the writable-dir fallback, and the interpreter path.

use super::{anyhow, Path, PathBuf};

pub(crate) fn expand_home(path: &str) -> PathBuf {
    if path == "~" {
        return home_dir();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    PathBuf::from(path)
}

pub(super) fn home_dir() -> PathBuf {
    pa_types::platform::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

/// Directory of the kernel venv, honoring `PRIME_AGENT_KERNEL_VENV`.
#[must_use]
pub fn kernel_venv_dir() -> PathBuf {
    // Whitespace is not a path (review #66). `"   "` is non-empty, so an unset-looking
    // variable used to produce a RELATIVE venv directory — created in the process's cwd rather
    // than under the agent dir, and then invisible to every later session that resolves the
    // default. Trimming makes "blank" mean unset, which is what an operator typing spaces means.
    if let Ok(override_dir) = std::env::var("PRIME_AGENT_KERNEL_VENV") {
        let override_dir = override_dir.trim();
        if !override_dir.is_empty() {
            return expand_home(override_dir);
        }
    }
    home_dir().join(".prime").join("agent").join("kernel-venv")
}

fn xdg_kernel_venv_dir() -> PathBuf {
    // Same whitespace rule as the override above (review #66): a blank `XDG_DATA_HOME` must
    // fall back to the default instead of silently rooting the venv at a relative path.
    let data_home = match std::env::var("XDG_DATA_HOME") {
        Ok(value) if !value.trim().is_empty() => expand_home(value.trim()),
        _ => home_dir().join(".local").join("share"),
    };
    data_home.join("prime").join("agent").join("kernel-venv")
}

pub(crate) fn resolve_writable_kernel_venv_dir() -> anyhow::Result<PathBuf> {
    let primary = kernel_venv_dir();
    if std::fs::create_dir_all(primary.parent().unwrap_or(Path::new("/"))).is_ok() {
        return Ok(primary);
    }
    if std::env::var("PRIME_AGENT_KERNEL_VENV").is_ok_and(|v| !v.is_empty()) {
        return Err(anyhow!(
            "couldn't create kernel venv parent directories for {}",
            primary.display()
        ));
    }
    let fallback = xdg_kernel_venv_dir();
    if std::fs::create_dir_all(fallback.parent().unwrap_or(Path::new("/"))).is_err() {
        return Err(anyhow!(
            "couldn't create kernel venv directory at {} or {}; set PRIME_AGENT_KERNEL_PYTHON to a python with a current prime-agent-runtime installed",
            primary.display(),
            fallback.display()
        ));
    }
    Ok(fallback)
}

/// Path of the venv's python interpreter.
#[must_use]
pub fn kernel_venv_python(venv: &Path) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts").join("python.exe")
    } else {
        venv.join("bin").join("python")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A whitespace-only override is UNSET, not a path (review #66).
    ///
    /// `"   "` is non-empty, so the old check returned it and `expand_home` handed back a
    /// relative directory — the venv landed in the process's cwd instead of under the agent
    /// dir. The two variables are process-global, so each case restores what it found.
    #[test]
    fn a_blank_override_falls_back_to_the_default() {
        let previous = std::env::var("PRIME_AGENT_KERNEL_VENV").ok();
        std::env::set_var("PRIME_AGENT_KERNEL_VENV", "   ");
        let resolved = kernel_venv_dir();
        match previous {
            Some(value) => std::env::set_var("PRIME_AGENT_KERNEL_VENV", value),
            None => std::env::remove_var("PRIME_AGENT_KERNEL_VENV"),
        }
        assert!(
            resolved.is_absolute(),
            "a blank override must not yield a relative venv dir: {}",
            resolved.display()
        );
        assert!(
            resolved.ends_with(Path::new(".prime").join("agent").join("kernel-venv")),
            "and it must land on the default: {}",
            resolved.display()
        );
    }

    /// A real override still wins, unchanged.
    #[test]
    fn a_real_override_is_honoured_verbatim() {
        let previous = std::env::var("PRIME_AGENT_KERNEL_VENV").ok();
        std::env::set_var("PRIME_AGENT_KERNEL_VENV", "/tmp/pa-kernel-venv-test");
        let resolved = kernel_venv_dir();
        match previous {
            Some(value) => std::env::set_var("PRIME_AGENT_KERNEL_VENV", value),
            None => std::env::remove_var("PRIME_AGENT_KERNEL_VENV"),
        }
        assert_eq!(resolved, PathBuf::from("/tmp/pa-kernel-venv-test"));
    }
}
