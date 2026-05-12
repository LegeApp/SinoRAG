use anyhow::{anyhow, Result};
use std::path::PathBuf;

use crate::research::context_expand::expand_passage_context;
use crate::datafusion_store::DataFusionStore;
use crate::search_packet::SearchResultPacket;

pub async fn run(
    parquet: PathBuf,
    passage_id: Option<String>,
    session: Option<PathBuf>,
    hit: Option<String>,
    before: usize,
    after: usize,
    out: Option<PathBuf>,
) -> Result<()> {
    let resolved = resolve_passage_id(passage_id, session, hit)?;
    let store = DataFusionStore::open(&parquet).await?;

    let payload = expand_passage_context(
        &store,
        &resolved.passage_id,
        resolved.hit_id,
        before,
        after,
    )
    .await?;

    let text = serde_json::to_string_pretty(&payload)? + "\n";

    if let Some(out) = out {
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&out, text)?;
        eprintln!("wrote {}", out.display());
    } else {
        print!("{text}");
    }

    Ok(())
}

struct ResolvedHit {
    passage_id: String,
    hit_id: Option<String>,
}

fn resolve_passage_id(
    passage_id: Option<String>,
    session: Option<PathBuf>,
    hit: Option<String>,
) -> Result<ResolvedHit> {
    match (passage_id, session, hit) {
        (Some(passage_id), None, None) => Ok(ResolvedHit {
            passage_id,
            hit_id: None,
        }),

        (None, Some(session), Some(hit_id)) => {
            let packet = SearchResultPacket::load(&session)?;
            let hit = packet.find_hit(&hit_id)?;

            Ok(ResolvedHit {
                passage_id: hit.passage_id.clone(),
                hit_id: Some(hit_id),
            })
        }

        _ => Err(anyhow!(
            "provide either --passage-id OR both --session and --hit"
        )),
    }
}
