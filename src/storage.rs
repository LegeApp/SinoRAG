use crate::models::PassageRecord;
use anyhow::Result;
use arrow::array::{ArrayRef, BooleanArray, Float64Array, Int32Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const PARQUET_BATCH_SIZE: usize = 25_000;

/// Compression codec for Parquet output.
///
/// ZSTD is the default — good ratio and fast decode. Use `Uncompressed` when
/// you want to apply a separate outer compression (e.g. 7z/LZMA2) without paying
/// the ZSTD overhead twice, or to benchmark different codecs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ParquetCompression {
    #[default]
    Zstd,
    Uncompressed,
}

impl From<ParquetCompression> for Compression {
    fn from(c: ParquetCompression) -> Self {
        match c {
            ParquetCompression::Zstd => Compression::ZSTD(Default::default()),
            ParquetCompression::Uncompressed => Compression::UNCOMPRESSED,
        }
    }
}

/// Auto-heal the pack-prep naming mismatch: that workflow can leave the passage
/// store on disk as `passages-raw.parquet` (uncompressed source) instead of the
/// canonical `passages.parquet`. If `canonical` is missing but its
/// `passages-raw.parquet` sibling exists, rename the sibling into place.
///
/// Returns `Ok(true)` if a rename happened, `Ok(false)` if there was nothing to do.
/// Callers own any user-facing messaging.
pub fn heal_raw_parquet(canonical: &Path) -> std::io::Result<bool> {
    let raw = canonical.with_file_name("passages-raw.parquet");
    if canonical.exists() || !raw.exists() {
        return Ok(false);
    }
    std::fs::rename(&raw, canonical)?;
    Ok(true)
}

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
    /// Per-passage corpus-specific metadata as JSON. Nullable; null for CBETA
    /// rows, populated by non-TEI ingesters (e.g. terebess).
    metadata_json: Vec<Option<String>>,
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
        self.metadata_json.push(None);
        Ok(())
    }

    /// Variant of `push` that also records corpus-specific metadata as JSON.
    /// Used by non-TEI ingesters whose extra fields don't fit the typed schema.
    pub fn push_with_metadata(
        &mut self,
        passage: &PassageRecord,
        metadata_json: Option<String>,
    ) -> Result<()> {
        self.push(passage)?;
        // Replace the trailing None we just pushed.
        let last = self.metadata_json.len() - 1;
        self.metadata_json[last] = metadata_json;
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
    compression: ParquetCompression,
) -> Result<PathBuf> {
    let path = out_dir.join(format!("part-{part_index:06}.parquet"));
    let file = File::create(&path)?;
    let schema = passage_schema();
    let record_batch = batch.to_record_batch(schema.clone())?;
    let props = WriterProperties::builder()
        .set_compression(compression.into())
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
    compression: ParquetCompression,
) -> Result<PathBuf> {
    let partition_dir = out_dir.join(format!("source_corpus={source_corpus}"));
    std::fs::create_dir_all(&partition_dir)?;
    let path = partition_dir.join(format!("part-{part_index:06}.parquet"));
    let file = File::create(&path)?;
    let schema = passage_schema();
    let record_batch = batch.to_record_batch(schema.clone())?;
    let props = WriterProperties::builder()
        .set_compression(compression.into())
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
            Arc::new(StringArray::from(self.metadata_json.clone())),
        ];
        Ok(RecordBatch::try_new(schema, arrays)?)
    }
}

// ---------------------------------------------------------------------------
// Dictionary parquet
// ---------------------------------------------------------------------------

pub const DICT_BATCH_SIZE: usize = 50_000;

#[derive(Default)]
pub struct DictBatch {
    term: Vec<String>,
    source: Vec<String>,
    sanskrit: Vec<Option<String>>,
    gloss: Vec<String>,
    usage_category: Vec<Option<String>>,
}

impl DictBatch {
    pub fn push(
        &mut self,
        term: String,
        source: String,
        sanskrit: Option<String>,
        gloss: String,
        usage_category: Option<String>,
    ) {
        self.term.push(term);
        self.source.push(source);
        self.sanskrit.push(sanskrit);
        self.gloss.push(gloss);
        self.usage_category.push(usage_category);
    }

    pub fn len(&self) -> usize {
        self.term.len()
    }

    pub fn is_empty(&self) -> bool {
        self.term.is_empty()
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    fn to_record_batch(&self, schema: Arc<Schema>) -> Result<RecordBatch> {
        let arrays: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from(self.term.clone())),
            Arc::new(StringArray::from(self.source.clone())),
            Arc::new(StringArray::from(self.sanskrit.clone())),
            Arc::new(StringArray::from(self.gloss.clone())),
            Arc::new(StringArray::from(self.usage_category.clone())),
        ];
        Ok(RecordBatch::try_new(schema, arrays)?)
    }
}

pub fn dict_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("term", DataType::Utf8, false),
        Field::new("source", DataType::Utf8, false),
        Field::new("sanskrit", DataType::Utf8, true),
        Field::new("gloss", DataType::Utf8, false),
        Field::new("usage_category", DataType::Utf8, true),
    ]))
}

pub fn write_dict_parquet_partitioned(
    batch: &DictBatch,
    out_dir: &Path,
    source_name: &str,
    part_index: usize,
    compression: ParquetCompression,
) -> Result<PathBuf> {
    let partition_dir = out_dir.join(format!("source={source_name}"));
    std::fs::create_dir_all(&partition_dir)?;
    let path = partition_dir.join(format!("part-{part_index:06}.parquet"));
    let file = File::create(&path)?;
    let schema = dict_schema();
    let record_batch = batch.to_record_batch(schema.clone())?;
    let props = WriterProperties::builder()
        .set_compression(compression.into())
        .build();
    let mut writer = ArrowWriter::try_new(file, schema, Some(props))?;
    writer.write(&record_batch)?;
    writer.close()?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// Person authority batch
// ---------------------------------------------------------------------------

pub const AUTHORITY_BATCH_SIZE: usize = 10_000;

#[derive(Default)]
pub struct PersonBatch {
    person_id: Vec<String>,
    primary_name: Vec<String>,
    primary_name_lang: Vec<String>,
    alt_names_json: Vec<String>,
    gender: Vec<Option<String>>,
    dynasty: Vec<Option<String>>,
    birth_year: Vec<Option<String>>,
    death_year: Vec<Option<String>>,
    occupation: Vec<Option<String>>,
    place_of_origin: Vec<Option<String>>,
    concise_bio: Vec<Option<String>>,
    teachers_json: Vec<String>,
    students_json: Vec<String>,
    wikidata_id: Vec<Option<String>>,
    cbdb_id: Vec<Option<String>>,
}

impl PersonBatch {
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        person_id: String,
        primary_name: String,
        primary_name_lang: String,
        alt_names_json: String,
        gender: Option<String>,
        dynasty: Option<String>,
        birth_year: Option<String>,
        death_year: Option<String>,
        occupation: Option<String>,
        place_of_origin: Option<String>,
        concise_bio: Option<String>,
        teachers_json: String,
        students_json: String,
        wikidata_id: Option<String>,
        cbdb_id: Option<String>,
    ) {
        self.person_id.push(person_id);
        self.primary_name.push(primary_name);
        self.primary_name_lang.push(primary_name_lang);
        self.alt_names_json.push(alt_names_json);
        self.gender.push(gender);
        self.dynasty.push(dynasty);
        self.birth_year.push(birth_year);
        self.death_year.push(death_year);
        self.occupation.push(occupation);
        self.place_of_origin.push(place_of_origin);
        self.concise_bio.push(concise_bio);
        self.teachers_json.push(teachers_json);
        self.students_json.push(students_json);
        self.wikidata_id.push(wikidata_id);
        self.cbdb_id.push(cbdb_id);
    }

    pub fn len(&self) -> usize {
        self.person_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.person_id.is_empty()
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    fn to_record_batch(&self, schema: Arc<Schema>) -> Result<RecordBatch> {
        let arrays: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from(self.person_id.clone())),
            Arc::new(StringArray::from(self.primary_name.clone())),
            Arc::new(StringArray::from(self.primary_name_lang.clone())),
            Arc::new(StringArray::from(self.alt_names_json.clone())),
            Arc::new(StringArray::from(self.gender.clone())),
            Arc::new(StringArray::from(self.dynasty.clone())),
            Arc::new(StringArray::from(self.birth_year.clone())),
            Arc::new(StringArray::from(self.death_year.clone())),
            Arc::new(StringArray::from(self.occupation.clone())),
            Arc::new(StringArray::from(self.place_of_origin.clone())),
            Arc::new(StringArray::from(self.concise_bio.clone())),
            Arc::new(StringArray::from(self.teachers_json.clone())),
            Arc::new(StringArray::from(self.students_json.clone())),
            Arc::new(StringArray::from(self.wikidata_id.clone())),
            Arc::new(StringArray::from(self.cbdb_id.clone())),
        ];
        Ok(RecordBatch::try_new(schema, arrays)?)
    }
}

pub fn person_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("person_id", DataType::Utf8, false),
        Field::new("primary_name", DataType::Utf8, false),
        Field::new("primary_name_lang", DataType::Utf8, false),
        Field::new("alt_names_json", DataType::Utf8, false),
        Field::new("gender", DataType::Utf8, true),
        Field::new("dynasty", DataType::Utf8, true),
        Field::new("birth_year", DataType::Utf8, true),
        Field::new("death_year", DataType::Utf8, true),
        Field::new("occupation", DataType::Utf8, true),
        Field::new("place_of_origin", DataType::Utf8, true),
        Field::new("concise_bio", DataType::Utf8, true),
        Field::new("teachers_json", DataType::Utf8, false),
        Field::new("students_json", DataType::Utf8, false),
        Field::new("wikidata_id", DataType::Utf8, true),
        Field::new("cbdb_id", DataType::Utf8, true),
    ]))
}

pub fn write_person_parquet(
    batch: &PersonBatch,
    out_dir: &Path,
    part_index: usize,
    compression: ParquetCompression,
) -> Result<PathBuf> {
    std::fs::create_dir_all(out_dir)?;
    let path = out_dir.join(format!("part-{part_index:06}.parquet"));
    let file = File::create(&path)?;
    let schema = person_schema();
    let record_batch = batch.to_record_batch(schema.clone())?;
    let props = WriterProperties::builder()
        .set_compression(compression.into())
        .build();
    let mut writer = ArrowWriter::try_new(file, schema, Some(props))?;
    writer.write(&record_batch)?;
    writer.close()?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// Place authority batch
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct PlaceBatch {
    place_id: Vec<String>,
    primary_name: Vec<String>,
    primary_name_lang: Vec<String>,
    alt_names_json: Vec<String>,
    latitude: Vec<Option<f64>>,
    longitude: Vec<Option<f64>>,
    geo_confidence: Vec<Option<String>>,
    district: Vec<Option<String>>,
    category: Vec<Option<String>>,
    description: Vec<Option<String>>,
    parent_place_id: Vec<Option<String>>,
}

impl PlaceBatch {
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        place_id: String,
        primary_name: String,
        primary_name_lang: String,
        alt_names_json: String,
        latitude: Option<f64>,
        longitude: Option<f64>,
        geo_confidence: Option<String>,
        district: Option<String>,
        category: Option<String>,
        description: Option<String>,
        parent_place_id: Option<String>,
    ) {
        self.place_id.push(place_id);
        self.primary_name.push(primary_name);
        self.primary_name_lang.push(primary_name_lang);
        self.alt_names_json.push(alt_names_json);
        self.latitude.push(latitude);
        self.longitude.push(longitude);
        self.geo_confidence.push(geo_confidence);
        self.district.push(district);
        self.category.push(category);
        self.description.push(description);
        self.parent_place_id.push(parent_place_id);
    }

    pub fn len(&self) -> usize {
        self.place_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.place_id.is_empty()
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    fn to_record_batch(&self, schema: Arc<Schema>) -> Result<RecordBatch> {
        let arrays: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from(self.place_id.clone())),
            Arc::new(StringArray::from(self.primary_name.clone())),
            Arc::new(StringArray::from(self.primary_name_lang.clone())),
            Arc::new(StringArray::from(self.alt_names_json.clone())),
            Arc::new(Float64Array::from(self.latitude.clone())),
            Arc::new(Float64Array::from(self.longitude.clone())),
            Arc::new(StringArray::from(self.geo_confidence.clone())),
            Arc::new(StringArray::from(self.district.clone())),
            Arc::new(StringArray::from(self.category.clone())),
            Arc::new(StringArray::from(self.description.clone())),
            Arc::new(StringArray::from(self.parent_place_id.clone())),
        ];
        Ok(RecordBatch::try_new(schema, arrays)?)
    }
}

pub fn place_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("place_id", DataType::Utf8, false),
        Field::new("primary_name", DataType::Utf8, false),
        Field::new("primary_name_lang", DataType::Utf8, false),
        Field::new("alt_names_json", DataType::Utf8, false),
        Field::new("latitude", DataType::Float64, true),
        Field::new("longitude", DataType::Float64, true),
        Field::new("geo_confidence", DataType::Utf8, true),
        Field::new("district", DataType::Utf8, true),
        Field::new("category", DataType::Utf8, true),
        Field::new("description", DataType::Utf8, true),
        Field::new("parent_place_id", DataType::Utf8, true),
    ]))
}

pub fn write_place_parquet(
    batch: &PlaceBatch,
    out_dir: &Path,
    part_index: usize,
    compression: ParquetCompression,
) -> Result<PathBuf> {
    std::fs::create_dir_all(out_dir)?;
    let path = out_dir.join(format!("part-{part_index:06}.parquet"));
    let file = File::create(&path)?;
    let schema = place_schema();
    let record_batch = batch.to_record_batch(schema.clone())?;
    let props = WriterProperties::builder()
        .set_compression(compression.into())
        .build();
    let mut writer = ArrowWriter::try_new(file, schema, Some(props))?;
    writer.write(&record_batch)?;
    writer.close()?;
    Ok(path)
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
        // Corpus-specific extras (e.g. terebess source_url + main_image_path).
        // Nullable so existing CBETA partitions read cleanly with null here.
        Field::new("metadata_json", DataType::Utf8, true),
    ]))
}
