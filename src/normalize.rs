use unicode_general_category::{get_general_category, GeneralCategory};
use unicode_normalization::UnicodeNormalization;

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

    for ch in text.nfkc() {
        if ch.is_whitespace() {
            continue;
        }

        let cat = get_general_category(ch);
        if matches!(
            cat,
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
        ) {
            continue;
        }

        out.push(ch);
    }

    out
}
