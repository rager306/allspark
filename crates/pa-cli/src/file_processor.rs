//! `@file` argument expansion for the initial prompt: each argument becomes
//! either a `<file name>` text block or a base64 image attachment (the
//! magic-byte sniff, never the extension, plus the same 4.5MB inline limit).
//! No resize engine: an oversized image under auto-resize falls back to the
//! converter-unavailable text (`[Image omitted: ...]`) TS emits.
//! Text files over `MAX_TEXT_EMBED_BYTES` embed only their first cap bytes
//! plus an explicit truncation marker (a deliberate divergence from the TS
//! binary, which embeds whole files unbounded).

use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use pa_agent::types::ImageContent;

/// 4.5MB of base64 payload, headroom below the provider's 5MB limit.
const DEFAULT_MAX_BYTES: usize = 4_500_000;

/// Cap on the text bytes one @file argument may embed into the initial
/// prompt; files above it embed a prefix plus a truncation marker. 256KiB is
/// ~5x the 50KiB tool-output byte cap (`pa-core` truncate.rs) and a small
/// fraction of the 4.5MB image inline limit, so source files and logs of
/// interest pass whole while a stray huge file cannot dominate the prompt.
const MAX_TEXT_EMBED_BYTES: u64 = 256 * 1024;

/// The expanded first prompt: @file text blocks joined into one string,
/// plus the image attachments.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ProcessedFiles {
    pub text: String,
    pub images: Vec<ImageContent>,
}

/// A file-argument failure: the exact stderr line printed before exit 1.
#[derive(Debug, Clone, PartialEq)]
pub struct FileProcessingError {
    pub message: String,
}

/// Expand the `@file` arguments: every argument resolves against the
/// cwd (with the TS macOS filename variants), a missing file fails the
/// run, an empty file is skipped, an image attaches as base64, and
/// anything else embeds as a `<file name>` text block.
pub fn process_file_arguments(
    file_args: &[String],
    cwd: &Path,
    auto_resize_images: bool,
) -> Result<ProcessedFiles, FileProcessingError> {
    let mut processed = ProcessedFiles::default();
    for file_arg in file_args {
        let resolved = pa_core::resolve_read_path(file_arg, &cwd.to_string_lossy());
        let absolute_path = PathBuf::from(&resolved);
        let metadata = std::fs::metadata(&absolute_path).map_err(|_| FileProcessingError {
            message: format!("Error: File not found: {resolved}"),
        })?;
        if metadata.len() == 0 {
            continue;
        }
        match pa_tui::image_load::load_image_from_path(&absolute_path) {
            // An image: attach the payload (the same sniff the paste path
            // uses), with the auto-resize setting gating the size limit.
            Ok(Some(image)) => {
                if auto_resize_images && image.data.len() > DEFAULT_MAX_BYTES {
                    let _ = writeln!(
                    processed.text,
                    "<file name=\"{resolved}\">[Image omitted: could not be resized below the inline image size limit.]</file>"
                );
                    continue;
                }
                processed.images.push(ImageContent {
                    data: image.data,
                    mime_type: image.mime_type,
                });
                let _ = writeln!(processed.text, "<file name=\"{resolved}\"></file>");
            }
            // Not an image: embed the content in a file block (Node's utf-8 read decodes
            // invalid sequences lossily, so binary non-image files embed instead of failing).
            // Files over the cap embed only their first MAX_TEXT_EMBED_BYTES bytes plus a
            // truncation marker, and the read itself stops at the cap, so one @file
            // argument cannot pull an unbounded payload into the initial prompt.
            Ok(None) => {
                let total_bytes = metadata.len();
                let mut file =
                    std::fs::File::open(&absolute_path).map_err(|error| FileProcessingError {
                        message: format!("Error: Could not read file {resolved}: {error}"),
                    })?;
                let mut bytes = Vec::new();
                (&mut file)
                    .take(total_bytes.min(MAX_TEXT_EMBED_BYTES))
                    .read_to_end(&mut bytes)
                    .map_err(|error| FileProcessingError {
                        message: format!("Error: Could not read file {resolved}: {error}"),
                    })?;
                let mut embedded_bytes = bytes.len();
                if total_bytes > MAX_TEXT_EMBED_BYTES {
                    // The byte cap may land inside a UTF-8 character: cut back to the last
                    // complete one so the marker never follows a split character. Genuinely
                    // invalid bytes stay and decode lossily like the untruncated path.
                    if let Err(error) = std::str::from_utf8(&bytes) {
                        if error.error_len().is_none() {
                            embedded_bytes = error.valid_up_to();
                        }
                    }
                }
                bytes.truncate(embedded_bytes);
                let mut content = String::from_utf8_lossy(&bytes).into_owned();
                if total_bytes > MAX_TEXT_EMBED_BYTES {
                    let _ = write!(
                        content,
                        "\n[file truncated at {embedded_bytes} of {total_bytes} bytes]"
                    );
                }
                let _ = writeln!(
                    processed.text,
                    "<file name=\"{resolved}\">\n{content}\n</file>"
                );
            }
            Err(error) => {
                return Err(FileProcessingError {
                    message: format!("Error: Could not read file {resolved}: {error}"),
                });
            }
        }
    }
    Ok(processed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().to_path_buf();
        (dir, path)
    }

    #[test]
    fn text_files_embed_in_file_blocks() {
        let (_guard, dir) = temp_dir();
        let file = dir.join("notes.md");
        std::fs::write(&file, "the content").expect("write fixture");
        let processed = process_file_arguments(&[file.to_string_lossy().to_string()], &dir, true)
            .expect("process");
        assert_eq!(processed.images, Vec::new());
        assert_eq!(
            processed.text,
            format!(
                "<file name=\"{}\">\nthe content\n</file>\n",
                file.to_string_lossy()
            )
        );
    }

    #[test]
    fn relative_paths_resolve_against_the_cwd() {
        let (_guard, dir) = temp_dir();
        std::fs::write(dir.join("a.txt"), "relative").expect("write fixture");
        let processed =
            process_file_arguments(&["a.txt".to_string()], &dir, true).expect("process");
        assert!(processed.text.contains("relative"));
        assert!(processed.text.contains(&format!(
            "<file name=\"{}\">",
            dir.join("a.txt").to_string_lossy()
        )));
    }

    #[test]
    fn missing_file_fails_with_the_ts_error() {
        let (_guard, dir) = temp_dir();
        let missing = dir.join("nope.txt").to_string_lossy().to_string();
        let error = process_file_arguments(std::slice::from_ref(&missing), &dir, true).unwrap_err();
        assert_eq!(error.message, format!("Error: File not found: {missing}"));
    }

    #[test]
    fn empty_files_are_skipped() {
        let (_guard, dir) = temp_dir();
        let file = dir.join("empty.txt");
        std::fs::write(&file, "").expect("write fixture");
        let processed = process_file_arguments(&[file.to_string_lossy().to_string()], &dir, true)
            .expect("process");
        assert_eq!(processed, ProcessedFiles::default());
    }

    #[test]
    fn images_attach_as_base64_content() {
        let (_guard, dir) = temp_dir();
        // A minimal 1x1 PNG (the magic prefix the sniffer matches).
        let png: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D,
        ];
        let file = dir.join("pixel.png");
        std::fs::write(&file, png).expect("write fixture");
        let processed = process_file_arguments(&[file.to_string_lossy().to_string()], &dir, true)
            .expect("process");
        assert_eq!(processed.images.len(), 1);
        assert_eq!(processed.images[0].mime_type, "image/png");
        assert!(processed.text.contains(&format!(
            "<file name=\"{}\"></file>",
            file.to_string_lossy()
        )));
    }

    #[test]
    fn oversized_images_under_auto_resize_get_the_omitted_note() {
        let (_guard, dir) = temp_dir();
        // A PNG-magic payload whose base64 exceeds the 4.5MB limit.
        let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend(std::iter::repeat_n(0u8, 4_000_000));
        let file = dir.join("huge.png");
        std::fs::write(&file, &png).expect("write fixture");
        let processed = process_file_arguments(&[file.to_string_lossy().to_string()], &dir, true)
            .expect("process");
        assert_eq!(processed.images, Vec::new());
        assert!(processed
            .text
            .contains("[Image omitted: could not be resized below the inline image size limit.]"));
    }

    #[test]
    fn non_utf8_files_embed_lossily() {
        let (_guard, dir) = temp_dir();
        let file = dir.join("blob.bin");
        std::fs::write(&file, [0xff, 0xfe, b'x']).expect("write fixture");
        let processed = process_file_arguments(&[file.to_string_lossy().to_string()], &dir, true)
            .expect("process");
        assert!(processed.text.contains('x'));
        assert!(processed.text.contains('\u{fffd}'));
        assert_eq!(processed.images, Vec::new());
    }

    #[test]
    fn oversized_text_files_embed_the_prefix_and_a_truncation_marker() {
        let (_guard, dir) = temp_dir();
        let file = dir.join("huge.log");
        let total = MAX_TEXT_EMBED_BYTES as usize + 1_000;
        let digits: Vec<u8> = (0..total).map(|i| b'0' + (i % 10) as u8).collect();
        std::fs::write(&file, &digits).expect("write fixture");
        let processed = process_file_arguments(&[file.to_string_lossy().to_string()], &dir, true)
            .expect("process");
        let prefix: String = (0..MAX_TEXT_EMBED_BYTES as usize)
            .map(|i| char::from(b'0' + (i % 10) as u8))
            .collect();
        assert_eq!(
            processed.text,
            format!(
                "<file name=\"{}\">\n{prefix}\n[file truncated at {} of {total} bytes]\n</file>\n",
                file.to_string_lossy(),
                MAX_TEXT_EMBED_BYTES
            )
        );
    }

    #[test]
    fn a_file_exactly_at_the_cap_embeds_in_full() {
        let (_guard, dir) = temp_dir();
        let file = dir.join("edge.log");
        let content = "x".repeat(MAX_TEXT_EMBED_BYTES as usize);
        std::fs::write(&file, &content).expect("write fixture");
        let processed = process_file_arguments(&[file.to_string_lossy().to_string()], &dir, true)
            .expect("process");
        assert_eq!(
            processed.text,
            format!(
                "<file name=\"{}\">\n{content}\n</file>\n",
                file.to_string_lossy()
            )
        );
    }

    #[test]
    fn a_single_giant_line_is_truncated_like_any_other_file() {
        let (_guard, dir) = temp_dir();
        let file = dir.join("one-line.log");
        let total = MAX_TEXT_EMBED_BYTES as usize + 5_000;
        std::fs::write(&file, "a".repeat(total)).expect("write fixture");
        let processed = process_file_arguments(&[file.to_string_lossy().to_string()], &dir, true)
            .expect("process");
        let prefix = "a".repeat(MAX_TEXT_EMBED_BYTES as usize);
        assert_eq!(
            processed.text,
            format!(
                "<file name=\"{}\">\n{prefix}\n[file truncated at {} of {total} bytes]\n</file>\n",
                file.to_string_lossy(),
                MAX_TEXT_EMBED_BYTES
            )
        );
    }

    #[test]
    fn the_cap_never_splits_a_multibyte_character() {
        let (_guard, dir) = temp_dir();
        let file = dir.join("accent.log");
        // The two-byte 'é' straddles the cap boundary: its first byte is the last
        // byte under the cap.
        let mut bytes = "a".repeat(MAX_TEXT_EMBED_BYTES as usize - 1).into_bytes();
        let total = bytes.len() + "é".len() + b"tail".len();
        bytes.extend("é".as_bytes());
        bytes.extend(b"tail");
        std::fs::write(&file, &bytes).expect("write fixture");
        let processed = process_file_arguments(&[file.to_string_lossy().to_string()], &dir, true)
            .expect("process");
        let prefix = "a".repeat(MAX_TEXT_EMBED_BYTES as usize - 1);
        assert!(!processed.text.contains('\u{fffd}'));
        assert_eq!(
            processed.text,
            format!(
                "<file name=\"{}\">\n{prefix}\n[file truncated at {} of {total} bytes]\n</file>\n",
                file.to_string_lossy(),
                MAX_TEXT_EMBED_BYTES - 1
            )
        );
    }

    #[test]
    fn auto_resize_off_attaches_the_raw_payload() {
        let (_guard, dir) = temp_dir();
        let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend(std::iter::repeat_n(0u8, 4_000_000));
        let file = dir.join("huge.png");
        std::fs::write(&file, &png).expect("write fixture");
        let processed = process_file_arguments(&[file.to_string_lossy().to_string()], &dir, false)
            .expect("process");
        assert_eq!(processed.images.len(), 1);
    }
}
