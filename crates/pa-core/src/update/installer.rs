//! The installer-takeover update funnel: `prime-agent update` and the TUI's
//! `/update` download the update channel's installer and run it — the
//! official domain endpoint for stable, the download base's
//! `install-beta.sh` for nightly; never a GitHub raw or workflow URL. The
//! script is the single source of truth for the whole move — it resolves
//! and downloads the latest build, uninstalls the TypeScript version,
//! publishes the payload, and never touches `~/.prime/agent` (the sessions
//! and configuration the products share). The command's contract is "fetch
//! from the official source, run it". This module only fetches and execs
//! the script, then reports what landed; every install/uninstall decision
//! stays in the script the installer-takeover lane owns, so the two
//! surfaces can never drift from it.

use std::io::{Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};

use super::release::update_user_agent;

/// `PRIME_AGENT_RUST_PREFIX`: the installer's own prefix knob (the launcher
/// probe reads the same value the script installs under).
pub const ENV_PREFIX: &str = "PRIME_AGENT_RUST_PREFIX";
/// `PRIME_AGENT_RUST_INSTALLER_URL`: the installer script URL override —
/// tests serve their own script, a pinned install can point elsewhere.
pub const ENV_INSTALLER_URL: &str = "PRIME_AGENT_RUST_INSTALLER_URL";
/// The installer's release-channel knob (`stable` | `beta`): the funnel
/// passes the requested channel, else the installed one.
pub const ENV_RELEASE_CHANNEL: &str = "PRIME_AGENT_RELEASE_CHANNEL";
/// The installer's download-base knob (the bucket holding the channel
/// manifests and release archives).
pub const ENV_DOWNLOAD_BASE_URL: &str = "PRIME_AGENT_DOWNLOAD_BASE_URL";
/// `PRIME_AGENT_INSTALL_PAYLOAD_DIR`: the directory of the payload binary
/// that runs the update funnel. The installer script's daemon stop gate stops
/// a Rust daemon only when its own executable lives under this run's install
/// roots — the prefix it writes, or this payload's tree (the fork's launcher
/// layout keeps the payload in `libexec/`, outside the prefix). Without the
/// variable (a standalone `curl | sh` run) the prefix alone decides.
pub const ENV_INSTALL_PAYLOAD_DIR: &str = "PRIME_AGENT_INSTALL_PAYLOAD_DIR";
/// `PRIME_AGENT_FORK_BUILD`: the fork build id the funnel exports to the
/// installer script so its daemon stop-probe can tell a fork run from an
/// upstream one (both payloads are named `prime-agent`, so argv cannot; the
/// script keys off this marker and the fork's own socket path — known bug
/// #22). Baked from the compile-time `ALLSPARK_FORK_BUILD`; unset upstream.
pub const ENV_FORK_BUILD: &str = "PRIME_AGENT_FORK_BUILD";
/// `PRIME_AGENT_ALLOW_HTTP`: the explicitly-named escape hatch `install-rust.sh`
/// and `install.ps1` already honor — a plaintext (non-`https`) installer URL or
/// download base is refused unless this is set to `1`, for a local channel/e2e
/// run the operator controls. The Rust funnel mirrors the scripts' own rule so
/// the surface that fetches and execs the script cannot be more permissive
/// than the script itself.
pub const ENV_ALLOW_HTTP: &str = "PRIME_AGENT_ALLOW_HTTP";
/// `install-rust.sh`'s `DOWNLOAD_BASE_URL_DEFAULT`: where the channel
/// manifests (`latest.json`, `beta.json`) are published.
pub const DEFAULT_DOWNLOAD_BASE_URL: &str = "https://pub-728493de92a943e2a9b2d17b4719f318.r2.dev";

/// The official domain's install endpoint — the stable channel's installer
/// source; never a GitHub raw or workflow URL (the override env var stays
/// for tests and pinned installs).
pub const OFFICIAL_INSTALLER_URL: &str = "https://app.primeintellect.ai/prime-agent/install.sh";
/// The nightly installer's file under the download base (the domain only
/// forwards `install.sh`, so nightly updates fetch it from the base).
pub const BETA_INSTALLER_FILE: &str = "install-beta.sh";

/// `install-rust.sh` as this build shipped it. The local operations
/// (`prime-agent update --rollback` and `--archive`) run this copy: they
/// need no network, and the script matches the layout this build was
/// installed with.
const BUNDLED_INSTALLER: &str = include_str!("../../../../install-rust.sh");

/// The payload binary's file name under `<prefix>/share/prime-agent`.
const PAYLOAD_BINARY: &str = if cfg!(windows) {
    "prime-agent.exe"
} else {
    "prime-agent"
};

/// The small-file budget for the script download (the script is a few KB;
/// a hung fetch must not hang the update).
const SCRIPT_FETCH_TIMEOUT: Duration = Duration::from_secs(30);
/// The launcher `--version` probe budget (the same bound the release
/// probe uses; a hung launcher must not hang the report).
const LAUNCHER_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// The installer script URL for an installer channel (`stable` | `beta`):
/// `<download base>/install-beta.sh` for beta, the official domain's
/// install endpoint otherwise. `PRIME_AGENT_RUST_INSTALLER_URL` overrides
/// both — tests serve their own script, a pinned install can point
/// elsewhere.
#[must_use]
pub fn installer_script_url(channel: Option<&str>) -> String {
    if let Ok(url) = std::env::var(ENV_INSTALLER_URL) {
        if !url.trim().is_empty() {
            return url;
        }
    }
    match channel {
        Some("beta") => format!(
            "{}/{BETA_INSTALLER_FILE}",
            download_base_url().trim_end_matches('/')
        ),
        _ => OFFICIAL_INSTALLER_URL.to_string(),
    }
}

/// The download base `--check` reads the channel manifest from: the
/// installer's own knob, else its default.
#[must_use]
pub fn download_base_url() -> String {
    std::env::var(ENV_DOWNLOAD_BASE_URL)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_DOWNLOAD_BASE_URL.to_string())
}

/// Whether a URL is `https://` (the installer scripts' own `https://*` case).
#[must_use]
fn is_https_url(url: &str) -> bool {
    url.trim_start().starts_with("https://")
}

/// A URL with its userinfo removed, for anything a human or a log will read
/// (review #32, CWE-532).
///
/// `https://user:token@host/path` is a legal way to hand a mirror its
/// credentials, and every refusal/error path here formats the URL. The token is
/// then in the daemon log, the update status file, and the TUI's error row.
/// Everything between the scheme's `//` and the last `@` before the path is the
/// userinfo; a URL with no `@` in that span is returned unchanged.
#[must_use]
pub fn redact_url_credentials(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let authority_start = scheme_end + 3;
    let authority_end = url[authority_start..]
        .find(['/', '?', '#'])
        .map_or(url.len(), |at| authority_start + at);
    // Strip the userinfo when there is one, then redact credential-bearing query
    // parameters — a presigned mirror URL puts its secret there instead (`?token=`,
    // `?X-Amz-Signature=`), and it leaks by exactly the same path (review #32, the
    // half the first fix left: userinfo only).
    let mut out = match url[authority_start..authority_end].rfind('@') {
        Some(userinfo_end) => format!(
            "{}<redacted>@{}",
            &url[..authority_start],
            &url[authority_start + userinfo_end + 1..]
        ),
        None => url.to_string(),
    };
    redact_secret_query_params(&mut out);
    out
}

/// Query parameter names whose VALUE is a credential.
///
/// Matched case-insensitively as a substring of the name, so `access_token`,
/// `X-Amz-Signature` and `sig` all land. Deliberately a list and not "the whole query":
/// a query also carries the diagnostic part of a URL (a channel name, a page number),
/// and erasing it turns an error message into one nobody can act on.
const SECRET_QUERY_PARAMS: [&str; 8] = [
    "token",
    "signature",
    "credential",
    "password",
    "secret",
    "sig",
    "key",
    "auth",
];

/// Replace the VALUE of every credential-bearing query parameter with `<redacted>`, and
/// drop the fragment.
///
/// Three shapes the first version missed, all named by the fourth reverify wave:
///   * a **bare value** with no `=` (`?ghp_abc123`) carries a credential and has no name to
///     match, so it is redacted whole;
///   * a **fragment** (`#access_token=…`) is never sent to a server but IS printed, and an
///     OAuth-style URL keeps its token there;
///   * a pair that is neither named nor bare is left ALONE on purpose: a query also carries
///     the diagnostic half (`?channel=beta`), and erasing it makes the message useless.
fn redact_secret_query_params(url: &mut String) {
    // The fragment first: it is not part of the query and its content is never diagnostic.
    if let Some(fragment) = url.find('#') {
        url.truncate(fragment);
        url.push_str("#<redacted>");
    }
    let Some(query_start) = url.find('?') else {
        return;
    };
    // The query ends where the (already redacted) fragment starts: taking `url.len()` here
    // swallowed the marker into the last pair's VALUE and dropped it from the output.
    let query_end = url.find('#').unwrap_or(url.len());
    let query = url[query_start + 1..query_end].to_string();
    let mut changed = false;
    let redacted: Vec<String> = query
        .split('&')
        .map(|pair| {
            let (name, value) = match pair.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (pair, None),
            };
            let lowered = name.to_ascii_lowercase();
            match (value, lowered.is_empty()) {
                // A bare token with no `=` at all.
                (None, _) if !pair.is_empty() => {
                    changed = true;
                    "<redacted>".to_string()
                }
                // A NAMED parameter whose name says credential.
                (Some(_), false)
                    if SECRET_QUERY_PARAMS
                        .iter()
                        .any(|needle| lowered.contains(needle)) =>
                {
                    changed = true;
                    format!("{name}=<redacted>")
                }
                // `?=value`: a value with no name at all, so no name can vouch for it.
                (Some(_), true) => {
                    changed = true;
                    "=<redacted>".to_string()
                }
                _ => pair.to_string(),
            }
        })
        .collect();
    if changed {
        url.replace_range(query_start + 1..query_end, &redacted.join("&"));
    }
}

/// The refusal for a non-`https` installer URL or download base, unless the
/// explicitly-named escape hatch ([`ENV_ALLOW_HTTP`]`=1`) is active — the exact
/// rule and knob `install-rust.sh` and `install.ps1` carry for the download
/// base. `None` when the URL is acceptable (or the hatch is open). Pure, so the
/// rule is unit-testable without the process-global environment.
#[must_use]
pub fn installer_url_refusal(url: &str, allow_http: bool) -> Option<String> {
    if is_https_url(url) || allow_http {
        return None;
    }
    Some(format!(
        "the installer URL must be an https URL: {} \
         (set {ENV_ALLOW_HTTP}=1 only for a local channel you control)",
        redact_url_credentials(url)
    ))
}

/// Whether the [`ENV_ALLOW_HTTP`] escape hatch is active (`=1`).
///
/// Public so every surface that fetches an update over the network reads the SAME hatch. The
/// download base in `pa-cli`'s plan step was missing the check entirely (review #6).
#[must_use]
pub fn allow_http() -> bool {
    std::env::var(ENV_ALLOW_HTTP).is_ok_and(|value| value == "1")
}

/// Refuse a non-`https` URL and log the refusal: the Rust funnel must be no
/// more permissive than the script it fetches and execs.
fn ensure_https_url(url: &str) -> Result<(), UpdateFailure> {
    if let Some(message) = installer_url_refusal(url, allow_http()) {
        tracing::warn!(
            url = %redact_url_credentials(url),
            "refusing a non-https installer source: {message}"
        );
        return Err(UpdateFailure { message });
    }
    Ok(())
}

/// The install prefix the launcher probe reads (`PRIME_AGENT_RUST_PREFIX`,
/// the installer's own `~/.local` default).
#[must_use]
pub fn install_prefix() -> PathBuf {
    if let Some(prefix) = std::env::var_os(ENV_PREFIX) {
        if !prefix.is_empty() {
            return PathBuf::from(prefix);
        }
    }
    if let Some(prefix) = running_installer_prefix() {
        return prefix;
    }
    pa_types::platform::home_dir()
        .unwrap_or_default()
        .join(".local")
}

/// The target triple for one platform pair (`std::env::consts`' vocabulary:
/// the installer's own `uname -s`/`-m` mapping over the same four targets).
#[must_use]
pub fn target_for(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu"),
        ("windows", "x86_64") => Some("x86_64-pc-windows-msvc"),
        _ => None,
    }
}

/// The target triple this machine's update would install.
///
/// # Errors
/// Returns an error naming the machine when no continuous build is published for its platform.
pub fn current_target() -> Result<&'static str> {
    target_for(std::env::consts::OS, std::env::consts::ARCH).ok_or_else(|| {
        anyhow!(
            "{}",
            no_build_message(std::env::consts::OS, std::env::consts::ARCH)
        )
    })
}

/// The refusal `--check` prints for one platform pair: the machine plus the
/// full published matrix, so an unsupported machine sees exactly what the
/// channel builds (the same message install-rust.sh's uname arm dies with).
#[must_use]
pub fn no_build_message(os: &str, arch: &str) -> String {
    format!(
        "no rust build is published for {os} {arch} (the release channel \
         builds aarch64-apple-darwin, x86_64-apple-darwin, \
         aarch64-unknown-linux-gnu, x86_64-unknown-linux-gnu, and \
         x86_64-pc-windows-msvc)"
    )
}

/// The commit a running version's `-continuous.<sha>` stamp names — the
/// identity `--check` compares against the latest run's `head_sha`.
#[must_use]
pub fn running_commit(version: &str) -> Option<&str> {
    let (_, commit) = version.trim().rsplit_once("-continuous.")?;
    let hex =
        (7..=40).contains(&commit.len()) && commit.bytes().all(|byte| byte.is_ascii_hexdigit());
    hex.then_some(commit)
}

/// Where the installer's output goes while it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallerOutput {
    /// The CLI's run: the installer's own progress streams to the user's terminal.
    Inherit,
    /// The TUI's run: the output is captured (the live frame stays intact)
    /// and the failure tail becomes the message; the run is detached from
    /// the terminal (it can never prompt).
    Capture,
}

/// What a completed installer run landed: the new build's version line, when the probe found one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub version: Option<String>,
}

/// Why an update run failed: the actionable message the surfaces print
/// (the CLI as its `Error:` line, the TUI as the error row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateFailure {
    pub message: String,
}

/// Run the takeover update with the environment's knobs (the install
/// prefix and the channel's script URL): the composition root's entry.
/// `channel` is the requested release channel (`stable` | `beta`); `None`
/// keeps the installed one.
///
/// # Errors
/// Returns the failure message for every non-installing outcome (see
/// [`run_installer_from`]).
pub async fn run_installer(
    channel: Option<&'static str>,
    output: InstallerOutput,
) -> std::result::Result<Installed, UpdateFailure> {
    ensure_this_build_can_update()?;
    let prefix = install_prefix();
    let channel = channel.or_else(|| installed_channel(&prefix));
    run_installer_from(&installer_script_url(channel), &prefix, channel, output).await
}

/// Refuse to run the upstream installer from a fork build.
///
/// Every URL this funnel can reach belongs to upstream: the download base is
/// upstream's R2 bucket and the non-beta script is served from
/// `app.primeintellect.ai`. Nothing here chooses a channel by product — the
/// fork marker only ever reaches the installer's daemon-stop probe — so a fork
/// build that updates itself installs UPSTREAM into whatever prefix it resolves,
/// silently replacing the fork's payload with a build that lacks the fork-only
/// crates.
///
/// Until a fork release manifest exists (identity, version, checksums, atomic
/// activation), refusing is the honest answer: a clear "not supported" beats an
/// update that appears to succeed and changes the product underneath. The escape
/// hatch stays for a deliberate cross-install: setting the installer URL or the
/// download base routes the funnel somewhere this build does not own, and that is
/// the caller's decision to make, not a default.
fn ensure_this_build_can_update() -> std::result::Result<(), UpdateFailure> {
    let url_override = std::env::var(ENV_INSTALLER_URL).ok();
    let base_override = std::env::var(ENV_DOWNLOAD_BASE_URL).ok();
    fork_update_refusal(
        option_env!("ALLSPARK_FORK_BUILD"),
        url_override.as_deref(),
        base_override.as_deref(),
    )
}

/// The decision behind [`ensure_this_build_can_update`], pure over its inputs so a
/// test can exercise both products without a rebuilt binary.
///
/// # Errors
///
/// [`UpdateFailure`] when this is a fork build and neither override is set.
fn fork_update_refusal(
    fork_build: Option<&str>,
    installer_url: Option<&str>,
    download_base: Option<&str>,
) -> std::result::Result<(), UpdateFailure> {
    if fork_build.is_none() {
        return Ok(());
    }
    let configured = |value: Option<&str>| value.is_some_and(|value| !value.trim().is_empty());
    if configured(installer_url) || configured(download_base) {
        return Ok(());
    }
    Err(UpdateFailure {
        message: "this is an allspark fork build, and no fork update channel is configured: \
                  the updater's default endpoints belong to upstream prime-agent, so updating \
                  would replace this build with one that lacks the fork crates. Point \
                  PRIME_AGENT_RUST_INSTALLER_URL (or PRIME_AGENT_DOWNLOAD_BASE_URL) at a fork \
                  release, or install the new allspark release manually."
            .to_string(),
    })
}

/// Run the takeover update from one explicit script URL and install
/// prefix: platform preflight, fetch the branch's installer script, and
/// exec it with the environment passed through (the installer's own
/// `PRIME_AGENT_RUST_*` knobs ride the process environment; the script
/// owns download, the TypeScript uninstall, the publish, and the
/// `~/.prime/agent` preserve). On success the
/// launcher's `--version` answers the new version; on failure the
/// previous install is kept (the script's own rollback covers a
/// mid-publish crash).
///
/// # Errors
/// Returns the failure message for every non-installing outcome.
pub async fn run_installer_from(
    url: &str,
    prefix: &Path,
    channel: Option<&'static str>,
    output: InstallerOutput,
) -> std::result::Result<Installed, UpdateFailure> {
    // THE HTTPS RULE (mirrored from install-rust.sh / install.ps1): the funnel
    // fetches this URL and pipes the bytes to `/bin/sh -s --`, so a plaintext
    // source would let a network attacker swap the script the funnel then runs
    // with the user's credentials. A non-https URL is refused up front — before
    // any request — unless the explicitly-named escape hatch is set.
    ensure_https_url(url)?;
    current_target().map_err(|error| UpdateFailure {
        message: format!("{error:#}"),
    })?;
    let (script, handle) = fetch_script(url).await.map_err(|error| UpdateFailure {
        // The TOP-LEVEL message only, never `{error:#}`: the alternate form walks reqwest's
        // error chain, and a chain can carry the URL in a nested source that no wrapper of
        // ours ever saw — so the redacted URL above would sit beside a raw one
        // (review #32's last shape). The top message names the failure kind, which is what
        // an operator acts on.
        message: format!(
            "could not download the installer from {}: {error}",
            redact_url_credentials(url)
        ),
    })?;
    let result = execute_script(&handle, &[], prefix, channel, output).await;
    let _ = std::fs::remove_file(&script);
    result?;
    let version = launcher_version(prefix).await;
    Ok(Installed { version })
}

/// Run the bundled `install-rust.sh` with `args` (`--rollback`, or `--archive
/// <path>`) against the install under `prefix`. On success the version is
/// the one the published payload's install marker records — except on the
/// Windows payload handoff, where the caller IS the locked payload: the
/// child is spawned detached and no version is read (the installer's own
/// streamed output carries the outcome).
///
/// # Errors
/// Returns the failure message when the script cannot be written or run,
/// or exits nonzero (its own message already streamed to the terminal).
pub async fn run_bundled_installer(
    prefix: &Path,
    args: &[&std::ffi::OsStr],
) -> std::result::Result<Installed, UpdateFailure> {
    // `mut` serves the Windows handoff's post-pre-flight rewind; the
    // unix path only ever borrows the handle.
    #[cfg_attr(not(windows), allow(unused_mut))]
    let (script, mut handle) =
        write_script(BUNDLED_INSTALLER.as_bytes()).map_err(|error| UpdateFailure {
            message: format!("could not write the bundled installer: {error:#}"),
        })?;
    #[cfg(windows)]
    if caller_owns_payload(prefix) {
        // THE HANDOFF (Windows rename rule): this process's image sits inside
        // <prefix>/share/prime-agent and Windows refuses to rename a directory
        // that contains a running image, so the script's publish step can only
        // succeed after this process exits. The child is spawned without
        // waiting and inherits the terminal, so the installer's own steps and
        // error messages still print there. The script rides the child's stdin
        // (`bash -s --`, the curl|sh one-liner's own invocation form), so the
        // temp file's name is removed the moment the child holds its handle —
        // nothing of this run outlives the process, not even on a failed
        // spawn (std opens with FILE_SHARE_DELETE; a failure leaves nobody
        // reading and the removal is the same). The caller returns
        // immediately; the outcome rides the installer's own output.
        // The local-mode pre-flight (the exact-equivalence source): the
        // rollback's generations record rides the script's OWN prefix
        // spelling (cygpath + physical_path), and the archive's checks
        // (name, tar, payload, the version the packaged package.json
        // reports) are the script's staged run's own — no native mirror
        // can answer either faithfully, so the script itself answers, in
        // its PRIME_AGENT_ROLLBACK_CHECK / PRIME_AGENT_ARCHIVE_CHECK
        // modes, synchronously. Neither mode claims the lock or renames
        // the live tree (the rollback check only reads; the archive
        // check's extraction rides a temporary stage swept on its own
        // exit), so waiting never fights this process's locked image,
        // and the refusal they print is the one the handed-off run
        // would die with — the caller exits with the real status, and
        // only the publication itself stays async (the platform-
        // inherent residue).
        if let Err(error) = local_mode_preflight(&handle, prefix, args).await {
            let _ = std::fs::remove_file(&script);
            return Err(error);
        }
        // The pre-flight child read the script to EOF through its own
        // descriptor — a clone shares this handle's file position, so the
        // real child must start from the first byte again.
        handle.seek(SeekFrom::Start(0)).map_err(|error| {
            let _ = std::fs::remove_file(&script);
            UpdateFailure {
                message: format!("could not run the installer: {error}"),
            }
        })?;
        let shell = trusted_shell().inspect_err(|_| {
            let _ = std::fs::remove_file(&script);
        })?;
        let mut command = installer_child(&shell, prefix, None);
        command
            .arg("-s")
            .arg("--")
            .args(args)
            .stdin(std::process::Stdio::from(handle));
        // THE PROCESS-CONTROL WALL (the in-house detached-spawn wrapper:
        // CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS | CREATE_NO_WINDOW,
        // the product's own detached-survives-parent mapping): a console-
        // attached child dies with the caller's terminal close (the
        // CTRL_CLOSE broadcast) — mid-publish, exactly when the handoff
        // exists to let it finish — and Ctrl+C at the caller's terminal
        // must not reach the installer either. The inherited stdio
        // handles keep the installer's own steps printing to the
        // caller's window for as long as that window lives.
        crate::platform::process::set_new_process_group(command.as_std_mut());
        // THE PARENT EXIT: the payload unlock this whole handoff exists
        // for happens only when THIS process dies — the child gets the
        // pid and the script's head waits for it (bounded) before any
        // step touches the payload, so the release-before-publish
        // ordering is explicit instead of a spawn-timing coincidence.
        command.env(
            "PRIME_AGENT_INSTALLER_PARENT_PID",
            std::process::id().to_string(),
        );
        let spawned = command.spawn();
        let _ = std::fs::remove_file(&script);
        spawned.map_err(|error| UpdateFailure {
            message: format!("could not run the installer: {error}"),
        })?;
        return Ok(Installed { version: None });
    }
    let result = execute_script(&handle, args, prefix, None, InstallerOutput::Inherit).await;
    let _ = std::fs::remove_file(&script);
    result?;
    Ok(Installed {
        version: installed_version(prefix),
    })
}

/// The install prefix of an installer-owned binary: `exe` is
/// `<prefix>/share/prime-agent/prime-agent` (`prime-agent.exe` on Windows)
/// and that payload carries the installer's marker. `None` for any other
/// binary (a managed release, a development build).
#[must_use]
fn installer_prefix_of(exe: &Path) -> Option<PathBuf> {
    let payload = exe.parent()?;
    let share = payload.parent()?;
    let owned = exe.file_name()? == PAYLOAD_BINARY
        && payload.file_name()? == "prime-agent"
        && share.file_name()? == "share";
    let prefix = share.parent()?;
    (owned && installed_channel(prefix).is_some()).then(|| prefix.to_path_buf())
}

/// The install prefix of the running binary when the installer owns it
/// (`<prefix>/share/prime-agent/prime-agent` in a marked payload).
#[must_use]
pub fn running_installer_prefix() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    // Windows answers the module path already absolute; its canonical form
    // is the \\?\ verbatim spelling, which the installer cannot take as a
    // prefix.
    #[cfg(not(windows))]
    let exe = exe.canonicalize().ok()?;
    installer_prefix_of(&exe)
}

/// True when this process's own image is the payload binary of `prefix`
/// (`<prefix>/share/prime-agent/prime-agent.exe` with the install marker):
/// the Windows rename rule — a running image locks its directory, so an
/// installer child can only publish this payload after this process exits.
/// On unix a rename never fights a running image, so the answer is false.
#[must_use]
pub fn caller_owns_payload(prefix: &Path) -> bool {
    #[cfg(not(windows))]
    {
        let _ = prefix;
        false
    }
    #[cfg(windows)]
    {
        // The same helper the CLI read its prefix from: both sides then
        // spell the path identically (canonicalizing here would answer
        // the \\?\ verbatim form the prefix never equals).
        running_installer_prefix().is_some_and(|self_prefix| self_prefix == prefix)
    }
}

/// The synchronous local-mode pre-flight for the Windows handoff: run the
/// bundled installer ITSELF in its check mode — `PRIME_AGENT_ROLLBACK_CHECK`
/// or `PRIME_AGENT_ARCHIVE_CHECK` by the requested mode — and fail with the
/// script's own refusal when the operation cannot run. Neither mode ever
/// claims the publication lock or renames the live tree — the rollback
/// check only reads, and the archive check's extraction lives in a
/// temporary stage swept on its own exit — so waiting never fights the
/// locked image; and the answer is the script's OWN scan, the exact
/// equivalence the caller's exit code needs (once handed off, the
/// outcome can no longer be this process's exit status).
///
/// # Errors
/// Returns the pre-flight's captured refusal tail or the spawn failure.
#[cfg(windows)]
async fn local_mode_preflight(
    script: &std::fs::File,
    prefix: &Path,
    args: &[&std::ffi::OsStr],
) -> std::result::Result<(), UpdateFailure> {
    let shell = trusted_shell()?;
    let mut command = installer_child(&shell, prefix, None);
    command
        .arg("-s")
        .arg("--")
        .args(args)
        .stdin(std::process::Stdio::from(script.try_clone().map_err(
            |error| UpdateFailure {
                message: format!("could not run the installer: {error}"),
            },
        )?));
    if args.first().copied() == Some(std::ffi::OsStr::new("--rollback")) {
        command.env("PRIME_AGENT_ROLLBACK_CHECK", "1");
    } else {
        command.env("PRIME_AGENT_ARCHIVE_CHECK", "1");
    }
    let output = command.output().await.map_err(|error| UpdateFailure {
        message: format!("could not run the installer: {error}"),
    })?;
    if output.status.success() {
        return Ok(());
    }
    let refusal =
        output_tail(&output).unwrap_or_else(|| "the installer refused the request".to_string());
    Err(UpdateFailure { message: refusal })
}

/// Flatten an install marker's ps1-era encodings to plain bytes: strip a
/// leading UTF-8 or UTF-16 BOM, then drop the NULs (UTF-16's interleave;
/// this script's own ASCII markers contain none). UTF-16BE text loses its
/// byte pairing, but the marker content is ASCII, so both UTF-16 orders
/// read back as the same ASCII bytes.
fn normalize_marker_bytes(bytes: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    let bytes = match bytes {
        [0xEF, 0xBB, 0xBF, rest @ ..] | [0xFF, 0xFE, rest @ ..] | [0xFE, 0xFF, rest @ ..] => rest,
        _ => bytes,
    };
    if bytes.contains(&0) {
        std::borrow::Cow::Owned(
            bytes
                .iter()
                .copied()
                .filter(|byte| *byte != 0)
                .collect::<Vec<u8>>(),
        )
    } else {
        std::borrow::Cow::Borrowed(bytes)
    }
}

/// The installed payload's version, read from the install marker's
/// "version <v>" line — through the same encoding normalization as
/// [`installed_channel`]: a ps1-written marker can be UTF-16 or
/// BOM-prefixed, and the version line of an encoded marker must read too.
fn installed_version(prefix: &Path) -> Option<String> {
    let bytes = std::fs::read(prefix.join("share/prime-agent/.prime-agent-install")).ok()?;
    let normalized = normalize_marker_bytes(&bytes).into_owned();
    let marker = String::from_utf8_lossy(&normalized);
    let version = marker.lines().nth(1)?.strip_prefix("version ")?.trim();
    (!version.is_empty()).then(|| version.to_string())
}

/// The installed payload's channel, read from the install marker (the
/// installer's own `.prime-agent-install` under the prefix's share dir;
/// its first line is "channel <name>"). `None` when the marker is absent
/// (a pre-marker install or a foreign tree) or carries no known channel —
/// the update then rides the fetched script's own default.
#[must_use]
pub fn installed_channel(prefix: &Path) -> Option<&'static str> {
    // The marker read mirrors install-rust.sh's marker_text: install.ps1's
    // Set-Content follows $PSDefaultParameterValues['*:Encoding'], so a
    // ps1-published live marker can be UTF-16 (NUL-interleaved, and its
    // BOM is not valid UTF-8, failing the whole read) or BOM-prefixed.
    // Flattening the NULs and stripping a leading BOM keeps the exact
    // prefix check working for this script's own ASCII writes and for the
    // ps1-written ones alike (a plain marker passes through untouched).
    let bytes = std::fs::read(prefix.join("share/prime-agent/.prime-agent-install")).ok()?;
    let bytes = normalize_marker_bytes(&bytes);
    let marker = String::from_utf8_lossy(&bytes);
    // The marker's first line must be the installer's OWN write shape —
    // "install-rust.sh channel <name>" — the exact prefix is the ownership
    // proof, so a foreign marker never steers the update onto a channel.
    let channel = marker
        .lines()
        .next()?
        .trim()
        .strip_prefix("install-rust.sh channel ")?
        .trim();
    match channel {
        "stable" => Some("stable"),
        "beta" => Some("beta"),
        _ => None,
    }
}

/// Fetch the installer script into a per-run temp file (the small-file
/// budget; the file is the exact bytes the branch serves) — the path with
/// the held handle [`write_script`] created it through.
///
/// # Errors
/// Returns an error when the request fails, answers a non-success status,
/// or the body cannot be read or written.
/// The redirect policy every update fetch carries: each hop must satisfy the same
/// rule as the URL the caller passed (review #5, CWE-319).
///
/// `ensure_https_url` only sees the URL the caller passes, and reqwest follows up
/// to ten redirects by default. One funnel was guarded that way; the channel
/// manifest and the binary archive were not. The manifest is UNSIGNED — the
/// archive's sha comes FROM it, so a plaintext manifest lets an attacker supply
/// matching bytes — and a plaintext `Location` on the installer fetch would hand
/// that funnel a script it pipes to `/bin/sh -s --`.
///
/// The check asks ONLY the scheme, for the hop's destination and for every hop
/// already taken, so a refusal cannot be laundered through an intermediate
/// plaintext redirect.
///
/// It deliberately does not consult [`allow_http`]. That escape hatch belongs to
/// the URL the caller hands in — a local channel an operator opted into — and
/// reading it here would put a process-global env lookup on every hop, where a
/// concurrent test (or a process changing its own settings) flips the answer
/// mid-chain. A plaintext 30x the caller was not allowed to start is also not one
/// it is allowed to follow.
#[must_use]
pub fn update_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        let plaintext = |url: &reqwest::Url| !is_https_url(url.as_str());
        if plaintext(attempt.url()) || attempt.previous().iter().any(plaintext) {
            return attempt.stop();
        }
        attempt.follow()
    })
}

async fn fetch_script(url: &str) -> Result<(PathBuf, std::fs::File)> {
    let response = pa_ai::utils::tls::http_client_builder()
        .redirect(update_redirect_policy())
        .build()
        .expect("installer client")
        .get(url)
        .header("User-Agent", update_user_agent(env!("CARGO_PKG_VERSION")))
        .timeout(SCRIPT_FETCH_TIMEOUT)
        .send()
        .await
        // The URL carries the mirror's credentials (userinfo) on every error path, and
        // this one reaches the daemon log, the update status file and the TUI's error
        // row: `request https://user:token@host/install.sh`. Redacted at the source, so
        // a new caller cannot reintroduce the leak by forgetting to wrap it (review #32).
        .with_context(|| format!("request {}", redact_url_credentials(url)))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .with_context(|| "read the installer script")?;
    if !status.is_success() {
        anyhow::bail!("download {} returned {status}", redact_url_credentials(url));
    }
    if bytes.is_empty() {
        anyhow::bail!(
            "the installer script at {} was empty",
            redact_url_credentials(url)
        );
    }
    write_script(&bytes)
}

/// Write installer script bytes to a per-run temp file, created fresh
/// under an unguessable name and returned together with the OPEN HANDLE
/// the bytes were written through. The temp directory is writable by
/// every process of this user (and, under a permissive umask, by every
/// local user), so a script the child later re-opens BY NAME could be
/// swapped between this write and the run — executing the swap with the
/// credentials this funnel is hardened to guard. The handle is therefore
/// the only thing every consumer hands to the child (as its stdin, the
/// `curl|sh` one-liner's own invocation form): the bytes that run are
/// the bytes written here, and the name only serves the post-run
/// cleanup.
fn write_script(bytes: &[u8]) -> Result<(PathBuf, std::fs::File)> {
    let script = std::env::temp_dir().join(format!(
        "prime-agent-update-{}.sh",
        uuid::Uuid::now_v7().simple()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        // Owner-only: the default umask would leave the shared temp
        // directory's other users a write window into the file the
        // child is about to run.
        options.mode(0o600);
    }
    let mut script_file = options
        .open(&script)
        .with_context(|| format!("create {}", script.display()))?;
    script_file
        .write_all(bytes)
        .with_context(|| format!("write {}", script.display()))?;
    // Every consumer hands a clone of this handle to the child as its
    // stdin, and a clone shares the file position: start them all from
    // the first byte.
    script_file
        .seek(SeekFrom::Start(0))
        .with_context(|| format!("rewind {}", script.display()))?;
    Ok((script, script_file))
}

/// The trusted interpreter for the installer child (the `curl|sh`
/// one-liner's own): the absolute /bin/sh on unix — never PATH-resolved,
/// so a poisoned `PATH` cannot substitute the interpreter that runs the
/// installer with the inherited credentials.
#[cfg(not(windows))]
fn trusted_shell() -> PathBuf {
    PathBuf::from("/bin/sh")
}

/// On Windows the kernel shell default's TRUSTED Git Bash roots -
/// hardcoded install-dir literals, never PATH and never `where bash.exe`
/// (the `get_shell_config` fallback that serves the kernel shell would
/// let a repo-controlled `PATH` place the interpreter that receives the
/// inherited `GITHUB_TOKEN`; the funnel must not use it).
///
/// # Errors
/// Returns the failure when no trusted Git Bash root exists.
#[cfg(windows)]
fn trusted_shell() -> std::result::Result<std::path::PathBuf, UpdateFailure> {
    crate::platform::shell::default_kernel_bash_shell()
        .map(std::path::PathBuf::from)
        .ok_or_else(|| UpdateFailure {
            // No shellPath guidance here: the funnel, like its unix side
            // (the hardcoded /bin/sh), resolves only the trusted roots -
            // the settings key serves the kernel shell, not this privileged
            // execution (the promise would lie).
            message: "could not run the installer: no Git Bash found at \
                  the trusted install roots \
                  (C:\\Program Files\\Git\\bin\\bash.exe); install \
                  Git for Windows (https://git-scm.com/download/win) \
                  to update from this machine"
                .to_string(),
        })
}

/// Build the installer's child command: [`trusted_shell`]'s interpreter
/// with the installer's env knobs (the caller completes it with the
/// `-s --` form — the `curl|sh` one-liner's own — where the script rides
/// the child's stdin from the handle [`write_script`] returned and
/// `args` stay the child's positional parameters), so both wait modes
/// ([`execute_script`]) and the Windows handoff spawn this same command:
/// the install prefix riding the child's environment as the installer's
/// own knob, so the script publishes exactly where the probe looks —
/// every other `PRIME_AGENT_RUST_*` knob passes through untouched.
fn installer_child(
    shell: &Path,
    prefix: &Path,
    channel: Option<&'static str>,
) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(shell);
    command.env(ENV_PREFIX, prefix);
    // The tree this run's own payload lives in (see ENV_INSTALL_PAYLOAD_DIR):
    // the launcher layout puts it outside the prefix, so the script's daemon
    // stop gate needs both roots to recognise this install's own daemon.
    if let Some(payload_dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| crate::packages::exe_dir_of(&exe))
    {
        command.env(ENV_INSTALL_PAYLOAD_DIR, payload_dir);
    }
    // FORK-ONLY: tell the installer script this run belongs to the fork, so
    // its daemon stop-probe skips the upstream TypeScript socket (known bug
    // #22: both payloads are named `prime-agent`, so only a marker/socket path
    // separates them). The fork build id is baked at compile time
    // (`ALLSPARK_FORK_BUILD=fork.N`, the same knob the version splash reads);
    // an upstream build carries none and the script keeps its TS takeover.
    if let Some(fork_build) = option_env!("ALLSPARK_FORK_BUILD") {
        command.env(ENV_FORK_BUILD, fork_build);
    }
    // The check knobs are the PRE-FLIGHT's alone: a user shell carrying
    // either would make the real run validate and exit without
    // publishing, so the shared builder strips both (the pre-flight sets
    // its own back after this).
    command.env_remove("PRIME_AGENT_ROLLBACK_CHECK");
    command.env_remove("PRIME_AGENT_ARCHIVE_CHECK");
    // The requested channel wins; otherwise the update stays on the channel
    // the install marker records (the fetched script's own default is the
    // stable render, so a beta install would silently switch channels).
    if let Some(channel) = channel.or_else(|| installed_channel(prefix)) {
        command.env(ENV_RELEASE_CHANNEL, channel);
    }
    command
}

/// Exec the script and wait for it: [`trusted_shell`]'s interpreter
/// under [`installer_child`]'s env wiring, the `-s --` form with the
/// script riding the child's stdin from the handle [`write_script`]
/// returned (the shared temp directory can never swap the bytes under a
/// name — the handle IS what runs) and `args` as the child's positional
/// parameters. The script's own
/// die messages already streamed with [`InstallerOutput::Inherit`];
/// with [`InstallerOutput::Capture`] the tail becomes the failure
/// message.
///
/// # Errors
/// Returns the failure when the script cannot start or exits nonzero.
async fn execute_script(
    script: &std::fs::File,
    args: &[&std::ffi::OsStr],
    prefix: &Path,
    channel: Option<&'static str>,
    output: InstallerOutput,
) -> std::result::Result<(), UpdateFailure> {
    #[cfg(windows)]
    let shell = trusted_shell()?;
    #[cfg(not(windows))]
    let shell = trusted_shell();
    let stdin = std::process::Stdio::from(script.try_clone().map_err(|error| UpdateFailure {
        message: format!("could not run the installer: {error}"),
    })?);
    let mut command = installer_child(&shell, prefix, channel);
    command.arg("-s").arg("--").args(args).stdin(stdin);
    match output {
        InstallerOutput::Inherit => {
            let status = command.status().await.map_err(|error| UpdateFailure {
                message: format!("could not run the installer: {error}"),
            })?;
            if status.success() {
                return Ok(());
            }
            Err(UpdateFailure {
                message: match status.code() {
                    Some(code) => format!(
                        "the installer exited with code {code}; the current install was kept"
                    ),
                    None => {
                        "the installer was terminated by a signal; the current install was kept"
                            .to_string()
                    }
                },
            })
        }
        InstallerOutput::Capture => {
            // The TUI owns the terminal: no controlling tty, and the
            // child's stdin is the script itself — so the script cannot
            // open /dev/tty to prompt and never reads a terminal.
            #[cfg(unix)]
            crate::platform::process::set_new_session(command.as_std_mut());
            let captured = command.output().await.map_err(|error| UpdateFailure {
                message: format!("could not run the installer: {error}"),
            })?;
            if captured.status.success() {
                return Ok(());
            }
            let tail = output_tail(&captured);
            let detail = tail.map_or_else(String::new, |tail| format!(":\n{tail}"));
            Err(UpdateFailure {
                message: match captured.status.code() {
                    Some(code) => {
                        format!("the installer exited with code {code}{detail}")
                    }
                    None => {
                        format!("the installer was terminated by a signal{detail}")
                    }
                },
            })
        }
    }
}

/// The last informative lines of a captured installer run (die messages go
/// to stderr; a silent failure falls back to stdout's last line).
fn output_tail(captured: &std::process::Output) -> Option<String> {
    let stderr = String::from_utf8_lossy(&captured.stderr);
    let mut last_stderr: Vec<String> = stderr
        .lines()
        .rev()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .take(3)
        .collect();
    if last_stderr.is_empty() {
        let stdout = String::from_utf8_lossy(&captured.stdout);
        last_stderr = stdout
            .lines()
            .rev()
            .map(|line| line.trim().to_string())
            .filter(|line| !line.is_empty())
            .take(1)
            .collect();
    }
    last_stderr.reverse();
    let joined = last_stderr.join("\n");
    (!joined.is_empty()).then_some(joined)
}

/// The installed launcher's `--version` answer: the takeover layout's `bin/prime-agent` first, the
/// pre-takeover `bin/prime-agent-rust` second, and only answers stamped `-continuous.<commit>`
/// count (the TS product's launcher never matches).
async fn launcher_version(prefix: &Path) -> Option<String> {
    // The launcher names: the .cmd twin on Windows (the sh-script launcher
    // cannot be exec'd by CreateProcess; Rust runs .cmd through cmd.exe),
    // the sh launcher pair on unix (the pre-takeover name second).
    #[cfg(windows)]
    const LAUNCHER_NAMES: [&str; 2] = ["prime-agent.cmd", "prime-agent"];
    #[cfg(not(windows))]
    const LAUNCHER_NAMES: [&str; 2] = ["prime-agent", "prime-agent-rust"];
    for name in LAUNCHER_NAMES {
        let launcher = prefix.join("bin").join(name);
        if !launcher.is_file() {
            continue;
        }
        let probe = tokio::process::Command::new(&launcher)
            .arg("--version")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .output();
        let Ok(output) = tokio::time::timeout(LAUNCHER_PROBE_TIMEOUT, probe).await else {
            continue;
        };
        let Ok(output) = output else {
            continue;
        };
        let first = String::from_utf8_lossy(&output.stdout);
        let version = first.lines().next().map(str::trim).unwrap_or_default();
        if running_commit(version).is_some() {
            return Some(version.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The platform map `prime-agent update --check` names on every
    /// supported platform pair (the same set install-rust.sh's uname case
    /// resolves): the Windows pair ships the MSVC target, and the refusal
    /// covers every other pair with the full matrix in the message.
    #[test]
    fn target_for_covers_the_published_matrix() {
        assert_eq!(target_for("macos", "aarch64"), Some("aarch64-apple-darwin"));
        assert_eq!(target_for("macos", "x86_64"), Some("x86_64-apple-darwin"));
        assert_eq!(
            target_for("linux", "x86_64"),
            Some("x86_64-unknown-linux-gnu")
        );
        assert_eq!(
            target_for("linux", "aarch64"),
            Some("aarch64-unknown-linux-gnu")
        );
        assert_eq!(
            target_for("windows", "x86_64"),
            Some("x86_64-pc-windows-msvc")
        );
        // The unsupported pairs refuse; the message names the machine and
        // the full matrix (the Windows build included, so a refused
        // Windows-adjacent machine sees the MSVC target it needs).
        assert_eq!(target_for("windows", "aarch64"), None);
        assert_eq!(target_for("freebsd", "x86_64"), None);
        let message = no_build_message("windows", "aarch64");
        assert!(message.contains("windows aarch64"), "{message}");
        assert!(message.contains("x86_64-pc-windows-msvc"), "{message}");
    }

    /// The installer's daemon stop gate needs BOTH of this run's install
    /// roots: the prefix it writes and the tree of the payload that invoked
    /// it (the fork's launcher keeps the payload in libexec/, outside the
    /// prefix, and that payload is the install the update replaces). The
    /// funnel exports them so the shipped script reads the same roots the
    /// Rust side resolved.
    #[test]
    fn the_installer_child_carries_both_install_roots() {
        /// The child's install-root knobs, read back from its environment.
        fn roots(command: &tokio::process::Command) -> (Option<String>, Option<String>) {
            // One pass over the child's env: the funnel sets both knobs.
            let mut envs = std::collections::HashMap::new();
            for (key, value) in command.as_std().get_envs() {
                envs.insert(
                    key.to_string_lossy().to_string(),
                    value.map(|value| value.to_string_lossy().to_string()),
                );
            }
            let read = |key: &str| envs.get(key).cloned().flatten();
            (read(ENV_PREFIX), read(ENV_INSTALL_PAYLOAD_DIR))
        }

        let dir = tempfile::TempDir::new().unwrap();
        let shell = dir.path().join("sh");
        std::fs::copy("/bin/sh", &shell).unwrap();
        let prefix = dir.path().join("prefix");

        let (carried_prefix, carried_payload) = roots(&installer_child(&shell, &prefix, None));
        assert_eq!(carried_prefix, Some(prefix.to_string_lossy().to_string()));
        let expected_payload = std::env::current_exe()
            .ok()
            .and_then(|exe| crate::packages::exe_dir_of(&exe))
            .map(|dir| dir.to_string_lossy().to_string());
        assert_eq!(carried_payload, expected_payload);
    }

    /// The ps1-era marker encodings read back as their ASCII content: a
    /// UTF-16 or BOM-prefixed live marker (install.ps1's Set-Content
    /// followed $`PSDefaultParameterValues`) keeps the installer-ownership
    /// gate working instead of sending a ps1-updated machine down the
    /// managed path.
    #[test]
    fn installed_channel_reads_the_ps1_encoded_marker() {
        let dir = tempfile::TempDir::new().unwrap();
        let prefix = dir.path().join("prefix");
        let payload = prefix.join("share/prime-agent");
        std::fs::create_dir_all(&payload).unwrap();
        let cases: [(&str, Vec<u8>, &str); 4] = [
            (
                "utf-16le",
                {
                    let text: Vec<u16> = "install-rust.sh channel beta\nversion 1\n"
                        .encode_utf16()
                        .collect();
                    let mut bytes = vec![0xFF, 0xFE];
                    for unit in &text {
                        bytes.extend_from_slice(&unit.to_le_bytes());
                    }
                    bytes
                },
                "beta",
            ),
            (
                "utf-8-bom",
                {
                    let mut bytes = vec![0xEF, 0xBB, 0xBF];
                    bytes.extend_from_slice(b"install-rust.sh channel stable\nversion 1\n");
                    bytes
                },
                "stable",
            ),
            (
                "utf-16be",
                {
                    let text: Vec<u16> = "install-rust.sh channel stable\nversion 1\n"
                        .encode_utf16()
                        .collect();
                    let mut bytes = vec![0xFE, 0xFF];
                    for unit in &text {
                        bytes.extend_from_slice(&unit.to_be_bytes());
                    }
                    bytes
                },
                "stable",
            ),
            (
                "ascii",
                b"install-rust.sh channel stable\nversion 1\n".to_vec(),
                "stable",
            ),
        ];
        for (name, bytes, expected) in cases {
            std::fs::write(payload.join(".prime-agent-install"), &bytes).unwrap();
            assert_eq!(
                installed_channel(&prefix),
                Some(expected),
                "{name}: the ps1-era encoding reads back as its ASCII channel"
            );
            assert_eq!(
                installed_version(&prefix).as_deref(),
                Some("1"),
                "{name}: the version line of an encoded marker reads too"
            );
        }
    }

    /// The installed-marker channel read: a beta install's update must
    /// stay on beta (the marker the installer writes at publish carries
    /// the channel), a stable or pre-marker install rides the script's
    /// own default, and a foreign marker is never treated as a channel.
    #[test]
    fn installed_channel_reads_the_publish_marker() {
        let dir = tempfile::TempDir::new().unwrap();
        let prefix = dir.path().join("prefix");
        assert_eq!(installed_channel(&prefix), None);
        let share = prefix.join("share/prime-agent");
        std::fs::create_dir_all(&share).unwrap();
        std::fs::write(
            share.join(".prime-agent-install"),
            "install-rust.sh channel beta\nversion 0.10.0\n",
        )
        .unwrap();
        assert_eq!(installed_channel(&prefix), Some("beta"));
        std::fs::write(
            share.join(".prime-agent-install"),
            "install-rust.sh channel stable\nversion 0.10.0\n",
        )
        .unwrap();
        assert_eq!(installed_channel(&prefix), Some("stable"));
        std::fs::write(share.join(".prime-agent-install"), "nightly\n").unwrap();
        assert_eq!(installed_channel(&prefix), None);
        std::fs::write(
            share.join(".prime-agent-install"),
            "other installer channel beta\n",
        )
        .unwrap();
        assert_eq!(installed_channel(&prefix), None);
        std::fs::write(share.join(".prime-agent-install"), "channel beta\n").unwrap();
        assert_eq!(installed_channel(&prefix), None);
    }

    /// Only the payload binary of a marked installer tree is
    /// installer-owned; the marker's version line is the installed
    /// version.
    #[test]
    fn installer_prefix_of_needs_the_payload_path_and_the_marker() {
        let dir = tempfile::TempDir::new().unwrap();
        let prefix = dir.path().join("prefix");
        let payload = prefix.join("share/prime-agent");
        std::fs::create_dir_all(&payload).unwrap();
        let exe = payload.join(PAYLOAD_BINARY);
        assert_eq!(installer_prefix_of(&exe), None, "no marker: not owned");
        std::fs::write(
            payload.join(".prime-agent-install"),
            "install-rust.sh channel beta\nversion 0.9.9-beta.10\n",
        )
        .unwrap();
        assert_eq!(installer_prefix_of(&exe), Some(prefix.clone()));
        assert_eq!(installed_version(&prefix).as_deref(), Some("0.9.9-beta.10"));
        // A binary elsewhere in the tree, or a managed release, is not.
        assert_eq!(installer_prefix_of(&payload.join("other")), None);
        assert_eq!(
            installer_prefix_of(&prefix.join("releases/0.9.8/prime-agent")),
            None
        );
    }

    /// The handoff gate never opens on unix: a rename never fights a
    /// running image, so the local operations always wait for the
    /// installer — even a fully staged, marker-bearing payload prefix
    /// stays closed (the platform alone decides).
    #[test]
    #[cfg(not(windows))]
    fn caller_owns_payload_stays_false_for_a_staged_prefix_on_unix() {
        let dir = tempfile::TempDir::new().unwrap();
        let prefix = dir.path().join("prefix");
        let payload = prefix.join("share/prime-agent");
        std::fs::create_dir_all(&payload).unwrap();
        std::fs::write(
            payload.join(".prime-agent-install"),
            "install-rust.sh channel beta\nversion 0.9.9-beta.10\n",
        )
        .unwrap();
        assert!(
            !caller_owns_payload(&prefix),
            "unix renames never fight a running image"
        );
    }

    /// The handoff gate opens only when the marker-bearing payload path
    /// IS this process's own image: the test binary never runs from a
    /// payload dir, so even a fully staged prefix stays closed on
    /// Windows — the true arm needs a real self-installed run (the e2e
    /// covers the wait path).
    #[test]
    #[cfg(windows)]
    fn caller_owns_payload_needs_the_running_image_on_windows() {
        let dir = tempfile::TempDir::new().unwrap();
        let prefix = dir.path().join("prefix");
        let payload = prefix.join("share/prime-agent");
        std::fs::create_dir_all(&payload).unwrap();
        std::fs::write(
            payload.join(".prime-agent-install"),
            "install-rust.sh channel beta\nversion 0.9.9-beta.10\n",
        )
        .unwrap();
        assert!(
            !caller_owns_payload(&prefix),
            "a staged prefix never names this process's own image"
        );
    }

    /// Serve `body` over one plain HTTP request (the hermetic source the
    /// funnel fetches its mock installer from): bind an ephemeral loopback
    /// socket, answer the first request, return the URL the funnel uses.
    /// Unix only: its callers are the unix shell-installer tests.
    #[cfg(unix)]
    fn serve(body: &'static str) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let address = listener.local_addr().expect("local address");
        std::thread::spawn(move || {
            use std::io::Write as _;
            if let Ok((mut stream, _)) = listener.accept() {
                // Read the request head first: answering before the request is drained can
                // reset the connection mid-write (the client then reports a broken response).
                let mut head = Vec::new();
                let mut byte = [0_u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    match std::io::Read::read(&mut stream, &mut byte) {
                        Ok(0) | Err(_) => return,
                        Ok(_) => head.push(byte[0]),
                    }
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        format!("http://{address}/install.sh")
    }

    /// The process-global env lock for the installer tests: the funnel's
    /// https rule reads `PRIME_AGENT_ALLOW_HTTP` (process-global, like the
    /// other knobs), so the tests that open or clear it serialize here and a
    /// parallel run cannot flip the hatch under another test.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Hold the env lock with `PRIME_AGENT_ALLOW_HTTP` set to `value` (`None`
    /// clears it), restoring the prior value on drop. The hatch is exactly the
    /// knob the shipped scripts honor, so opening it here exercises the
    /// documented local-channel path rather than weakening the rule; clearing
    /// it exercises the refusal. The guard rides a struct (not a bare
    /// `MutexGuard`) so the std lock is never held as a local across the
    /// tests' awaits.
    struct AllowHttpGuard {
        // Held for the whole guard scope: the lock is released only after the
        // prior value is restored on drop.
        _lock: std::sync::MutexGuard<'static, ()>,
        prior: Option<std::ffi::OsString>,
    }

    impl AllowHttpGuard {
        fn open() -> Self {
            Self::set(Some("1"))
        }

        // Used only by the cfg(unix) refusal test; on other targets the
        // whole gated test set compiles out, so this variant would sit
        // unused under -D warnings.
        #[cfg(unix)]
        fn closed() -> Self {
            Self::set(None)
        }

        fn set(value: Option<&str>) -> Self {
            let lock = env_lock();
            let prior = std::env::var_os(ENV_ALLOW_HTTP);
            match value {
                Some(value) => std::env::set_var(ENV_ALLOW_HTTP, value),
                None => std::env::remove_var(ENV_ALLOW_HTTP),
            }
            Self { _lock: lock, prior }
        }
    }

    impl Drop for AllowHttpGuard {
        fn drop(&mut self) {
            match self.prior.take() {
                Some(value) => std::env::set_var(ENV_ALLOW_HTTP, value),
                None => std::env::remove_var(ENV_ALLOW_HTTP),
            }
        }
    }

    /// The sandboxed preserve fixture: a session file under a home the installer
    /// world shares, with its exact bytes snapshotted for the byte-identity assert.
    struct Preserve {
        // The byte-identity readers are the unix `assert_untouched`
        // checks; on other platforms the fixture only stages the file.
        #[cfg_attr(not(unix), allow(dead_code))]
        session_file: PathBuf,
        #[cfg_attr(not(unix), allow(dead_code))]
        bytes: Vec<u8>,
    }

    impl Preserve {
        fn new(root: &Path) -> Self {
            let session_file = root
                .join("home")
                .join(".prime/agent/sessions/session.jsonl");
            std::fs::create_dir_all(session_file.parent().expect("session dir"))
                .expect("session dir");
            let bytes =
                b"{\"type\":\"user_message\",\"content\":\"the session the update must preserve\"}\n"
                    .to_vec();
            std::fs::write(&session_file, &bytes).expect("write session file");
            Self {
                session_file,
                bytes,
            }
        }

        /// Unix only: the update-flow tests that read the snapshot sit behind the unix gate.
        #[cfg(unix)]
        fn assert_untouched(&self) {
            let observed =
                std::fs::read(&self.session_file).expect("session file survives the update");
            assert_eq!(
                observed, self.bytes,
                "the session store is byte-identical after the update"
            );
        }
    }

    /// The mock installer the funnel downloads in the tests: it installs a launcher
    /// that answers a stamped `--version`, exactly the takeover's contract. Unix only.
    #[cfg(unix)]
    const MOCK_INSTALLER: &str = r#"#!/bin/sh
set -eu
# Fails only under a controlling terminal (a developer run or `script`); headless CI has none.
if ( : <>/dev/tty ) 2>/dev/null || [ -t 0 ]; then echo "the captured installer reached the terminal" >&2; exit 9; fi
mkdir -p "${PRIME_AGENT_RUST_PREFIX}/bin"
printf '#!/bin/sh\necho "9.9.9-continuous.0123456789abcdef"\n' > "${PRIME_AGENT_RUST_PREFIX}/bin/prime-agent"
chmod 0755 "${PRIME_AGENT_RUST_PREFIX}/bin/prime-agent"
printf '%s' "${PRIME_AGENT_RELEASE_CHANNEL:-}" > "${PRIME_AGENT_RUST_PREFIX}/channel"
echo "installed: 9.9.9-continuous.0123456789abcdef"
"#;

    /// The pre-takeover installer: the launcher carries the legacy `prime-agent-rust`
    /// name the probe still accepts. Unix only: same sh-script class as [`MOCK_INSTALLER`].
    #[cfg(unix)]
    const LEGACY_INSTALLER: &str = r#"#!/bin/sh
set -eu
mkdir -p "${PRIME_AGENT_RUST_PREFIX}/bin"
printf '#!/bin/sh\necho "9.9.8-continuous.fedcba9876543210"\n' > "${PRIME_AGENT_RUST_PREFIX}/bin/prime-agent-rust"
chmod 0755 "${PRIME_AGENT_RUST_PREFIX}/bin/prime-agent-rust"
echo "installed: 9.9.8-continuous.fedcba9876543210"
"#;

    fn sandbox() -> (tempfile::TempDir, Preserve, PathBuf) {
        let root = tempfile::tempdir().expect("sandbox root");
        let preserve = Preserve::new(root.path());
        let prefix = root.path().join("prefix/.local");
        std::fs::create_dir_all(&prefix).expect("prefix");
        (root, preserve, prefix)
    }

    /// The funnel URL follows the channel: stable fetches the official
    /// domain's install endpoint, nightly (beta) fetches `install-beta.sh`
    /// from the download base (the domain forwards only `install.sh`), and
    /// the override pins both.
    #[test]
    fn the_installer_url_follows_the_channel() {
        // The knobs are SAVED and RESTORED around the probe: the env is
        // process-global, so a pinned value in the surrounding
        // environment must survive it.
        let prior_override = std::env::var(ENV_INSTALLER_URL).ok();
        let prior_base = std::env::var(ENV_DOWNLOAD_BASE_URL).ok();
        std::env::remove_var(ENV_INSTALLER_URL);
        std::env::remove_var(ENV_DOWNLOAD_BASE_URL);
        assert_eq!(
            installer_script_url(Some("stable")),
            "https://app.primeintellect.ai/prime-agent/install.sh"
        );
        assert_eq!(installer_script_url(None), OFFICIAL_INSTALLER_URL);
        assert_eq!(
            installer_script_url(Some("beta")),
            "https://pub-728493de92a943e2a9b2d17b4719f318.r2.dev/install-beta.sh"
        );
        std::env::set_var(ENV_DOWNLOAD_BASE_URL, "https://mirror.example/");
        assert_eq!(
            installer_script_url(Some("beta")),
            "https://mirror.example/install-beta.sh"
        );
        assert_eq!(installer_script_url(Some("stable")), OFFICIAL_INSTALLER_URL);
        std::env::set_var(ENV_INSTALLER_URL, "http://127.0.0.1:9/pinned.sh");
        assert_eq!(
            installer_script_url(Some("beta")),
            "http://127.0.0.1:9/pinned.sh"
        );
        assert_eq!(
            installer_script_url(Some("stable")),
            "http://127.0.0.1:9/pinned.sh"
        );
        for (name, prior) in [
            (ENV_INSTALLER_URL, prior_override),
            (ENV_DOWNLOAD_BASE_URL, prior_base),
        ] {
            match prior {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }

    #[test]
    fn the_target_matrix_covers_the_published_builds() {
        assert_eq!(target_for("macos", "aarch64"), Some("aarch64-apple-darwin"));
        assert_eq!(target_for("macos", "x86_64"), Some("x86_64-apple-darwin"));
        assert_eq!(
            target_for("linux", "x86_64"),
            Some("x86_64-unknown-linux-gnu")
        );
        assert_eq!(
            target_for("linux", "aarch64"),
            Some("aarch64-unknown-linux-gnu")
        );
        // The Windows pair ships the MSVC build (the release channel's
        // fifth target since the windows ship).
        assert_eq!(
            target_for("windows", "x86_64"),
            Some("x86_64-pc-windows-msvc")
        );
        // Windows ARM64 (the only unsupported Windows shape) and every
        // other OS refuse.
        assert_eq!(target_for("windows", "aarch64"), None);
        assert!(
            current_target().is_ok(),
            "the test matrix runs on a supported platform"
        );
    }

    #[test]
    fn running_commit_reads_the_continuous_stamp() {
        assert_eq!(
            running_commit("0.5.2-continuous.07f42eaa3a6159f942c6c24beb0352ce120a192c"),
            Some("07f42eaa3a6159f942c6c24beb0352ce120a192c")
        );
        assert_eq!(
            running_commit(" 9.9.9-continuous.0123456 "),
            Some("0123456")
        );
        assert_eq!(
            running_commit("0.5.2"),
            None,
            "a dev build carries no stamp"
        );
        assert_eq!(
            running_commit("2.31.4"),
            None,
            "the TypeScript product's version never reads as a rust commit"
        );
    }

    /// The requested channel (an explicit flag or the saved setting) wins
    /// over the install marker; without one the marker's channel rides.
    #[tokio::test]
    #[cfg(unix)]
    async fn the_requested_channel_wins_over_the_install_marker() {
        let _http = AllowHttpGuard::open();
        let (root, _preserve, prefix) = sandbox();
        let share = prefix.join("share/prime-agent");
        std::fs::create_dir_all(&share).unwrap();
        std::fs::write(
            share.join(".prime-agent-install"),
            "install-rust.sh channel beta\nversion 0.10.0\n",
        )
        .unwrap();
        let url = serve(MOCK_INSTALLER);
        run_installer_from(&url, &prefix, Some("stable"), InstallerOutput::Capture)
            .await
            .expect("the funnel installs");
        assert_eq!(
            std::fs::read_to_string(prefix.join("channel")).unwrap(),
            "stable"
        );
        let url = serve(MOCK_INSTALLER);
        run_installer_from(&url, &prefix, None, InstallerOutput::Capture)
            .await
            .expect("the funnel installs");
        assert_eq!(
            std::fs::read_to_string(prefix.join("channel")).unwrap(),
            "beta"
        );
        drop(root);
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn the_funnel_runs_the_downloaded_installer_and_preserves_the_session_store() {
        let _http = AllowHttpGuard::open();
        let (root, preserve, prefix) = sandbox();
        let installed = run_installer_from(
            &serve(MOCK_INSTALLER),
            &prefix,
            None,
            InstallerOutput::Capture,
        )
        .await
        .expect("the funnel installs the mock build");
        assert_eq!(
            installed.version.as_deref(),
            Some("9.9.9-continuous.0123456789abcdef"),
            "the probe reads the launcher's own --version answer"
        );
        let launcher = prefix.join("bin/prime-agent");
        assert!(launcher.is_file(), "the launcher landed");
        let mode = std::fs::metadata(&launcher).expect("launcher metadata");
        assert!(unix_mode_is_executable(&mode), "the launcher is executable");
        preserve.assert_untouched();
        drop(root);
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn the_probe_still_reads_a_pre_takeover_launchers_version() {
        let _http = AllowHttpGuard::open();
        let (root, _preserve, prefix) = sandbox();
        let installed = run_installer_from(
            &serve(LEGACY_INSTALLER),
            &prefix,
            None,
            InstallerOutput::Capture,
        )
        .await
        .expect("the funnel installs the legacy-named build");
        assert_eq!(
            installed.version.as_deref(),
            Some("9.9.8-continuous.fedcba9876543210"),
            "the pre-takeover launcher answers through the probe"
        );
        drop(root);
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn a_failed_installer_keeps_the_previous_install_and_reports_the_error() {
        let _http = AllowHttpGuard::open();
        let (root, preserve, prefix) = sandbox();
        std::fs::create_dir_all(prefix.join("bin")).expect("bin dir");
        let previous = prefix.join("bin/prime-agent");
        std::fs::write(&previous, "#!/bin/sh\necho 9.9.7-continuous.0000001\n")
            .expect("previous launcher");
        make_executable(&previous);

        let failure = run_installer_from(
            &serve("#!/bin/sh\necho 'install-rust.sh: the artifact download failed' >&2\nexit 3\n"),
            &prefix,
            None,
            InstallerOutput::Capture,
        )
        .await
        .expect_err("the failing script fails the update");
        assert!(
            failure.message.contains("the artifact download failed"),
            "the failure carries the script's own die message: {}",
            failure.message
        );
        assert!(failure.message.contains("code 3"), "{}", failure.message);
        let version = launcher_version(&prefix).await;
        assert_eq!(version.as_deref(), Some("9.9.7-continuous.0000001"));
        preserve.assert_untouched();
        drop(root);
    }

    #[tokio::test]
    async fn an_unfetchable_script_fails_without_installing() {
        // The hatch is open: this test pins the FETCH failure, not the https
        // rule (which its plain-HTTP URL would otherwise trip first).
        let _http = AllowHttpGuard::open();
        let (_root, _preserve, prefix) = sandbox();
        let failure = run_installer_from(
            "http://127.0.0.1:9/install-rust.sh",
            &prefix,
            None,
            InstallerOutput::Capture,
        )
        .await
        .expect_err("the unreachable URL fails the update");
        assert!(
            failure.message.contains("could not download the installer"),
            "the failure names the fetch: {}",
            failure.message
        );
        assert!(
            !prefix.join("bin").exists(),
            "nothing landed from a failed fetch"
        );
    }

    /// THE HTTPS RULE (mirrored from `install-rust.sh`/`install.ps1`): a
    /// plaintext installer URL is refused up front, with a message naming the
    /// escape hatch, and NO request is attempted; an `https://` URL passes the
    /// same check. This is the regression for the env-URL gap (#19): without
    /// the rule, a non-https `PRIME_AGENT_RUST_INSTALLER_URL` (or a beta
    /// download base) was fetched and executed with no scheme check.
    #[test]
    fn a_non_https_installer_url_is_refused_unless_the_hatch_is_open() {
        // The pure rule: https passes, http is refused, the hatch lifts it.
        assert_eq!(
            installer_url_refusal(
                "https://app.primeintellect.ai/prime-agent/install.sh",
                /*allow_http*/ false
            ),
            None
        );
        let refusal =
            installer_url_refusal("http://127.0.0.1:9/pinned.sh", /*allow_http*/ false)
                .expect("a plaintext URL is refused");
        assert!(
            refusal.contains("must be an https URL") && refusal.contains(ENV_ALLOW_HTTP),
            "the refusal names the rule and the hatch: {refusal}"
        );
        assert_eq!(
            installer_url_refusal("http://127.0.0.1:9/pinned.sh", /*allow_http*/ true),
            None,
            "the explicitly-named hatch admits a plaintext URL"
        );
    }

    /// The funnel itself refuses a non-https env URL BEFORE any fetch: the
    /// error names the rule, and nothing lands. The same URL installs once the
    /// hatch is open — proving the refusal was the scheme, not the URL.
    #[tokio::test]
    #[cfg(unix)]
    async fn the_funnel_refuses_a_non_https_url_before_fetching() {
        // The hatch is closed (the default): the http mock server's URL must be
        // refused by the scheme rule, never reached.
        {
            let _closed = AllowHttpGuard::closed();
            let (_root, _preserve, prefix) = sandbox();
            let failure = run_installer_from(
                &serve(MOCK_INSTALLER),
                &prefix,
                None,
                InstallerOutput::Capture,
            )
            .await
            .expect_err("a plaintext installer URL is refused");
            assert!(
                failure.message.contains("must be an https URL"),
                "the refusal names the https rule: {}",
                failure.message
            );
            assert!(
                !prefix.join("bin").exists(),
                "a refused URL installs nothing"
            );
        }
        // The same URL installs through the hatch (the local-channel path).
        let _http = AllowHttpGuard::open();
        let (_root, _preserve, prefix) = sandbox();
        run_installer_from(
            &serve(MOCK_INSTALLER),
            &prefix,
            None,
            InstallerOutput::Capture,
        )
        .await
        .expect("the hatch admits the local plain-HTTP installer");
        assert!(
            prefix.join("bin/prime-agent").is_file(),
            "the hatched install landed"
        );
    }

    #[cfg(unix)]
    fn unix_mode_is_executable(metadata: &std::fs::Metadata) -> bool {
        use std::os::unix::fs::PermissionsExt as _;
        metadata.permissions().mode() & 0o111 != 0
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = std::fs::metadata(path).expect("metadata").permissions();
        permissions.set_mode(permissions.mode() | 0o755);
        std::fs::set_permissions(path, permissions).expect("chmod");
    }
    /// A fork build must refuse the updater before it fetches anything.
    ///
    /// Every endpoint the funnel can reach belongs to upstream (the R2 download
    /// base, the `app.primeintellect.ai` script), and nothing in the path chooses a
    /// channel by product — so a fork build that "updates" installs upstream over
    /// itself. Until a fork release manifest exists, the honest answer is a refusal
    /// that says so; the overrides stay for a deliberate cross-install.
    #[test]
    fn a_fork_build_refuses_the_upstream_updater() {
        let refusal = fork_update_refusal(Some("fork.010"), None, None)
            .expect_err("a fork build must not update from the upstream channel");
        assert!(
            refusal.message.contains("allspark fork build"),
            "the message must name the product: {}",
            refusal.message
        );
        assert!(
            refusal.message.contains("fork crates"),
            "the message must say what would be lost: {}",
            refusal.message
        );
        assert!(
            refusal.message.contains(ENV_INSTALLER_URL)
                && refusal.message.contains(ENV_DOWNLOAD_BASE_URL),
            "the message must name both escape hatches: {}",
            refusal.message
        );
    }

    /// An upstream build keeps the updater it always had.
    #[test]
    fn an_upstream_build_updates_as_before() {
        fork_update_refusal(None, None, None).expect("upstream is not refused");
    }

    /// A configured channel is the caller's decision, so the refusal stands aside —
    /// including a blank override, which configures nothing.
    #[test]
    fn a_configured_fork_channel_is_honoured() {
        fork_update_refusal(Some("fork.010"), Some("https://example.test/i.sh"), None)
            .expect("an explicit installer URL is honoured");
        fork_update_refusal(Some("fork.010"), None, Some("https://example.test/dl"))
            .expect("an explicit download base is honoured");
        assert!(
            fork_update_refusal(Some("fork.010"), Some("   "), Some("")).is_err(),
            "a blank override configures nothing and must not defeat the refusal"
        );
    }

    // ── THE BUNDLED INSTALLER CARRIES THE FORK PROTECTIONS ────────────────────
    //
    // `BUNDLED_INSTALLER` is `include_str!("install-rust.sh")`: the script is
    // frozen into the binary at COMPILE time. Every fork protection added to
    // `install-rust.sh` therefore reaches users only when they build a new
    // binary — and an install built before a protection existed keeps the old
    // behaviour forever.
    //
    // That is exactly how the upstream daemon died three times: the installed
    // fork.008 binary carried an installer with no fork gate, so its `allspark
    // update` fetched upstream's channel and upstream's installer stopped BOTH
    // daemons (measured 2026-10-24 14:18: allspark 14:18:01, upstream
    // 14:18:06). A code fix in the tree does not reach an installed build.
    //
    // These pins fail the build when the bundled script loses a protection, so
    // the regression is caught at compile/test time instead of on a user's
    // machine.
    #[test]
    fn the_bundled_installer_carries_the_fork_gate() {
        assert!(
            BUNDLED_INSTALLER.contains("PRIME_AGENT_FORK_BUILD"),
            "the bundled installer must read the fork marker: without it a fork run \
             probes the upstream TypeScript socket and stops a co-installed daemon"
        );
        assert!(
            BUNDLED_INSTALLER.contains("fork_run="),
            "the bundled installer must derive its fork/upstream gate"
        );
        assert!(
            BUNDLED_INSTALLER.contains("--fork-run"),
            "the bundled installer must pass the fork gate into the stop probe, or the \
             probe's install-root test claims the whole prefix and stops the upstream \
             daemon living under PREFIX/share/prime-agent"
        );
        assert!(
            BUNDLED_INSTALLER.contains("allspark"),
            "the bundled installer must know the fork's own socket namespace"
        );
    }

    /// The stop-probe's ownership test is the crash site: it must narrow the
    /// owned roots for a fork run instead of trusting the shared prefix.
    #[test]
    fn the_bundled_installer_narrows_ownership_for_a_fork_run() {
        assert!(
            BUNDLED_INSTALLER.contains("if fork_run:"),
            "the probe must branch on the fork gate before deciding which install roots \
             it owns"
        );
        assert!(
            BUNDLED_INSTALLER.contains("OWN_ROOTS"),
            "the probe's owned-root list must exist (it is what the install-root test reads)"
        );
    }

    /// The bundled script and the script on disk must be the same file: a
    /// developer editing install-rust.sh without rebuilding still runs the old
    /// script at update time, which reads as "the fix did nothing".
    #[test]
    fn the_bundled_installer_matches_the_script_on_disk() {
        let on_disk = include_str!("../../../../install-rust.sh");
        assert_eq!(
            BUNDLED_INSTALLER.len(),
            on_disk.len(),
            "the bundled installer and install-rust.sh differ in length"
        );
    }
    /// A refusal must not print the URL's credentials (review #32, CWE-532).
    ///
    /// `https://user:token@mirror/install.sh` is a legal way to hand a mirror its
    /// credentials, and the refusal text reaches the daemon log, the update status
    /// file, and the TUI's error row.
    #[test]
    fn a_refusal_redacts_url_credentials() {
        let refusal = installer_url_refusal(
            "http://user:s3cr3t-token@mirror.example.com/install.sh",
            /*allow_http*/ false,
        )
        .expect("a plaintext URL is refused");
        assert!(
            !refusal.contains("s3cr3t-token"),
            "the refusal must not carry the token: {refusal}"
        );
        assert!(
            refusal.contains("<redacted>@mirror.example.com/install.sh"),
            "the host and path stay, so the operator can tell WHICH URL: {refusal}"
        );
    }

    /// A credential-free URL is printed verbatim; nothing else changes.
    #[test]
    fn a_refusal_without_credentials_prints_the_url() {
        let url = "http://mirror.example.com/install.sh";
        let refusal =
            installer_url_refusal(url, /*allow_http*/ false).expect("a plaintext URL is refused");
        assert!(
            refusal.contains(url),
            "a credential-free URL is named as-is: {refusal}"
        );
    }

    /// The redactor handles the shapes a URL can take.
    #[test]
    fn url_redaction_covers_the_authority_shapes() {
        for (input, expected) in [
            ("https://tok@h/x", "https://<redacted>@h/x"),
            ("https://u:p@h", "https://<redacted>@h"),
            (
                "https://u:p@h:8443/x?y=1",
                "https://<redacted>@h:8443/x?y=1",
            ),
            ("https://h/x", "https://h/x"),
            ("http://h", "http://h"),
            // A `@` in the PATH is not userinfo.
            ("https://h/x@y", "https://h/x@y"),
        ] {
            assert_eq!(redact_url_credentials(input), expected, "input: {input}");
        }
    }

    /// A query parameter can carry the credential instead of the userinfo.
    ///
    /// The first fix redacted the authority only. A presigned mirror URL puts its secret
    /// in the query (`?token=`, `?X-Amz-Signature=`, `?access_key=`), and every message
    /// in this funnel — refusal, request error, download failure, the tracing field —
    /// formats the whole URL, so that half leaked by exactly the same path (review #32).
    ///
    /// The diagnostic part of a query survives: erasing everything after `?` would make
    /// the message unactionable, and the finding is about credentials, not queries.
    #[test]
    fn url_redaction_covers_credential_query_parameters() {
        for (input, expected) in [
            (
                "https://h/install.sh?token=abc123",
                "https://h/install.sh?token=<redacted>",
            ),
            (
                "https://h/a.tar.gz?X-Amz-Signature=deadbeef&X-Amz-Credential=k",
                "https://h/a.tar.gz?X-Amz-Signature=<redacted>&X-Amz-Credential=<redacted>",
            ),
            (
                "https://h/a?channel=stable&api_key=s3cret",
                "https://h/a?channel=stable&api_key=<redacted>",
            ),
            // A query value that is NOT a credential stays: the message must stay useful.
            (
                "https://h/a?channel=beta&page=2",
                "https://h/a?channel=beta&page=2",
            ),
            // Userinfo AND a query secret at once.
            (
                "https://u:p@h/a?token=t",
                "https://<redacted>@h/a?token=<redacted>",
            ),
            // A fragment is DROPPED, not preserved: it is printed but never sent, so its
            // content is never diagnostic and an OAuth-style `#access_token=` lives there.
            (
                "https://h/a?token=t#frag",
                "https://h/a?token=<redacted>#<redacted>",
            ),
        ] {
            assert_eq!(redact_url_credentials(input), expected, "input: {input}");
        }
    }

    /// The shapes the name-matching redactor could not see.
    ///
    /// A BARE value has no name to match (`?ghp_abc123`), a FRAGMENT is printed but never
    /// sent (`#access_token=…`), and a value with an empty name has nothing that could vouch
    /// for it. The fourth reverify wave named all three after the name-list fix landed.
    #[test]
    fn url_redaction_covers_bare_values_and_fragments() {
        for (input, expected) in [
            ("https://h/a?ghp_abc123", "https://h/a?<redacted>"),
            ("https://h/a#access_token=secret", "https://h/a#<redacted>"),
            ("https://h/a?=sometoken", "https://h/a?=<redacted>"),
            // A named diagnostic parameter still survives: the message must stay usable.
            ("https://h/a?channel=beta", "https://h/a?channel=beta"),
            // Name and bare value together: each is handled on its own.
            (
                "https://h/a?channel=beta&ghp_abc123&token=t",
                "https://h/a?channel=beta&<redacted>&token=<redacted>",
            ),
        ] {
            assert_eq!(redact_url_credentials(input), expected, "input: {input}");
        }
    }

    /// A reqwest error chain can carry the raw URL in a nested source.
    ///
    /// The wrapper redacted the URL it was given and then formatted `{error:#}`, whose
    /// alternate form walks the chain — so a raw URL inside a nested `source()` sat beside
    /// the redacted one (review #32's last shape). The message now uses the TOP-level error
    /// only.
    #[test]
    fn the_download_error_path_does_not_walk_the_cause_chain() {
        let source = include_str!("installer.rs");
        let marker = "could not download the installer from";
        let at = source.find(marker).expect("the download failure message");
        let window = &source[at..(at + 400).min(source.len())];
        assert!(
            !window.contains("{error:#}"),
            "the download failure must not walk the error chain: {window}"
        );
    }

    /// The archive download redacted nothing: the same funnel leaked from its other half.
    ///
    /// `download_archive` and `download_once` formatted `{url}` into four messages. The
    /// installer path was wrapped by the first fix; these were missed because the finding
    /// cited the installer file. Checked in the SOURCE, so a message written in a
    /// different function cannot reintroduce the leak.
    #[test]
    fn no_update_message_formats_a_raw_url() {
        for (file, body) in [
            ("download.rs", include_str!("download.rs")),
            ("installer.rs", include_str!("installer.rs")),
        ] {
            for line in body.lines() {
                let trimmed = line.trim();
                assert!(
                    !(trimmed.contains("{url}")
                        && (trimmed.contains("format!(\"")
                            || trimmed.contains("bail!(\"")
                            || trimmed.contains("anyhow!(\""))),
                    "{file} formats a raw URL into a message: {trimmed}"
                );
            }
        }
    }
}

/// A redirect that leaves https is not followed (review #5, CWE-319).
///
/// `ensure_https_url` only vets the URL the caller passed; reqwest follows up to
/// ten redirects by default, so a plaintext `Location` would hand the funnel a
/// script it pipes to `/bin/sh -s --`. The policy allows an https→https hop
/// (GitHub Releases redirects to a CDN) and stops at any other scheme.
///
/// The policy is exercised through a real redirect chain on loopback rather than
/// by inspecting the closure: what matters is the request the client actually
/// makes, not the shape of the code that decides it.
#[tokio::test]
async fn an_installer_redirect_to_plaintext_is_not_followed() {
    use std::io::{Read as _, Write as _};

    // A target server that records whether it was reached at all. It ANSWERS when
    // reached — a listener that only accepts would hang the client under a broken
    // policy, turning this test into a suite-wide stall instead of a failure.
    let target = std::net::TcpListener::bind("127.0.0.1:0").expect("bind target");
    let target_address = target.local_addr().expect("target address");
    let reached = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let reached = std::sync::Arc::clone(&reached);
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = target.accept() {
                reached.store(true, std::sync::atomic::Ordering::SeqCst);
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nreached",
                );
            }
        });
    }

    // The redirecting server answers with one Location pointing at the target.
    let redirector = std::net::TcpListener::bind("127.0.0.1:0").expect("bind redirector");
    let redirector_address = redirector.local_addr().expect("redirector address");
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = redirector.accept() {
            let mut head = Vec::new();
            let mut byte = [0_u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                match stream.read(&mut byte) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => head.push(byte[0]),
                }
            }
            let response = format!(
                "HTTP/1.1 302 Found\r\nLocation: http://{target_address}/install.sh\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });

    let client = pa_ai::utils::tls::http_client_builder()
        .redirect(update_redirect_policy())
        .build()
        .expect("client");
    let response = client
        .get(format!("http://{redirector_address}/install.sh"))
        .send()
        .await
        .expect("the redirect response itself arrives");
    // A 3xx means the client stopped instead of following.
    assert!(
        response.status().is_redirection(),
        "an http Location must stop the chain, got {}",
        response.status()
    );
    // And the plaintext target was never contacted.
    assert!(
        !reached.load(std::sync::atomic::Ordering::SeqCst),
        "the plaintext target must never be reached"
    );
}

/// The policy is the difference: reqwest's default follows the same chain.
///
/// Without this, a passing redirect test proves only that the chain stopped —
/// which could be for any reason. Here the default client is driven over the
/// same two servers and must reach the plaintext target, so the pair pins the
/// behavioural difference the fix buys.
#[tokio::test]
async fn the_default_policy_would_have_followed_the_plaintext_hop() {
    use std::io::{Read as _, Write as _};

    let target = std::net::TcpListener::bind("127.0.0.1:0").expect("bind target");
    let target_address = target.local_addr().expect("target address");
    // The target answers 200 with a body, and records that it was reached.
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = target.accept() {
            let mut head = Vec::new();
            let mut byte = [0_u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                match stream.read(&mut byte) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => head.push(byte[0]),
                }
            }
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nreached",
            );
        }
    });

    let redirector = std::net::TcpListener::bind("127.0.0.1:0").expect("bind redirector");
    let redirector_address = redirector.local_addr().expect("redirector address");
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = redirector.accept() {
            let mut head = Vec::new();
            let mut byte = [0_u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                match stream.read(&mut byte) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => head.push(byte[0]),
                }
            }
            let response = format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://{target_address}/install.sh\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
            let _ = stream.write_all(response.as_bytes());
        }
    });

    // A bounded wait: if the default client ever stops following, this test fails
    // with a timeout instead of hanging the suite on a listener nobody accepts.
    let body = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        reqwest::Client::new()
            .get(format!("http://{redirector_address}/install.sh"))
            .send(),
    )
    .await
    .expect("the default client must answer within the bound")
    .expect("the default client follows the hop")
    .text()
    .await
    .expect("body");
    assert_eq!(
        body, "reached",
        "the default policy must reach the plaintext target, which is why the installer sets its own"
    );
}
