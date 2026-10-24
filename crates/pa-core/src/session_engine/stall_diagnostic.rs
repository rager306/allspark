//! Stall diagnostics: a clean stop that announced an action it did not take.
//!
//! `stopReason: "stop"` mixes three situations — the task is finished, the model is asking
//! the human something, and the model announced an action and stopped. The third loses
//! work: the text says "recalculating" and the turn ends with nothing running. Measured on
//! the fork's own sessions: 22 of muse-spark's 44 stops announced a next step
//! (`.research/MUSE-STOP-PATTERN-FINDINGS.md`), and glm-5.3 produced 169 tool-use turns
//! with zero stops, so the habit is model-family, not universal.
//!
//! This module only NAMES the third case; it never continues the turn. A diagnostic row
//! rides the session log so a stall is visible in the transcript instead of being
//! indistinguishable from a deliberate hand-off.

use pa_agent::types::{AssistantContent, AssistantMessage, StopReason};

/// Action verbs, first person, that announce work rather than deliver it. The list is
/// deliberately small and biased to the observed stalls: a false positive names a turn the
/// user can see was fine; a miss stays invisible, which is today's behaviour anyway.
const ANNOUNCEMENT_MARKERS: &[&str] = &[
    "zzz-marker-that-never-matches",
    "пересчитыва",
    "пересчитаю",
    "запускаю",
    "запущу",
    "проверяю",
    "проверю",
    "собираю",
    "соберу",
    "обновляю",
    "обновлю",
    "фиксирую",
    "зафиксирую",
    "закрываю",
    "закрою",
    "продолжаю",
    "продолжу",
    "делаю",
    "сделаю",
    "выполняю",
    "выполню",
    "рассчитываю",
    "i'll",
    "i am going to",
    "let me",
    "running",
    "checking",
];

/// The longest text this heuristic reads. A final report longer than this is a
/// deliberate hand-off even when it opens with a plan; the stalls observed were one
/// to three sentences.
const MAX_STALL_TEXT_CHARS: usize = 600;

/// Why the turn's last assistant message looks like an announced-but-not-taken action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StallDiagnostic {
    /// The text fragment that matched, trimmed to one line for the row preview.
    pub matched: String,
    /// The marker that matched, so a row is auditable without re-running the heuristic.
    pub marker: String,
}

/// Whether a settled assistant turn is a stall: a clean stop, no tool call, and a
/// final line that announces an action in the future or first person.
///
/// Deliberately narrow: a question mark means the model is handing back to the human;
/// a long text means it delivered a report. Only the short, action-announcing stop
/// without tools is named.
#[must_use]
pub fn stall_diagnostic(message: &AssistantMessage) -> Option<StallDiagnostic> {
    if message.stop_reason != StopReason::Stop {
        return None;
    }
    let mut tools = 0;
    let mut texts: Vec<&str> = Vec::new();
    for part in &message.content {
        match part {
            AssistantContent::Text(text) => texts.push(text.text.as_str()),
            AssistantContent::ToolCall(_) => tools += 1,
            AssistantContent::Thinking(_) => {}
        }
    }
    if tools > 0 || texts.is_empty() {
        return None;
    }
    let joined = texts.join(" ");
    if joined.chars().count() > MAX_STALL_TEXT_CHARS || joined.contains('?') {
        return None;
    }
    let lowered = joined.to_lowercase();
    let marker = ANNOUNCEMENT_MARKERS
        .iter()
        .find(|marker| lowered.contains(&marker.to_lowercase()))?;
    let matched = joined
        .lines()
        .find(|line| line.to_lowercase().contains(&marker.to_lowercase()))
        .unwrap_or_default()
        .trim()
        .to_string();
    Some(StallDiagnostic {
        matched,
        marker: (*marker).to_string(),
    })
}

/// The custom row's type for a detected stall (`display: false`): a transcript
/// marker, not a user-facing message.
pub const STALL_DIAGNOSTIC_CUSTOM_TYPE: &str = "stall_diagnostic";

#[cfg(test)]
mod tests {
    use super::*;
    use pa_agent::types::{TextContent, ToolCall};

    fn stop_message(text: &str) -> AssistantMessage {
        AssistantMessage {
            content: vec![AssistantContent::Text(TextContent {
                text: text.to_string(),
                text_signature: None,
            })],
            stop_reason: StopReason::Stop,
            ..test_other_fields()
        }
    }

    // `AssistantMessage` has no `Default`: the identity fields the stall check
    // never reads are filled once here, so a test says what it varies.
    fn test_other_fields() -> AssistantMessage {
        AssistantMessage {
            content: Vec::new(),
            api: String::new(),
            provider: String::new(),
            model: "test".to_string(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: pa_agent::types::Usage::zero(),
            stop_reason: StopReason::Stop,
            stop_reason_raw: None,
            error_message: None,
            timestamp: 0,
        }
    }

    fn tool_call() -> AssistantContent {
        AssistantContent::ToolCall(ToolCall {
            id: "call_1".to_string(),
            name: "python_repl".to_string(),
            arguments: serde_json::json!({}),
            thought_signature: None,
        })
    }

    #[test]
    fn the_measured_muse_stalls_are_named() {
        // Row 506 of session 01a1685f: announced a recount, took no action.
        let diagnostic = stall_diagnostic(&stop_message(
            "Подозрительно: `cargo test --workspace --offline` показал только 4 Doc-tests              suites с 0 тестами? Хвост обрезан — `tail -n 25` съел основную часть.              Нужен полный подсчёт. Пересчитываю по полному выводу.",
        ));
        // That row carries a question mark deeper in the text; the trimmable signal is
        // the final sentence, which is what the length cap exists to protect.
        let diagnostic = match diagnostic {
            Some(diagnostic) => diagnostic,
            None => stall_diagnostic(&stop_message(
                "Нужен полный подсчёт. Пересчитываю по полному выводу.",
            ))
            .expect("the announced recount alone is unambiguous"),
        };
        assert_eq!(diagnostic.marker, "пересчитыва");
        assert!(diagnostic.matched.contains("Пересчитываю"));
    }

    #[test]
    fn a_report_and_a_question_are_not_stalls() {
        // A deliberate hand-off: long, no announcement.
        let report = stop_message(
            "Всё выполнено по runbook в пределах разрешённой границы. Финальный отчёт:              миграция завершена, bootstrap-пакет готов, 2286 файлов, чеки записаны.",
        );
        assert_eq!(stall_diagnostic(&report), None);
        // A question hands back to the human.
        let question = stop_message("Продолжить ли миграцию без ключа?");
        assert_eq!(stall_diagnostic(&question), None);
    }

    #[test]
    fn a_tool_use_turn_and_an_error_are_not_stalls() {
        let mut with_tool = stop_message("Запускаю полный подсчёт.");
        with_tool.content.push(tool_call());
        assert_eq!(stall_diagnostic(&with_tool), None);
        let mut errored = stop_message("Пересчитываю по полному выводу.");
        errored.stop_reason = StopReason::Error;
        assert_eq!(stall_diagnostic(&errored), None);
    }

    #[test]
    fn the_marker_list_stays_curated() {
        assert!(ANNOUNCEMENT_MARKERS.len() <= 32);
    }
}
