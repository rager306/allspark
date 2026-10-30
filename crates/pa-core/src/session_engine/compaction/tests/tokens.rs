//! What one message, one entry, and one whole context cost.
//!
//! The estimators are the numbers the trigger and the retention budget both
//! price against, so their agreements are the contract: chars/4 for text,
//! geometry for a user image, zero for a regenerable snapshot, and an anchor
//! on the last valid provider usage.

use super::super::*;
use super::fixtures::*;

#[test]
fn estimate_context_tokens_anchors_on_the_last_valid_usage() {
    let messages = vec![
        user_message("turn one", 1),
        assistant_message(1_000, 2),
        user_message("12345678", 3), // 8 chars -> 2 trailing tokens
    ];
    let estimate = estimate_context_tokens(&messages);
    assert_eq!(estimate.last_usage_index, Some(1));
    assert_eq!(estimate.tokens, 1_002);
    let messages = vec![user_message("12345678", 1)];
    let estimate = estimate_context_tokens(&messages);
    assert_eq!(estimate.last_usage_index, None);
    assert_eq!(estimate.tokens, 2);
}

#[test]
fn token_estimation() {
    let user = AgentMessage::User(pa_types::ai::UserMessage {
        content: pa_types::ai::UserContent::Text("12345678".to_string()), // 8 chars -> 2 tokens
        timestamp: 0,
        rest: serde_json::Map::default(),
    });
    assert_eq!(estimate_tokens(&user), 2);
    let usage = pa_types::ai::Usage {
        input: 10,
        output: 5,
        cache_read: 1,
        cache_write: 1,
        total_tokens: 100,
        cost: pa_types::ai::UsageCost::default(),
    };
    assert_eq!(calculate_context_tokens(&usage), 100);
}

#[test]
fn user_images_estimate_by_geometry_not_zero() {
    // TS counted user images as 0; the honest estimate adds ceil(w*h/750).
    let single = user_image_message(None, &[(100, 50)], 1);
    assert_eq!(estimate_tokens(&single), 7);

    // Three images: 7 + ceil(1_000*1_000/750)=1_334 + ceil(1920*1080/750)=2_765.
    let several = user_image_message(None, &[(100, 50), (1_000, 1_000), (1920, 1080)], 1);
    assert_eq!(estimate_tokens(&several), 7 + 1_334 + 2_765);

    // Text still counts chars/4 on top of the image tokens.
    let with_text = user_image_message(Some("12345678"), &[(100, 50)], 1);
    assert_eq!(estimate_tokens(&with_text), 2 + 7);
}

#[test]
fn user_image_without_geometry_falls_back_to_the_flat_default() {
    // A non-image payload (undecodable prefix) keeps the flat 1_200-token
    // default, matching the tool/custom path.
    let message = AgentMessage::User(pa_types::ai::UserMessage {
        content: pa_types::ai::UserContent::Blocks(vec![pa_types::ai::UserContentBlock::Image(
            pa_types::ai::ImageContent {
                data: "not-an-image".to_string(),
                mime_type: "image/png".to_string(),
                rest: serde_json::Map::default(),
            },
        )]),
        timestamp: 1,
        rest: serde_json::Map::default(),
    });
    assert_eq!(estimate_tokens(&message), pa_ai::DEFAULT_IMAGE_TOKENS);
}

#[test]
fn user_image_geometry_reads_the_elision_marker() {
    // The daemon's snapshot elision leaves widthPx/heightPx with the payload
    // stripped; the estimate still reads the geometry.
    let mut rest = serde_json::Map::default();
    rest.insert("widthPx".to_string(), serde_json::json!(640));
    rest.insert("heightPx".to_string(), serde_json::json!(480));
    let message = AgentMessage::User(pa_types::ai::UserMessage {
        content: pa_types::ai::UserContent::Blocks(vec![pa_types::ai::UserContentBlock::Image(
            pa_types::ai::ImageContent {
                data: String::new(),
                mime_type: "image/png".to_string(),
                rest,
            },
        )]),
        timestamp: 1,
        rest: serde_json::Map::default(),
    });
    // ceil(640 * 480 / 750) = 410.
    assert_eq!(estimate_tokens(&message), 410);
}

#[test]
fn user_images_shift_the_context_estimate_before_any_usage() {
    // No assistant usage yet (the trailing-window / pre-first-assistant
    // case): an image turn must not read as zero.
    let messages = vec![user_image_message(
        Some("describe this"),
        &[(1_000, 1_000)],
        1,
    )];
    let estimate = estimate_context_tokens(&messages);
    assert_eq!(estimate.last_usage_index, None);
    // "describe this" = 13 chars -> ceil(13/4) = 4; the 1_000x1_000 image adds 1_334.
    assert_eq!(estimate.tokens, 4 + 1_334);
}

#[test]
fn last_assistant_usage_skips_aborted() {
    let entries = vec![
        assistant_entry("a1", "root"),
        message_entry(
            "a2",
            "a1",
            AgentMessage::Assistant(pa_types::ai::AssistantMessage {
                content: vec![],
                api: "openai-completions".to_string(),
                provider: "p".to_string(),
                model: "m".to_string(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: pa_types::ai::Usage {
                    input: 1,
                    output: 1,
                    cache_read: 0,
                    cache_write: 0,
                    total_tokens: 2,
                    cost: pa_types::ai::UsageCost::default(),
                },
                stop_reason: pa_types::ai::StopReason::Aborted,
                stop_reason_raw: None,
                error_message: None,
                timestamp: 0,
                rest: serde_json::Map::default(),
            }),
        ),
    ];
    let usage = get_last_assistant_usage(&entries).unwrap();
    assert_eq!(usage.total_tokens, 150);
}

/// Review 5 (#3779): the cut walk must price every model-visible entry, not
/// only `Message`.
///
/// A `CustomMessage` notice is in the projection the provider receives, yet
/// the walk charged it zero. So a tail made of notices looked free: the cut
/// point stayed far left, keeping a tail whose real cost already exceeded
/// `keep_recent_tokens` — and the next trigger saw a prompt larger than the
/// retention calculation promised.
#[test]
fn a_custom_notice_costs_the_same_in_the_walk_as_a_message_of_equal_size() {
    let big = "x".repeat(4_000); // ~1_000 tokens
    let notice = |id: &str, parent: &str| FileEntry::CustomMessage {
        payload: pa_types::session::CustomMessageEntry {
            custom_type: "the_user_asked_for_this".to_string(),
            content: pa_types::ai::UserContent::Text(big.clone()),
            details: None,
            display: true,
            rest: serde_json::Map::default(),
        },
        base: pa_types::session::EntryBase {
            id: Some(id.to_string()),
            parent_id: Some(parent.to_string()),
            timestamp: Some("2024-01-01T00:00:00.000Z".to_string()),
            rest: serde_json::Map::default(),
        },
    };
    let with_notice = vec![
        user_entry("u1", "root", "turn one"),
        assistant_entry("a1", "u1"),
        notice("n1", "a1"),
    ];
    let with_message = vec![
        user_entry("u1", "root", "turn one"),
        assistant_entry("a1", "u1"),
        user_entry("u2", "a1", &big),
    ];

    assert_eq!(
        estimate_entry_tokens(&with_notice[2]),
        estimate_entry_tokens(&with_message[2]),
        "a notice of the same text must cost what a message of that text costs"
    );
    // Both walk to the same cut point under the same budget: the notice is
    // as expensive as the user message, so neither may be kept "for free".
    let keep = 900;
    assert_eq!(
        find_cut_point(&with_notice, 0, 3, keep).first_kept_entry_index,
        find_cut_point(&with_message, 0, 3, keep).first_kept_entry_index,
    );
    let only_notice = find_cut_point(&with_notice, 0, 3, keep).first_kept_entry_index;
    assert!(
        only_notice > 0,
        "the expensive tail must be cut, not kept at index 0"
    );
}

/// A regenerable snapshot is NOT sent, so it must not consume budget.
#[test]
fn a_stale_digest_entry_costs_nothing_in_the_walk() {
    let digest = FileEntry::CustomMessage {
        payload: pa_types::session::CustomMessageEntry {
            custom_type: "harness_digest".to_string(),
            content: pa_types::ai::UserContent::Text("x".repeat(40_000)),
            details: None,
            display: false,
            rest: serde_json::Map::default(),
        },
        base: pa_types::session::EntryBase {
            id: Some("d1".to_string()),
            parent_id: Some("root".to_string()),
            timestamp: Some("2024-01-01T00:00:00.000Z".to_string()),
            rest: serde_json::Map::default(),
        },
    };
    assert_eq!(estimate_entry_tokens(&digest), 0);
}
