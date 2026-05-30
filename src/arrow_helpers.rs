use anyhow::Result;
use arrow::array::{Array, Float32Array, Float64Array, Int32Array, Int64Array, StringArray};
use arrow::record_batch::RecordBatch;

pub fn col_str(batch: &RecordBatch, idx: usize) -> Result<&StringArray> {
    batch
        .column(idx)
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| anyhow::anyhow!("column {idx} is not StringArray"))
}

pub fn col_i32(batch: &RecordBatch, idx: usize) -> Result<&Int32Array> {
    batch
        .column(idx)
        .as_any()
        .downcast_ref::<Int32Array>()
        .ok_or_else(|| anyhow::anyhow!("column {idx} is not Int32Array"))
}

pub fn col_i64(batch: &RecordBatch, idx: usize) -> Result<&Int64Array> {
    batch
        .column(idx)
        .as_any()
        .downcast_ref::<Int64Array>()
        .ok_or_else(|| anyhow::anyhow!("column {idx} is not Int64Array"))
}

pub fn col_f32(batch: &RecordBatch, idx: usize) -> Result<&Float32Array> {
    batch
        .column(idx)
        .as_any()
        .downcast_ref::<Float32Array>()
        .ok_or_else(|| anyhow::anyhow!("column {idx} is not Float32Array"))
}

pub fn col_f64(batch: &RecordBatch, idx: usize) -> Result<&Float64Array> {
    batch
        .column(idx)
        .as_any()
        .downcast_ref::<Float64Array>()
        .ok_or_else(|| anyhow::anyhow!("column {idx} is not Float64Array"))
}

/// Extract the `passage_id` and `zh_text_normalized` columns from a batch.
/// Used by both phrase_index and tfidf builders.
pub fn extract_passage_columns(batch: &RecordBatch) -> Result<(&StringArray, &StringArray)> {
    let passage_col = batch
        .schema()
        .column_with_name("passage_id")
        .ok_or_else(|| anyhow::anyhow!("missing passage_id column"))?
        .0;
    let text_col = batch
        .schema()
        .column_with_name("zh_text_normalized")
        .ok_or_else(|| anyhow::anyhow!("missing zh_text_normalized column"))?
        .0;
    let pids = col_str(batch, passage_col)?;
    let texts = col_str(batch, text_col)?;
    Ok((pids, texts))
}
