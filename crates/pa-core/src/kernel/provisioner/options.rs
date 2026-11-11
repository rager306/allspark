//! The provisioner's construction options: what one session hands its kernel.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::kernel::bootstrap::KernelPythonSkill;
use crate::kernel::shared::HostRequestHandlers;

use super::startup::{KernelBootstrapResultHandler, RestoreCallback, UnavailableSkillsCallback};
use super::KernelReadyGate;

#[derive(Default, Clone)]
pub struct IpythonKernelProvisionerOptions {
    /// Python override. Must have prime-agent-runtime installed.
    pub python: Option<PathBuf>,
    /// Default time limit for user `python_repl` cells, in milliseconds
    /// (`None`: never armed; `0`: disabled). The daemon engine resolves it
    /// from settings and disarms it for daemonless sessions; a cell's own
    /// `timeout` argument replaces it. The runtime bootstrap and internal
    /// state ops never arm it.
    pub cell_timeout_ms: Option<u64>,
    pub env: HashMap<String, String>,
    /// Command prefix prepended to every kernel `bash()` invocation.
    pub command_prefix: Option<String>,
    /// Trusted shell path injected for kernel `bash()`; `None` on platforms
    /// without one, where the runtime's teaching error fires instead.
    pub shell_path: Option<PathBuf>,
    pub session_id: Option<String>,
    pub host_handlers: HostRequestHandlers,
    pub python_skills: Vec<KernelPythonSkill>,
    /// Artifact directory of a persistent session; the revivable snapshot and
    /// the stderr log live there. `None` for ephemeral sessions.
    pub snapshot_dir: Option<PathBuf>,
    /// Where a clipped stdout/stderr keeps its ORIGINAL stream.
    ///
    /// Same directory as `snapshot_dir` in practice; a separate field so the
    /// two can diverge without one silently disabling the other. `None` keeps
    /// the historical behaviour, where the clip is the only record.
    pub evidence_dir: Option<PathBuf>,
    /// Await (e.g. a previous provisioner's dispose) before reading the snapshot.
    ///
    /// [`KernelReadyGate`] is the named alias, so the composition root can carry the same
    /// fact through its own options without repeating the closure type.
    pub ready_gate: Option<KernelReadyGate>,
    /// Publishes the restore outcome once the kernel is usable.
    pub on_restore: Option<RestoreCallback>,
    /// Fires when the kernel's last live background `bash()` handle
    /// settles, so owed continuations can resume.
    pub on_background_work_settled: Option<crate::kernel::shared::BackgroundWorkSettledCallback>,
    /// Fires once per kernel start when installed Python skills failed to import into the kernel,
    /// so the session can tell the model before it wastes turns calling them.
    pub on_unavailable_skills: Option<UnavailableSkillsCallback>,
    /// Publishes the per-boot result for the `kernel_bootstrap_*` counters.
    /// Telemetry only; kernel behavior never depends on it.
    pub on_bootstrap_result: Option<KernelBootstrapResultHandler>,
}
