use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassageRecord {
    #[serde(default)]
    pub source_corpus: String,
    #[serde(default)]
    pub source_work_id: String,
    #[serde(default)]
    pub source_section_id: String,
    #[serde(default)]
    pub source_locator: String,
    #[serde(default)]
    pub source_url: String,
    #[serde(default)]
    pub edition_siglum: String,
    #[serde(default)]
    pub edition_label: String,
    #[serde(default)]
    pub rights_id: String,
    #[serde(default)]
    pub rights_notes: String,
    #[serde(default)]
    pub retrieval_method: String,
    #[serde(default)]
    pub snapshot_id: String,
    #[serde(default)]
    pub quality_flags_json: String,
    pub passage_id: String,
    pub source_rel_path: String,
    pub xml_id: String,
    pub div_path: String,
    pub heading: String,
    pub heading_path: String,
    pub from_lb: Option<String>,
    pub to_lb: Option<String>,
    #[serde(default)]
    pub passage_ord_in_file: u32,
    pub zh_text_raw: String,
    pub zh_text_normalized: String,
    pub text_type: String,
    pub contains_person: bool,
    pub contains_term: bool,
    pub contains_foreign: bool,
    pub canon: String,
    pub canon_name: String,
    pub traditions: Vec<String>,
    pub period: String,
    pub origin: String,
    pub author: String,
    pub main_title: String,
    pub period_rank: i32,
    pub zh: String,
    pub normalized_zh: String,
}

impl PassageRecord {
    pub fn finalize_aliases(mut self) -> Self {
        self.zh = self.zh_text_raw.clone();
        self.normalized_zh = self.zh_text_normalized.clone();
        self
    }
}
