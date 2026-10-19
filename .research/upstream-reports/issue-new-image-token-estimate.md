# New issue: Compaction token estimate charges user-attached images as **0** tokens (and every image as a flat 4 800 chars regardless of size)

**Status:** draft / not submitted. **Type:** bug (mistimed compaction / context overflow). **Severity:** P2. **Area:** `crates/pa-core/src/session_engine/compaction.rs`.

## Summary

`estimate_tokens` for a `User` message with content blocks sums only `Text` blocks — an image block contributes **zero**. For `Custom` and `ToolResult` messages every image is a flat `4_800` chars (~1 200 tokens) no matter how many megabytes its base64 payload is. Both behaviours are copied verbatim from TS v0.9.8, right down to the `// ~1200 tokens` comment. Consequence: an image-heavy turn under-counts and compaction can be mistimed (the last-drop case at the trailing window boundary).

## Reproduction

1. Attach a large image to a user turn (or have a kernel cell return an image attachment).
2. The compaction estimator charges `0` for the user image (and a flat 1 200 tokens for a tool/custom image, even a multi-MB one).
3. In the trailing window (current turn after the last assistant `usage`, or history before the first assistant `usage`) the estimate is short by the whole image cost → compaction may not fire in time; the overflow path then triggers a compact-and-retry.

## Evidence (both eras)

| Era | File:line | Code |
|---|---|---|
| TS `v0.9.8` | `packages/coding-agent/src/core/compaction/compaction.ts:234-245` | `case "user":` loop adds only `block.type === "text"` → images count **0** |
| TS `v0.9.8` | `…/compaction.ts:263-274` | `case "custom"/"toolResult":` `if (block.type === "image") chars += 4800; // Estimate images as 4000 chars, or 1200 tokens` |
| Rust `v0.10.x` / `main` | `crates/pa-core/src/session_engine/compaction.rs:80-89` | `UserContent::Blocks(blocks) => div4(blocks.iter().filter_map(Text → Some(chars)).sum())` — images dropped |
| Rust | `compaction.rs:114`, `:128` | `UserContentBlock::Image(_) => 4_800, // ~1200 tokens` (Custom and ToolResult arms), flat regardless of base64 size |
| Rust | `compaction.rs:117` | `Raw(_) => 0` |
| Rust | `crates/pa-core/src/tools/render_utils.rs:52-54` | confirms tool results carry multi-MB base64 images |
| Rust | `compaction.rs:216-234` | `estimate_context_tokens` anchors on the last valid assistant `usage`; chars/4 applies only to the tail — so the under-count is confined to the trailing window (and pre-first-usage history) |

Note: `estimate_context_tokens` anchoring on real `usage` closes the *steady-state* case, so the original "compaction never fires for image-heavy sessions" framing is too strong — the real defect is the **trailing-window** under-count (the last-drop case that tips compaction over).

## Proposed fix

Charge images by geometry when the pixel dimensions are known: `ceil(w*h/750)` (the common vision rule, ~1 token / 750 px), falling back to the flat 1 200 tokens when the geometry cannot be read. Reuse `pa-ai`'s new `image_tokens_for_dimensions` / `DEFAULT_IMAGE_TOKENS`; read `widthPx`/`heightPx` from the block's `rest` (the daemon's snapshot elision leaves them) or decode a bounded prefix (`IMAGE_DIMENSIONS_PREFIX_BYTES = 1024`).

Key fragment (`compaction.rs`):

```rust
pa_types::ai::UserContent::Blocks(blocks) => {
    let text_chars: u64 = blocks.iter().filter_map(|b| match b {
        pa_types::ai::UserContentBlock::Text(t) => Some(chars(&t.text)), _ => None }).sum();
    let image_tokens: u64 = blocks.iter().map(user_image_tokens).sum();   // was: 0 for user images
    div4(text_chars) + image_tokens
}

fn user_image_tokens(block: &UserContentBlock) -> u64 {
    let Image(image) = block else { return 0 };
    let dims = image.rest.get("widthPx").and_then(Value::as_u64)
        .zip(image.rest.get("heightPx").and_then(Value::as_u64))
        .map(|(w, h)| (w as u32, h as u32))
        .or_else(|| get_image_dimensions_prefix(&image.data, &image.mime_type, IMAGE_DIMENSIONS_PREFIX_BYTES)
            .map(|d| (d.width_px, d.height_px)));
    match dims { Some((w, h)) => pa_ai::image_tokens_for_dimensions(w, h), None => pa_ai::DEFAULT_IMAGE_TOKENS }
}
```

The flat `4_800` arms for Custom/ToolResult stay (TS parity) but are annotated as such. Tests: user image by geometry (not zero), geometry fallback to flat, elision marker read, and the pre-usage shift. This is a deliberate divergence — earlier compaction firing is user-visible — so it belongs upstream (or as a documented parity exception).

## Impact

- Image-heavy sessions mis-time compaction: the trailing window under-counts by the full image cost, so the last-drop turn can push past the context limit and force an overflow compact-and-retry.
- Multi-MB tool images are billed as 1 200 tokens flat in the estimator, under-representing their true cost.
- Affects both eras identically (exact port of the TS estimate).

## References

- `.research/verify-unbounded.md` #13; `.research/verify-ts-era.md` §C (#13).
- TS reference: `git show v0.9.8:packages/coding-agent/src/core/compaction/compaction.ts` (lines 234-274).
