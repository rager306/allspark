//! The two namespace readers over a running kernel: what to prune, and which
//! user-defined names are live.

use crate::kernel::cancellation::AbortSignal;

use super::IpythonKernelProvisioner;

impl IpythonKernelProvisioner {
    /// Remove live variables above the snapshot's per-variable size limit.
    pub async fn prune_oversized_variables(&self) -> Option<Vec<String>> {
        let manager = self.manager()?;
        manager
            .prune_oversized_variables()
            .await
            .and_then(|r| r.pruned)
    }

    /// Live user-defined names in the kernel namespace, or `None` if listing
    /// failed or no kernel is running.
    pub async fn list_namespace_names(&self, signal: Option<AbortSignal>) -> Option<Vec<String>> {
        let manager = self.manager()?;
        manager.list_namespace_names(signal).await
    }
}
