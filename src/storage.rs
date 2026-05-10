use crate::models::PassageRecord;
use anyhow::Result;
use arrow::array::{ArrayRef, BooleanArray, Int32Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const PARQUET_BATCH_SIZE: usize = 25_000;

#[derive(Default)]
pub struct PassageBatch {
    source_corpus: Vec<String>,
    source_work_id: Vec<String>,
    source_section_id: Vec<String>,
    source_locator: Vec<String>,
    source_url: Vec<String>,
    edition_siglum: Vec<String>,
    edition_label: Vec<String>,
    rights_id: Vec<String>,
    rights_notes: Vec<String>,
    retrieval_method: Vec<String>,
    snapshot_id: Vec<String>,
    quality_flags_json: Vec<String>,
    passage_id: Vec<String>,
    source_rel_path: Vec<String>,
    xml_id: Vec<String>,
    div_path: Vec<String>,
    heading: Vec<String>,
    heading_path: Vec<String>,
    from_lb: Vec<Option<String>>,
    to_lb: Vec<Option<String>>,
    zh_text_raw: Vec<String>,
    zh_text_normalized: Vec<String>,
    text_type: Vec<String>,
    contains_person: Vec<bool>,
    contains_term: Vec<bool>,
    contains_foreign: Vec<bool>,
    canon: Vec<String>,
    canon_name: Vec<String>,
    traditions: Vec<String>,
    period: Vec<String>,
    origin: Vec<String>,
    author: Vec<String>,
    main_title: Vec<String>,
    period_rank: Vec<i32>,
    zh: Vec<String>,
    normalized_zh: Vec<String>,
}

impl PassageBatch {
    pub fn push(&mut self, passage: &PassageRecord) -> Result<()> {
        self.source_corpus.push(passage.source_corpus.clone());
        self.source_work_id.push(passage.source_work_id.clone());
        self.source_section_id
            .push(passage.source_section_id.clone());
        self.source_locator.push(passage.source_locator.clone());
        self.source_url.push(passage.source_url.clone());
        self.edition_siglum.push(passage.edition_siglum.clone());
        self.edition_label.push(passage.edition_label.clone());
        self.rights_id.push(passage.rights_id.clone());
        self.rights_notes.push(passage.rights_notes.clone());
        self.retrieval_method.push(passage.retrieval_method.clone());
        self.snapshot_id.push(passage.snapshot_id.clone());
        self.quality_flags_json
            .push(passage.quality_flags_json.clone());
        self.passage_id.push(passage.passage_id.clone());
        self.source_rel_path.push(passage.source_rel_path.clone());
        self.xml_id.push(passage.xml_id.clone());
        self.div_path.push(passage.div_path.clone());
        self.heading.push(passage.heading.clone());
        self.heading_path.push(passage.heading_path.clone());
        self.from_lb.push(passage.from_lb.clone());
        self.to_lb.push(passage.to_lb.clone());
        self.zh_text_raw.push(passage.zh_text_raw.clone());
        self.zh_text_normalized
            .push(passage.zh_text_normalized.clone());
        self.text_type.push(passage.text_type.clone());
        self.contains_person.push(passage.contains_person);
        self.contains_term.push(passage.contains_term);
        self.contains_foreign.push(passage.contains_foreign);
        self.canon.push(passage.canon.clone());
        self.canon_name.push(passage.canon_name.clone());
        self.traditions
            .push(serde_json::to_string(&passage.traditions)?);
        self.period.push(passage.period.clone());
        self.origin.push(passage.origin.clone());
        self.author.push(passage.author.clone());
        self.main_title.push(passage.main_title.clone());
        self.period_rank.push(passage.period_rank);
        self.zh.push(passage.zh.clone());
        self.normalized_zh.push(passage.normalized_zh.clone());
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.passage_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.passage_id.is_empty()
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

pub fn reset_parquet_dir(out_dir: &Path) -> Result<()> {
    if out_dir.exists() {
        std::fs::remove_dir_all(out_dir)?;
    }
    std::fs::create_dir_all(out_dir)?;
    Ok(())
}

pub fn write_parquet_part(
    batch: &PassageBatch,
    out_dir: &Path,
    part_index: usize,
) -> Result<PathBuf> {
    let path = out_dir.join(format!("part-{part_index:06}.parquet"));
    let file = File::create(&path)?;
    let schema = passage_schema();
    let record_batch = batch.to_record_batch(schema.clone())?;
    let props = WriterProperties::builder()
        .set_compression(Compression::ZSTD(Default::default()))
        .build();
    let mut writer = ArrowWriter::try_new(file, schema, Some(props))?;
    writer.write(&record_batch)?;
    writer.close()?;
    Ok(path)
}

pub fn write_parquet_part_partitioned(
    batch: &PassageBatch,
    out_dir: &Path,
    source_corpus: &str,
    part_index: usize,
) -> Result<PathBuf> {
    let partition_dir = out_dir.join(format!("source_corpus={source_corpus}"));
    std::fs::create_dir_all(&partition_dir)?;
    let path = partition_dir.join(format!("part-{part_index:06}.parquet"));
    let file = File::create(&path)?;
    let schema = passage_schema();
    let record_batch = batch.to_record_batch(schema.clone())?;
    let props = WriterProperties::builder()
        .set_compression(Compression::ZSTD(Default::default()))
        .build();
    let mut writer = ArrowWriter::try_new(file, schema, Some(props))?;
    writer.write(&record_batch)?;
    writer.close()?;
    Ok(path)
}

impl PassageBatch {
    fn to_record_batch(&self, schema: Arc<Schema>) -> Result<RecordBatch> {
        let arrays: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from(self.source_corpus.clone())),
            Arc::new(StringArray::from(self.source_work_id.clone())),
            Arc::new(StringArray::from(self.source_section_id.clone())),
            Arc::new(StringArray::from(self.source_locator.clone())),
            Arc::new(StringArray::from(self.source_url.clone())),
            Arc::new(StringArray::from(self.edition_siglum.clone())),
            Arc::new(StringArray::from(self.edition_label.clone())),
            Arc::new(StringArray::from(self.rights_id.clone())),
            Arc::new(StringArray::from(self.rights_notes.clone())),
            Arc::new(StringArray::from(self.retrieval_method.clone())),
            Arc::new(StringArray::from(self.snapshot_id.clone())),
            Arc::new(StringArray::from(self.quality_flags_json.clone())),
            Arc::new(StringArray::from(self.passage_id.clone())),
            Arc::new(StringArray::from(self.source_rel_path.clone())),
            Arc::new(StringArray::from(self.xml_id.clone())),
            Arc::new(StringArray::from(self.div_path.clone())),
            Arc::new(StringArray::from(self.heading.clone())),
            Arc::new(StringArray::from(self.heading_path.clone())),
            Arc::new(StringArray::from(self.from_lb.clone())),
            Arc::new(StringArray::from(self.to_lb.clone())),
            Arc::new(StringArray::from(self.zh_text_raw.clone())),
            Arc::new(StringArray::from(self.zh_text_normalized.clone())),
            Arc::new(StringArray::from(self.text_type.clone())),
            Arc::new(BooleanArray::from(self.contains_person.clone())),
            Arc::new(BooleanArray::from(self.contains_term.clone())),
            Arc::new(BooleanArray::from(self.contains_foreign.clone())),
            Arc::new(StringArray::from(self.canon.clone())),
            Arc::new(StringArray::from(self.canon_name.clone())),
            Arc::new(StringArray::from(self.traditions.clone())),
            Arc::new(StringArray::from(self.period.clone())),
            Arc::new(StringArray::from(self.origin.clone())),
            Arc::new(StringArray::from(self.author.clone())),
            Arc::new(StringArray::from(self.main_title.clone())),
            Arc::new(Int32Array::from(self.period_rank.clone())),
            Arc::new(StringArray::from(self.zh.clone())),
            Arc::new(StringArray::from(self.normalized_zh.clone())),
        ];
        Ok(RecordBatch::try_new(schema, arrays)?)
    }
}

fn passage_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("source_corpus", DataType::Utf8, false),
        Field::new("source_work_id", DataType::Utf8, false),
        Field::new("source_section_id", DataType::Utf8, false),
        Field::new("source_locator", DataType::Utf8, false),
        Field::new("source_url", DataType::Utf8, false),
        Field::new("edition_siglum", DataType::Utf8, false),
        Field::new("edition_label", DataType::Utf8, false),
        Field::new("rights_id", DataType::Utf8, false),
        Field::new("rights_notes", DataType::Utf8, false),
        Field::new("retrieval_method", DataType::Utf8, false),
        Field::new("snapshot_id", DataType::Utf8, false),
        Field::new("quality_flags_json", DataType::Utf8, false),
        Field::new("passage_id", DataType::Utf8, false),
        Field::new("source_rel_path", DataType::Utf8, false),
        Field::new("xml_id", DataType::Utf8, false),
        Field::new("div_path", DataType::Utf8, false),
        Field::new("heading", DataType::Utf8, false),
        Field::new("heading_path", DataType::Utf8, false),
        Field::new("from_lb", DataType::Utf8, true),
        Field::new("to_lb", DataType::Utf8, true),
        Field::new("zh_text_raw", DataType::Utf8, false),
        Field::new("zh_text_normalized", DataType::Utf8, false),
        Field::new("text_type", DataType::Utf8, false),
        Field::new("contains_person", DataType::Boolean, false),
        Field::new("contains_term", DataType::Boolean, false),
        Field::new("contains_foreign", DataType::Boolean, false),
        Field::new("canon", DataType::Utf8, false),
        Field::new("canon_name", DataType::Utf8, false),
        Field::new("traditions", DataType::Utf8, false),
        Field::new("period", DataType::Utf8, false),
        Field::new("origin", DataType::Utf8, false),
        Field::new("author", DataType::Utf8, false),
        Field::new("main_title", DataType::Utf8, false),
        Field::new("period_rank", DataType::Int32, false),
        Field::new("zh", DataType::Utf8, false),
        Field::new("normalized_zh", DataType::Utf8, false),
    ]))
}
