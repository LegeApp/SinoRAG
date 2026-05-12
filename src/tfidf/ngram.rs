/// Returns deduplicated n-gram hashes for a document.
/// Use this for document-frequency (DF) counting — each hash appears at most once.
pub fn char_ngram_hashes(text: &str, min_n: usize, max_n: usize) -> Vec<u64> {
    use xxhash_rust::xxh3::xxh3_64;

    let chars: Vec<char> = text.chars().filter(|c| is_cjk_ideograph(*c)).collect();
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();

    for n in min_n..=max_n {
        if chars.len() < n { continue; }

        for i in 0..=chars.len() - n {
            let mut gram = String::new();
            for ch in &chars[i..i + n] {
                gram.push(*ch);
            }
            if low_value_ngram(&gram) { continue; }

            let hash = xxh3_64(gram.as_bytes());
            if seen.insert(hash) {
                result.push(hash);
            }
        }
    }

    result
}

/// Returns all n-gram hashes including duplicates, for term-frequency (TF) counting.
/// Unlike `char_ngram_hashes`, the same hash can appear multiple times if the n-gram
/// recurs within the document.
pub fn char_ngram_hashes_all(text: &str, min_n: usize, max_n: usize) -> Vec<u64> {
    use xxhash_rust::xxh3::xxh3_64;

    let chars: Vec<char> = text.chars().filter(|c| is_cjk_ideograph(*c)).collect();
    let mut result = Vec::new();

    for n in min_n..=max_n {
        if chars.len() < n { continue; }
        for i in 0..=chars.len() - n {
            let mut gram = String::with_capacity(n * 4);
            for ch in &chars[i..i + n] {
                gram.push(*ch);
            }
            if low_value_ngram(&gram) { continue; }
            result.push(xxh3_64(gram.as_bytes()));
        }
    }

    result
}

pub fn char_ngrams(text: &str, min_n: usize, max_n: usize) -> impl Iterator<Item = String> + '_ {
    // Strip to CJK ideographs only — handles any non-CJK chars that survive upstream normalization
    // and prevents punctuation/digit artifacts from becoming accidental features.
    let chars: Vec<char> = text.chars().filter(|c| is_cjk_ideograph(*c)).collect();
    let mut grams = Vec::new();

    for n in min_n..=max_n {
        if chars.len() < n {
            continue;
        }
        for i in 0..=(chars.len() - n) {
            let gram: String = chars[i..i + n].iter().collect();
            if !low_value_ngram(&gram) {
                grams.push(gram);
            }
        }
    }

    grams.into_iter()
}

fn is_cjk_ideograph(c: char) -> bool {
    ('\u{3400}'..='\u{4DBF}').contains(&c)    // CJK Extension A
        || ('\u{4E00}'..='\u{9FFF}').contains(&c) // CJK Unified Ideographs
        || ('\u{F900}'..='\u{FAFF}').contains(&c) // CJK Compatibility Ideographs
        || ('\u{20000}'..='\u{2A6DF}').contains(&c) // CJK Extension B
}

fn low_value_ngram(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    low_value_ngram_chars(&chars)
}

fn low_value_ngram_chars(chars: &[char]) -> bool {
    if chars.is_empty() {
        return true;
    }
    if chars.iter().all(|c| *c == chars[0]) {
        return true;
    }
    // ABAB repeating pattern — e.g. 如是如是, 云何云何
    let len = chars.len();
    if len >= 4 && len % 2 == 0 {
        let half = len / 2;
        if chars[..half] == chars[half..] {
            return true;
        }
    }
    false
}

/// Writes deduplicated n-gram hashes into `out` (clears first). Reuses `out`
/// across calls to avoid repeated allocation.
pub fn char_ngram_hashes_into(text: &str, min_n: usize, max_n: usize, out: &mut Vec<u64>) {
    use std::collections::HashSet;
    use xxhash_rust::xxh3::xxh3_64;
    out.clear();
    let chars: Vec<char> = text.chars().filter(|c| is_cjk_ideograph(*c)).collect();
    let mut seen: HashSet<u64> = HashSet::new();
    let mut buf = Vec::<u8>::with_capacity(max_n * 4);
    for n in min_n..=max_n {
        if chars.len() < n {
            continue;
        }
        for i in 0..=(chars.len() - n) {
            let window = &chars[i..i + n];
            if low_value_ngram_chars(window) {
                continue;
            }
            buf.clear();
            for ch in window {
                let mut tmp = [0u8; 4];
                buf.extend_from_slice(ch.encode_utf8(&mut tmp).as_bytes());
            }
            let hash = xxh3_64(&buf);
            if seen.insert(hash) {
                out.push(hash);
            }
        }
    }
}

/// Writes all n-gram hashes (including duplicates) into `out` (clears first).
/// Reuses `out` across calls to avoid repeated allocation.
pub fn char_ngram_hashes_all_into(text: &str, min_n: usize, max_n: usize, out: &mut Vec<u64>) {
    use xxhash_rust::xxh3::xxh3_64;
    out.clear();
    let chars: Vec<char> = text.chars().filter(|c| is_cjk_ideograph(*c)).collect();
    let mut buf = Vec::<u8>::with_capacity(max_n * 4);
    for n in min_n..=max_n {
        if chars.len() < n {
            continue;
        }
        for i in 0..=(chars.len() - n) {
            let window = &chars[i..i + n];
            if low_value_ngram_chars(window) {
                continue;
            }
            buf.clear();
            for ch in window {
                let mut tmp = [0u8; 4];
                buf.extend_from_slice(ch.encode_utf8(&mut tmp).as_bytes());
            }
            out.push(xxh3_64(&buf));
        }
    }
}
