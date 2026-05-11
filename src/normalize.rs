use unicode_general_category::{get_general_category, GeneralCategory};
use unicode_normalization::UnicodeNormalization;
use wide::u8x16;

pub fn contains_cjk(text: &str) -> bool {
    text.chars().any(|ch| {
        ('\u{3400}'..='\u{4dbf}').contains(&ch)
            || ('\u{4e00}'..='\u{9fff}').contains(&ch)
            || ('\u{f900}'..='\u{faff}').contains(&ch)
    })
}

pub fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn normalize_zh(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    normalize_zh_into(text, &mut out);
    out
}

/// Allocation-conscious variant of `normalize_zh`. Reuses `out`'s capacity
/// across calls. Phase C hot path uses this from `text_analyzer::analyze`.
///
/// Includes an ASCII fast-path: 16-byte input blocks that are pure ASCII
/// (high bit clear, no whitespace, no punctuation, no symbol) get accepted
/// wholesale via one `wide::u8x16` mask check. Mixed and non-ASCII blocks
/// fall back to the scalar NFKC + general-category filter.
pub fn normalize_zh_into(text: &str, out: &mut String) {
    out.clear();
    out.reserve(text.len());

    let bytes = text.as_bytes();
    let mut i = 0usize;
    let mut scalar_start = 0usize;
    while i + 16 <= bytes.len() {
        if all_ascii_keep_block_16(&bytes[i..i + 16]) {
            // Safe: every byte is < 0x80 (verified by the mask) so the slice
            // is valid UTF-8 trivially.
            // SAFETY: ASCII subset confirmed above.
            unsafe {
                out.push_str(std::str::from_utf8_unchecked(&bytes[i..i + 16]));
            }
            i += 16;
            scalar_start = i;
        } else {
            // Mixed or non-ASCII — drop into scalar fallback for the rest
            // of the string (single contiguous tail is faster than chunked
            // round-trips between SIMD and scalar).
            break;
        }
    }
    let _ = scalar_start;

    // Scalar fallback for the remainder. Use NFKC + GeneralCategory exactly
    // as the original implementation did.
    if i < bytes.len() {
        // SAFETY: `i` only advances on confirmed ASCII codepoint boundaries
        // (every retained byte was < 0x80, which is a 1-byte UTF-8 sequence).
        let tail = unsafe { std::str::from_utf8_unchecked(&bytes[i..]) };
        for ch in tail.nfkc() {
            if ch.is_whitespace() {
                continue;
            }
            if is_strippable_category(ch) {
                continue;
            }
            out.push(ch);
        }
    }
}

/// `true` iff every byte in `block` is ASCII non-whitespace, non-punctuation,
/// non-symbol (the "keep" set under the existing `normalize_zh` policy).
///
/// Implements the policy as a set of mask checks on a u8x16 SIMD register:
/// - reject any byte >= 0x80 (would be a UTF-8 continuation / leading byte).
/// - reject 0x00..=0x20 (control chars + space).
/// - reject 0x7F (DEL).
/// - reject ASCII punctuation ranges (`!`-`/`, `:`-`@`, `[`-``` ` ```, `{`-`~`).
///
/// Calling this on a block that doesn't fully satisfy the predicate returns
/// false; the caller then falls back to the scalar path.
#[inline]
fn all_ascii_keep_block_16(block: &[u8]) -> bool {
    debug_assert_eq!(block.len(), 16);
    let arr: [u8; 16] = block.try_into().expect("len 16 enforced");
    let v = u8x16::new(arr);

    // wide-0.7's u8x16 has cmp_eq, max, min, move_mask — but no unsigned
    // cmp_lt. We synthesize range checks via:
    //   (v >= low)  ⟺  v.max(low) == v
    //   (v <= high) ⟺  v.min(high) == v
    // Each comparison returns 0xFF in passing lanes, 0x00 in failing lanes.
    // The keep-set is ASCII alnum: 0x30..=0x39 | 0x41..=0x5A | 0x61..=0x7A.

    let in_range = |lo: u8, hi: u8| -> u8x16 {
        let ge = v.max(u8x16::splat(lo)).cmp_eq(v);
        let le = v.min(u8x16::splat(hi)).cmp_eq(v);
        ge & le
    };
    let allowed = in_range(0x30, 0x39) | in_range(0x41, 0x5A) | in_range(0x61, 0x7A);
    // move_mask gathers the high bit of each lane; passing lanes are 0xFF
    // (high bit set), failing are 0x00. All-pass ⇔ low 16 bits all set.
    allowed.move_mask() == 0xFFFF
}

#[inline]
fn is_strippable_category(ch: char) -> bool {
    matches!(
        get_general_category(ch),
        GeneralCategory::ConnectorPunctuation
            | GeneralCategory::DashPunctuation
            | GeneralCategory::OpenPunctuation
            | GeneralCategory::ClosePunctuation
            | GeneralCategory::InitialPunctuation
            | GeneralCategory::FinalPunctuation
            | GeneralCategory::OtherPunctuation
            | GeneralCategory::SpaceSeparator
            | GeneralCategory::LineSeparator
            | GeneralCategory::ParagraphSeparator
            | GeneralCategory::MathSymbol
            | GeneralCategory::CurrencySymbol
            | GeneralCategory::ModifierSymbol
            | GeneralCategory::OtherSymbol
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_punct_and_ws() {
        assert_eq!(normalize_zh("a b!c"), "abc");
    }

    #[test]
    fn cjk_preserved() {
        assert_eq!(normalize_zh("如是我聞"), "如是我聞");
    }

    #[test]
    fn cjk_punct_stripped() {
        assert_eq!(normalize_zh("如是， 我聞。"), "如是我聞");
    }

    #[test]
    fn ascii_fast_path_matches_scalar() {
        // 32 ASCII chars; first 16 are pure alnum, then a punct triggers scalar tail.
        let a = normalize_zh("0123456789abcdefABCDEF, hello world!");
        // Scalar reference: punctuation + whitespace stripped.
        assert_eq!(a, "0123456789abcdefABCDEFhelloworld");
    }
}
