//! The review-receipt contract (`docs/review-receipt.md`) is only worth
//! something if a receipt can be checked against the tree. `docs/review-receipt.md`
//! says a claim without a quote is unverified, and a quote that cannot be found
//! is worse than none: it looks checked.
//!
//! This test parses every YAML receipt in that document and verifies the one
//! mechanical claim a machine can verify — that `quote` appears in `file` at
//! `line`. It does not judge the finding; it proves the receipt points at real
//! code, which is the part a reviewer gets wrong silently.

use std::path::{Path, PathBuf};

/// The workspace root: `pa-core` lives at `<root>/crates/pa-core`.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf()
}

fn receipt_document() -> String {
    let path = workspace_root().join("docs/review-receipt.md");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

/// One receipt's `file`, `line`, and `quote`, as the document writes them.
struct Receipt {
    file: String,
    line: usize,
    quote: String,
}

/// Strip the `|` block scalar's indentation, which is the document's, not the
/// quoted code's.
fn block_scalar(lines: &[&str]) -> String {
    let indents: Vec<usize> = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .collect();
    let cut = indents.iter().copied().min().unwrap_or(0);
    lines
        .iter()
        .map(|line| line.get(cut..).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
}

fn receipts() -> Vec<Receipt> {
    let document = receipt_document();
    let mut found = Vec::new();
    let lines: Vec<&str> = document.lines().collect();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("file: ") {
            let file = rest.trim().to_string();
            // `line:` follows `file:` in every receipt the contract describes.
            let line_number = lines
                .get(index + 1)
                .and_then(|next| next.trim_start().strip_prefix("line: "))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or_else(|| panic!("a receipt after {file} has no `line:`"));
            let quote_start = lines
                .get(index + 2)
                .and_then(|next| next.trim_start().strip_prefix("quote: "))
                .unwrap_or_else(|| panic!("a receipt after {file} has no `quote:`"));
            assert_eq!(
                quote_start.trim(),
                "|",
                "the quote in {file} must be a `|` block scalar"
            );
            let mut body = Vec::new();
            let mut cursor = index + 3;
            while cursor < lines.len() && lines[cursor].starts_with(' ') {
                body.push(lines[cursor]);
                cursor += 1;
            }
            found.push(Receipt {
                file,
                line: line_number,
                quote: block_scalar(&body),
            });
            index = cursor;
            continue;
        }
        index += 1;
    }
    found
}

#[test]
fn a_receipt_document_carries_at_least_one_confirmed_and_one_rejected_receipt() {
    let document = receipt_document();
    assert!(
        document.contains("verdict: confirmed"),
        "the contract is better taught by a confirmed example"
    );
    assert!(
        document.contains("verdict: rejected"),
        "a round of only confirmations was not adversarial enough to trust"
    );
    assert!(!receipts().is_empty(), "the document shows no receipt");
}

#[test]
fn every_receipt_quote_appears_in_its_file_at_its_line() {
    let root = workspace_root();
    let mut checked = 0;
    for receipt in receipts() {
        let path = root.join(&receipt.file);
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let source_lines: Vec<&str> = source.lines().collect();
        let quoted: Vec<&str> = receipt.quote.lines().collect();
        let first = receipt
            .line
            .checked_sub(1)
            .unwrap_or_else(|| panic!("{} cites line 0", receipt.file));
        assert!(
            first < source_lines.len(),
            "{}: line {} is past the file's {} lines",
            receipt.file,
            receipt.line,
            source_lines.len()
        );
        // The quote's first line must be AT the cited line; the rest follows.
        // A receipt that points at the right text from the wrong line is the
        // drift this check exists to catch.
        assert_eq!(
            source_lines[first].trim(),
            quoted[0].trim(),
            "{}: line {} is not the quoted line",
            receipt.file,
            receipt.line
        );
        for (offset, expected) in quoted.iter().enumerate().skip(1) {
            let actual = source_lines.get(first + offset).map(|line| line.trim());
            assert_eq!(
                actual,
                Some(expected.trim()),
                "{}: quote drifts at line {}",
                receipt.file,
                first + offset + 1
            );
        }
        checked += 1;
    }
    assert!(
        checked >= 2,
        "expected both example receipts, checked {checked}"
    );
}
