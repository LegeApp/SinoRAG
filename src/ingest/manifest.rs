use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestManifest {
    pub version: String,
    pub created: u64,
    pub source_root: PathBuf,
    pub sources: Vec<SourceStats>,
    pub total_passages: u64,
    pub errors: Vec<IngestError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceStats {
    pub source_type: String,
    pub source_path: String,
    pub work_id: Option<String>,
    pub passages_extracted: u64,
    pub bytes_read: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestError {
    pub source_path: String,
    pub error_type: String,
    pub message: String,
}

impl IngestManifest {
    pub fn new(source_root: PathBuf) -> Self {
        Self {
            version: "1.0".to_string(),
            created: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            source_root,
            sources: Vec::new(),
            total_passages: 0,
            errors: Vec::new(),
        }
    }

    pub fn add_source(&mut self, stats: SourceStats) {
        self.total_passages += stats.passages_extracted;
        self.sources.push(stats);
    }

    pub fn add_error(&mut self, path: String, error_type: &str, message: &str) {
        self.errors.push(IngestError {
            source_path: path,
            error_type: error_type.to_string(),
            message: message.to_string(),
        });
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, json)?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let json = fs::read_to_string(path)?;
        let manifest: IngestManifest = serde_json::from_str(&json)?;
        Ok(manifest)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParquetStats {
    pub path: PathBuf,
    pub total_rows: u64,
    pub partitions: HashMap<String, u64>,
    pub unique_works: u64,
    pub unique_corpus: Vec<String>,
    pub schema_columns: Vec<String>,
}

impl ParquetStats {
    pub fn from_parquet(parquet_path: &Path) -> Result<Self> {
        use crate::phrase_index::parquet_files;

        let files = parquet_files(parquet_path)?;
        let mut total_rows = 0u64;
        let mut partitions: HashMap<String, u64> = HashMap::new();
        let mut all_work_ids: Vec<String> = Vec::new();
        let mut all_corpus: Vec<String> = Vec::new();

        for file_path in files {
            let file = std::fs::File::open(&file_path)?;
            let builder = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(file)?;
            let reader = builder.build()?;

            let schema = builder.schema();
            let parquet_file = parquet::file::reader::SerializedFileReader::new(file)
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            let metadata = parquet_file.metadata();
            total_rows += metadata.num_rows() as u64;

            // Get partition info from path
            let path_str = file_path.to_string_lossy().to_string();
            for part in path_str.split('/') {
                if part.starts_with("source_corpus=") {
                    let corpus = part.trim_start_matches("source_corpus=").to_string();
                    *partitions.entry(corpus).or_insert(0) += 1;
                }
            }
        }

        Ok(Self {
            path: parquet_path.to_path_buf(),
            total_rows,
            partitions,
            unique_works: all_work_ids.len() as u64,
            unique_corpus: all_corpus,
            schema_columns: Vec::new(),
        })
    }
}

pub fn validate_pipeline(
    manifest_path: Option<&Path>,
    parquet_path: &Path,
) -> Result<PipelineValidation> {
    let manifest = match manifest_path {
        Some(p) => Some(IngestManifest::load(p)?),
        None => None,
    };

    let parquet_stats = ParquetStats::from_parquet(parquet_path)?;

    let mut validation = PipelineValidation {
        manifest,
        parquet_stats,
        issues: Vec::new(),
        work_coverage: HashMap::new(),
    };

    // Check for issues
    if let Some(m) = &validation.manifest {
        if m.errors.len() > 0 {
            validation.issues.push(format!("{} source files had errors", m.errors.len()));
        }
    }

    Ok(validation)
}

#[derive(Debug)]
pub struct PipelineValidation {
    pub manifest: Option<IngestManifest>,
    pub parquet_stats: ParquetStats,
    pub issues: Vec<String>,
    pub work_coverage: HashMap<String, f64>,
}
