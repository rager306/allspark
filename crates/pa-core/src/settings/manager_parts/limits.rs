//! The `[limits]` tool output windows (`bashOutputBytes`, `bashOutputLines`,
//! `kernelOutputChars`).

use crate::settings::manager::SettingsManager;

/// Bytes one `bash()` result keeps in its tail window.
pub const DEFAULT_BASH_OUTPUT_BYTES: usize = 50 * 1024;
/// Lines one `bash()` result keeps.
pub const DEFAULT_BASH_OUTPUT_LINES: usize = 2000;
/// Bytes the model's kernel `bash()` keeps: 512 KiB head + 1.5 MiB tail, the
/// `prime-agent-runtime` default (`_HEAD_CAP` + 3x).
pub const DEFAULT_KERNEL_BASH_OUTPUT_BYTES: usize = 512 * 1024;

impl SettingsManager {
    /// The byte window one `bash()` result keeps in the transcript
    /// (`limits.bashOutputBytes`, default 50 KiB). `0` or unset keeps the
    /// default: a zero-size window would show every run as truncated.
    #[must_use]
    pub fn get_bash_output_bytes(&self) -> usize {
        self.merged
            .limits
            .as_ref()
            .and_then(|limits| limits.bash_output_bytes)
            .filter(|bytes| *bytes > 0)
            .map_or(DEFAULT_BASH_OUTPUT_BYTES, |bytes| {
                usize::try_from(bytes).unwrap_or(usize::MAX)
            })
    }

    /// The line window one `bash()` result keeps (`limits.bashOutputLines`,
    /// default 2000). `0` or unset keeps the default.
    #[must_use]
    pub fn get_bash_output_lines(&self) -> usize {
        self.merged
            .limits
            .as_ref()
            .and_then(|limits| limits.bash_output_lines)
            .filter(|lines| *lines > 0)
            .map_or(DEFAULT_BASH_OUTPUT_LINES, |lines| {
                usize::try_from(lines).unwrap_or(usize::MAX)
            })
    }

    /// Characters one `python_repl` cell's stdout/stderr/result keeps
    /// (`limits.kernelOutputChars`, default 65536). `0` or unset keeps the
    /// default.
    #[must_use]
    pub fn get_kernel_output_chars(&self) -> usize {
        self.merged
            .limits
            .as_ref()
            .and_then(|limits| limits.kernel_output_chars)
            .filter(|chars| *chars > 0)
            .map_or(crate::kernel::shared::DEFAULT_MAX_OUTPUT_CHARS, |chars| {
                usize::try_from(chars).unwrap_or(usize::MAX)
            })
    }

    /// Bytes the MODEL's `bash()` keeps (the kernel head cap;
    /// `limits.kernelBashOutputBytes`, default 512 KiB). The tail is 3x this,
    /// the runtime's compiled ratio. `0` or unset keeps the default.
    #[must_use]
    pub fn get_kernel_bash_output_bytes(&self) -> usize {
        self.merged
            .limits
            .as_ref()
            .and_then(|limits| limits.kernel_bash_output_bytes)
            .filter(|bytes| *bytes > 0)
            .map_or(DEFAULT_KERNEL_BASH_OUTPUT_BYTES, |bytes| {
                usize::try_from(bytes).unwrap_or(usize::MAX)
            })
    }
}
