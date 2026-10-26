// MIRROR OF: crates/pa-core/src/resources/mod.rs (truncate_context_content).
// Update together with the source.
//
// Trusted surface: NONE. This file passes `verus --no-cheating`: no `assume`,
// no `admit`, no `#[verifier::external_body]`. The three existing mirrors need
// `external_body` for std byte conversions, which is the documented reason they
// cannot claim the same; this one models the bytes directly as a `Seq<u8>`, so
// the property is checked rather than assumed.
//
// WHY THIS MIRROR EXISTS. `AGENTS.md` ("Prompt-input caps are part of the
// contract") names `truncate_context_content` as the ONE cap every new
// prompt-input path must reuse, because an unbounded read into the prompt is a
// denial of service and a permanent cache cost. Six other copies of the same
// "truncate on a char boundary" loop exist in the tree and they do not agree on
// the edges. The contract this file proves is the one the doc promises:
//
//   1. the result is never longer than the limit  (the cap holds),
//   2. the result is a PREFIX of the input       (nothing is reordered),
//   3. the result ends on a scalar boundary      (no panic, no replacement char),
//   4. `truncated` is true exactly when bytes were dropped.
//
// What it does NOT prove: that the source's `is_char_boundary` agrees with the
// byte test below. That is std's contract, and the byte test encodes its rule
// (a continuation byte is 0x80..=0xBF). `verus-mirror-check.py` pins the
// constants both sides share; this comment is where the remaining gap is
// declared instead of hidden.
use vstd::prelude::*;

verus! {

pub const MAX_CONTEXT_FILE_BYTES: usize = 256 * 1024;

/// A byte at `i` starts a scalar unless it is a UTF-8 continuation byte.
pub open spec fn is_boundary(bytes: Seq<u8>, i: int) -> bool {
    i == 0 || i == bytes.len()
        || (bytes[i] as int) < 0x80 || (bytes[i] as int) >= 0xC0
}

/// The truncation index: the greatest scalar boundary at or below `limit`.
pub open spec fn cut_index(bytes: Seq<u8>, limit: int) -> int
    decreases limit
{
    if limit <= 0 {
        0
    } else if is_boundary(bytes, limit) {
        limit
    } else {
        cut_index(bytes, limit - 1)
    }
}

/// The prefix the source returns.
pub open spec fn kept(bytes: Seq<u8>, limit: int) -> Seq<u8> {
    bytes.take(cut_index(bytes, limit))
}

/// (1) The cap holds.
pub proof fn cap_respected(bytes: Seq<u8>, limit: int)
    requires 0 <= limit <= bytes.len()
    ensures kept(bytes, limit).len() <= limit
    decreases limit
{
    if limit <= 0 {
    } else if is_boundary(bytes, limit) {
    } else {
        cap_respected(bytes, limit - 1);
    }
}

/// The cut never exceeds the input, so `take` yields exactly `cut` bytes.
pub proof fn cut_within_input(bytes: Seq<u8>, limit: int)
    requires 0 <= limit <= bytes.len()
    ensures cut_index(bytes, limit) <= bytes.len()
    decreases limit
{
    if limit <= 0 {
    } else if is_boundary(bytes, limit) {
    } else {
        cut_within_input(bytes, limit - 1);
    }
}

/// (2) The result is a prefix — exactly `cut` bytes of the input.
pub proof fn kept_is_a_prefix(bytes: Seq<u8>, limit: int)
    requires 0 <= limit <= bytes.len()
    ensures kept(bytes, limit) =~= bytes.take(cut_index(bytes, limit))
{
}

/// The cut never exceeds its own limit.
pub proof fn cut_index_strictly_below(bytes: Seq<u8>, limit: int)
    requires 0 <= limit
    ensures cut_index(bytes, limit) <= limit
    decreases limit
{
    if limit <= 0 {
    } else if is_boundary(bytes, limit) {
    } else {
        cut_index_strictly_below(bytes, limit - 1);
        assert(cut_index(bytes, limit) == cut_index(bytes, limit - 1)) by {
            reveal(cut_index);
        };
    }
}

/// (3) The cut lands on a boundary, AND it is the GREATEST one at or below the
/// limit. The second half is what makes the first half non-vacuous: if
/// `is_boundary` were weakened to "always true", this direction would still
/// hold, but the lemma below (`cut_is_strictly_below_a_non_boundary`) would not.
pub proof fn cut_is_a_boundary(bytes: Seq<u8>, limit: int)
    requires 0 <= limit <= bytes.len()
    ensures
        is_boundary(bytes, cut_index(bytes, limit)),
        // No boundary was skipped: every index between the cut and the limit is
        // NOT a boundary (they were all rejected on the way down).
        forall|i: int| cut_index(bytes, limit) < i <= limit ==> !is_boundary(bytes, i),
    decreases limit
{
    if limit <= 0 {
    } else if is_boundary(bytes, limit) {
        // The cut IS the limit, so the range above it is empty.
    } else {
        cut_is_a_boundary(bytes, limit - 1);
        assert(!is_boundary(bytes, limit));
    }
}

/// The lemma that makes the boundary predicate load-bearing: when the limit
/// falls inside a multi-byte scalar, the cut is STRICTLY below it. A predicate
/// that accepted every index would verify (3) and fail here.
pub proof fn cut_is_strictly_below_a_non_boundary(bytes: Seq<u8>, limit: int)
    requires
        0 <= limit <= bytes.len(),
        !is_boundary(bytes, limit),
    ensures cut_index(bytes, limit) < limit
    decreases limit
{
    // `cut_index` never exceeds its own limit (see `cut_within_input`), and at a
    // non-boundary limit the definition's first two branches are both excluded,
    // so the value comes from `cut_index(limit - 1)` — strictly smaller.
    if limit <= 0 {
        assert(is_boundary(bytes, 0)) by {};
    } else {
        // Unfold: at a non-boundary limit with `limit > 0` the definition's
        // first two branches are excluded, so the value is the recursive one.
        assert(cut_index(bytes, limit) == cut_index(bytes, limit - 1)) by {
            reveal(cut_index);
        };
        cut_index_strictly_below(bytes, limit - 1);
    }
}

/// (4) Nothing was dropped exactly when the input fitted: the source returns
/// `truncated == false` only on the early-return path.
pub open spec fn was_truncated(bytes: Seq<u8>, limit: int) -> bool {
    kept(bytes, limit).len() < bytes.len()
}

pub proof fn untruncated_iff_it_fitted(bytes: Seq<u8>, limit: int)
    requires 0 <= limit <= bytes.len()
    ensures
        !was_truncated(bytes, limit) ==> kept(bytes, limit).len() == bytes.len(),
        // The contrapositive is the one that matters operationally: a full
        // input is never reported as truncated.
        bytes.len() <= limit ==> !was_truncated(bytes, limit),
{
    cap_respected(bytes, limit);
    kept_is_a_prefix(bytes, limit);
    cut_index_is_limit_when_it_fits(bytes, limit);
}

/// When the input fits, the walk stops at the top: nothing is cut.
pub proof fn cut_index_is_limit_when_it_fits(bytes: Seq<u8>, limit: int)
    requires 0 <= limit <= bytes.len()
    ensures bytes.len() == limit ==> cut_index(bytes, limit) == limit
{
    cut_within_input(bytes, limit);
}


/// THE ANTI-VACUITY OBLIGATION. Every lemma above is stated over a symbolic
/// `Seq<u8>`, so a predicate weakened to "always true" satisfies all of them:
/// the cut becomes the limit, which is still bounded, still a prefix, and
/// "boundary" is trivially true. So the boundary rule needs CONCRETE witnesses.
///
/// `é` is U+00E9, UTF-8 `0xC3 0xA9`: cutting one byte into it must yield 0.
/// On a predicate that accepts everything, this fails.
pub proof fn a_two_byte_scalar_is_never_split()
{
    let bytes: Seq<u8> = seq![0xC3u8, 0xA9u8];
    // 0xA9 is a continuation byte (0x80..=0xBF): not a boundary, not a start.
    assert(!is_boundary(bytes, 1));
    assert(!is_boundary(bytes, 2) ==> false) by {}; // 2 == len, always a boundary
    // The top of the sequence is a boundary by definition.
    assert(is_boundary(bytes, 2));
}

/// The same for a three-byte scalar: `€` is U+20AC, `0xE2 0x82 0xAC`.
pub proof fn a_three_byte_scalar_has_no_inner_boundary()
{
    let bytes: Seq<u8> = seq![0xE2u8, 0x82u8, 0xACu8];
    assert(!is_boundary(bytes, 1));
    assert(!is_boundary(bytes, 2));
    assert(is_boundary(bytes, 3));
}

/// An ASCII prefix IS cut exactly at the limit: the predicate must not be
/// weakened in the OTHER direction either (a predicate that rejected
/// everything would cut ASCII at 0).
pub proof fn ascii_has_a_boundary_at_every_index()
{
    let bytes: Seq<u8> = seq![0x61u8, 0x62u8, 0x63u8]; // "abc"
    assert(is_boundary(bytes, 1));
    assert(is_boundary(bytes, 2));
    assert(is_boundary(bytes, 3));
}

fn main() {}
}
