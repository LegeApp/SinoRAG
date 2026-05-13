use anyhow::{Context, Result};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

const VALID_EVIDENCE_ROLES: &[&str] = &["seed", "candidate"];
const VALID_RELATION_LABELS: &[&str] = &[
    "exact-reuse",
    "variant-reuse",
    "image-reuse",
    "biographical-retelling",
    "commentarial-reuse",
    "formulaic-reuse",
    "quotes-canon",
    "cites-sutra",
    "quotes-sastra",
    "echoes-canon",
    "inherits-phrase-from",
    "uses-transliteration",
    "cluster-member",
    "reference-scaffold",
];
const VALID_REVIEW_STATES: &[&str] = &["auto_import", "needs_review", "candidate_only"];
const VALID_BOILERPLATE: &[&str] = &["not_boilerplate", "possible_boilerplate", "boilerplate"];
const VALID_RENDER_POLICIES: &[&str] = &["render_default", "collapse", "store_hidden"];

fn evidence_role(ev: &serde_json::Value) -> Option<&str> {
    if let Some(role) = ev.get("evidence_role").and_then(|v| v.as_str()) {
        return Some(role);
    }

    match ev.get("side").and_then(|v| v.as_str()) {
        Some("zen") => Some("seed"),
        Some("canon") => Some("candidate"),
        Some(other) => Some(other),
        None => None,
    }
}

fn relation_label(claim: &serde_json::Value) -> Option<&str> {
    claim
        .get("relation_label")
        .and_then(|v| v.as_str())
        .or_else(|| {
            claim
                .get("graph_hint")
                .and_then(|h| h.get("edge_label"))
                .and_then(|v| v.as_str())
        })
        .or_else(|| claim.get("claim_type").and_then(|v| v.as_str()))
}

fn render_policy(claim: &serde_json::Value) -> Option<&str> {
    claim
        .get("graph_hint")
        .and_then(|h| h.get("render_policy"))
        .and_then(|v| v.as_str())
        .or_else(|| {
            claim
                .get("graph_hint")
                .and_then(|h| h.get("render"))
                .and_then(|v| v.as_str())
        })
}

fn map_ring(claim: &serde_json::Value) -> Option<i64> {
    if let Some(n) = claim
        .get("graph_hint")
        .and_then(|h| h.get("map_ring"))
        .and_then(|v| v.as_i64())
    {
        return Some(n);
    }

    match claim.get("ring").and_then(|v| v.as_str()) {
        Some("ring_1") => Some(1),
        Some("ring_2") => Some(2),
        Some("ring_3") => Some(3),
        _ => None,
    }
}

fn validate_confidence(claim: &serde_json::Value, cid: &str, errors: &mut Vec<String>) {
    let Some(conf) = claim.get("confidence") else {
        return;
    };

    if let Some(c) = conf.as_f64() {
        if !(0.0..=1.0).contains(&c) {
            errors.push(format!("{}: confidence {} out of range [0.0, 1.0]", cid, c));
        }
        return;
    }

    if let Some(s) = conf.as_str() {
        if !["low", "medium", "high"].contains(&s) {
            errors.push(format!("{}: invalid confidence string {:?}", cid, s));
        }
        return;
    }

    errors.push(format!(
        "{}: confidence must be number or low/medium/high",
        cid
    ));
}

pub fn run(adjudication: PathBuf) -> Result<()> {
    if !adjudication.exists() {
        let result = serde_json::json!({
            "file": adjudication.display().to_string(),
            "status": "ERROR",
            "errors": [format!("file not found: {}", adjudication.display())]
        });
        println!("{}", serde_json::to_string_pretty(&result)?);
        anyhow::bail!("file not found");
    }

    let content = fs::read_to_string(&adjudication).with_context(|| {
        format!(
            "failed to read adjudication file: {}",
            adjudication.display()
        )
    })?;
    let payload: Value = serde_json::from_str(&content).with_context(|| {
        format!(
            "invalid JSON in adjudication file: {}",
            adjudication.display()
        )
    })?;

    let errors = check_adjudication(&payload);
    let status = if errors.is_empty() {
        "PASSED"
    } else {
        "FAILED"
    };

    let result = serde_json::json!({
        "file": adjudication.display().to_string(),
        "accepted_claims": payload.get("accepted_claims").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0),
        "rejected_candidates": payload.get("rejected_candidates").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0),
        "errors": errors,
        "status": status
    });

    println!("{}", serde_json::to_string_pretty(&result)?);

    if errors.is_empty() {
        Ok(())
    } else {
        anyhow::bail!("validation failed")
    }
}

fn check_adjudication(payload: &Value) -> Vec<String> {
    let mut errors = Vec::new();

    for field in [
        "task_id",
        "seed_passage_id",
        "accepted_claims",
        "rejected_candidates",
    ] {
        if !payload.get(field).is_some() {
            errors.push(format!("missing top-level field: {:?}", field));
        }
    }

    if let Some(claims) = payload.get("accepted_claims").and_then(|v| v.as_array()) {
        for claim in claims {
            if let Some(claim_obj) = claim.as_object() {
                let cid = claim_obj
                    .get("claim_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("<no claim_id>");

                let evidence_vec = claim_obj
                    .get("evidence")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                let evidence = &evidence_vec;
                if evidence.len() < 2 {
                    errors.push(format!(
                        "{}: evidence must have at least 2 items (got {})",
                        cid,
                        evidence.len()
                    ));
                } else {
                    let roles: std::collections::HashSet<&str> =
                        evidence.iter().filter_map(|e| evidence_role(e)).collect();
                    if !roles.contains("seed") {
                        errors.push(format!("{}: evidence missing a 'seed' role item", cid));
                    }
                    if !roles.contains("candidate") {
                        errors.push(format!("{}: evidence missing a 'candidate' role item", cid));
                    }
                    for (i, ev) in evidence.iter().enumerate() {
                        let has_zh_quote = ev
                            .get("zh_quote")
                            .and_then(|v| v.as_str())
                            .map(|s| !s.is_empty())
                            .unwrap_or(false);
                        let has_quote_zh = ev
                            .get("quote_zh")
                            .and_then(|v| v.as_str())
                            .map(|s| !s.is_empty())
                            .unwrap_or(false);
                        if !has_zh_quote && !has_quote_zh {
                            errors.push(format!("{}: evidence[{}] missing zh_quote", cid, i));
                        }
                        if ev
                            .get("passage_id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.is_empty())
                            .unwrap_or(true)
                        {
                            errors.push(format!("{}: evidence[{}] missing passage_id", cid, i));
                        }
                        let role = evidence_role(ev);
                        if let Some(r) = role {
                            if !VALID_EVIDENCE_ROLES.contains(&r) {
                                errors.push(format!(
                                    "{}: evidence[{}] invalid evidence_role {:?}",
                                    cid, i, r
                                ));
                            }
                        }
                    }
                }

                if claim_obj
                    .get("matched_phrases")
                    .and_then(|v| v.as_array())
                    .map(|a| a.is_empty())
                    .unwrap_or(true)
                {
                    errors.push(format!("{}: missing or empty matched_phrases", cid));
                }

                let rel = relation_label(claim);
                if let Some(r) = rel {
                    if !VALID_RELATION_LABELS.contains(&r) {
                        errors.push(format!("{}: invalid relation_label {:?}", cid, r));
                    }
                }

                let rs = claim_obj.get("review_state").and_then(|v| v.as_str());
                if let Some(s) = rs {
                    if !VALID_REVIEW_STATES.contains(&s) {
                        errors.push(format!("{}: invalid review_state {:?}", cid, s));
                    }
                }

                validate_confidence(claim, cid, &mut errors);

                let ba = claim_obj
                    .get("boilerplate_assessment")
                    .and_then(|v| v.as_str());
                if let Some(b) = ba {
                    if !VALID_BOILERPLATE.contains(&b) {
                        errors.push(format!("{}: invalid boilerplate_assessment {:?}", cid, b));
                    }
                }

                let hint_obj = claim_obj
                    .get("graph_hint")
                    .and_then(|v| v.as_object())
                    .cloned()
                    .unwrap_or_default();
                let hint = &hint_obj;
                if hint.get("research_lens").is_none() {
                    errors.push(format!("{}: graph_hint missing research_lens", cid));
                }

                let mr = map_ring(claim);
                if let Some(r) = mr {
                    if r != 1 && r != 2 && r != 3 {
                        errors.push(format!(
                            "{}: graph_hint map_ring {} not in {{1, 2, 3}}",
                            cid, r
                        ));
                    }
                }

                let rp = render_policy(claim);
                if let Some(p) = rp {
                    if !VALID_RENDER_POLICIES.contains(&p) {
                        errors.push(format!("{}: graph_hint invalid render_policy {:?}", cid, p));
                    }
                }
            }
        }
    }

    errors
}
