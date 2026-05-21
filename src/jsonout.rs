use anyhow::Result;
use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;

pub fn value_str(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

pub fn write_or_print<T: Serialize>(payload: &T, out: Option<PathBuf>) -> Result<()> {
    let text = serde_json::to_string_pretty(payload)?;
    if let Some(out) = out {
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&out, text + "\n")?;
        println!("wrote {}", out.display());
    } else {
        println!("{text}");
    }
    Ok(())
}
