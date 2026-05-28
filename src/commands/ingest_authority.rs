//! `sinorag ingest-authority` — parse DDBC person + place authority TEI XML into parquet.
//!
//! Sources (from cbeta-reader dict/):
//!   Buddhist_Studies_Person_Authority.xml  → data/persons.parquet/
//!   Buddhist_Studies_Place_Authority.xml   → data/places.parquet/

use crate::storage::{self, PersonBatch, PlaceBatch};
use anyhow::{Context, Result};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::path::{Path, PathBuf};

pub fn run(dict_dir: PathBuf, persons_out: PathBuf, places_out: PathBuf, parquet_compression: crate::storage::ParquetCompression) -> Result<()> {
    if !dict_dir.is_dir() {
        anyhow::bail!("directory not found: {}", dict_dir.display());
    }

    let person_xml = dict_dir.join("Buddhist_Studies_Person_Authority.xml");
    let place_xml = dict_dir.join("Buddhist_Studies_Place_Authority.xml");

    if person_xml.exists() {
        eprintln!("Ingesting persons from {}...", person_xml.display());
        let count = ingest_persons(&person_xml, &persons_out, parquet_compression)
            .with_context(|| format!("parsing {}", person_xml.display()))?;
        eprintln!("  {count} persons → {}", persons_out.display());
    } else {
        eprintln!("skip: person authority XML not found at {}", person_xml.display());
    }

    if place_xml.exists() {
        eprintln!("Ingesting places from {}...", place_xml.display());
        let count = ingest_places(&place_xml, &places_out, parquet_compression)
            .with_context(|| format!("parsing {}", place_xml.display()))?;
        eprintln!("  {count} places → {}", places_out.display());
    } else {
        eprintln!("skip: place authority XML not found at {}", place_xml.display());
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Person parser
// ---------------------------------------------------------------------------

fn ingest_persons(xml_path: &Path, out_dir: &Path, compression: crate::storage::ParquetCompression) -> Result<usize> {
    let data = std::fs::read(xml_path)?;
    let mut reader = Reader::from_reader(data.as_slice());
    reader.config_mut().trim_text(true);

    let mut batch = PersonBatch::default();
    let mut part_index = 0usize;
    let mut total = 0usize;

    // Per-person accumulator
    let mut in_person = false;
    let mut person_id = String::new();

    // Name fields
    let mut primary_name = String::new();
    let mut primary_name_lang = String::new();
    let mut alt_names: Vec<String> = Vec::new();

    // Simple text accumulators keyed by element name / context
    let mut gender: Option<String> = None;
    let mut dynasty: Option<String> = None;
    let mut birth_raw: Option<String> = None;
    let mut death_raw: Option<String> = None;
    let mut occupation: Option<String> = None;
    let mut place_of_origin: Option<String> = None;
    let mut concise_bio: Option<String> = None;
    let mut wikidata_id: Option<String> = None;
    let mut cbdb_id: Option<String> = None;
    let mut teachers: Vec<String> = Vec::new();
    let mut students: Vec<String> = Vec::new();

    // Element-level state
    #[derive(PartialEq)]
    enum Context {
        None,
        PersName { is_alt: bool, lang: String },
        Birth,
        Death,
        Occupation,
        NoteDynasty,
        NoteConcise,
        NotePlaceOfOrigin,
        IdnoWikidata,
        IdnoCbdb,
    }
    let mut ctx = Context::None;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let ename = e.name();
                let local = local_name(ename.as_ref());
                match local {
                    b"person" => {
                        in_person = true;
                        person_id = attr_val(e, b"xml:id").or_else(|| attr_val(e, b"id")).unwrap_or_default();
                        primary_name.clear();
                        primary_name_lang.clear();
                        alt_names.clear();
                        gender = None;
                        dynasty = None;
                        birth_raw = None;
                        death_raw = None;
                        occupation = None;
                        place_of_origin = None;
                        concise_bio = None;
                        wikidata_id = None;
                        cbdb_id = None;
                        teachers.clear();
                        students.clear();
                        ctx = Context::None;
                    }
                    b"persName" if in_person => {
                        let is_alt = attr_val(e, b"type").as_deref() == Some("alternative");
                        let lang = attr_val(e, b"lang")
                            .or_else(|| attr_val(e, b"xml:lang"))
                            .unwrap_or_default();
                        ctx = Context::PersName { is_alt, lang };
                    }
                    b"sex" if in_person => {
                        gender = attr_val(e, b"value").map(|v| match v.as_str() {
                            "1" => "male".to_string(),
                            "2" => "female".to_string(),
                            other => other.to_string(),
                        });
                    }
                    b"birth" if in_person => {
                        ctx = Context::Birth;
                    }
                    b"death" if in_person => {
                        ctx = Context::Death;
                    }
                    b"occupation" if in_person => {
                        ctx = Context::Occupation;
                    }
                    b"note" if in_person => {
                        match attr_val(e, b"type").as_deref() {
                            Some("dynasty") => ctx = Context::NoteDynasty,
                            Some("concise") => ctx = Context::NoteConcise,
                            Some("placeOfOrigin") => ctx = Context::NotePlaceOfOrigin,
                            _ => ctx = Context::None,
                        }
                    }
                    b"idno" if in_person => {
                        match attr_val(e, b"type").as_deref() {
                            Some("Wikidata") => ctx = Context::IdnoWikidata,
                            Some("CBDB") => ctx = Context::IdnoCbdb,
                            _ => ctx = Context::None,
                        }
                    }
                    b"relation" if in_person => {
                        let rel_type = attr_val(e, b"type").unwrap_or_default();
                        let name = attr_val(e, b"n").unwrap_or_default();
                        if !name.is_empty() {
                            match rel_type.as_str() {
                                "teacher" => teachers.push(name),
                                "student" => students.push(name),
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) => {
                if !in_person {
                    continue;
                }
                let text_owned = String::from_utf8_lossy(e.as_ref()).into_owned();
                let text = text_owned.trim();
                if text.is_empty() {
                    continue;
                }
                match &mut ctx {
                    Context::PersName { is_alt, lang } => {
                        if *is_alt {
                            // Only store CJK alt names for matching
                            if crate::normalize::contains_cjk(text) {
                                alt_names.push(text.to_string());
                            }
                        } else if primary_name.is_empty() {
                            primary_name = text.to_string();
                            primary_name_lang = lang.clone();
                        }
                    }
                    Context::Birth => {
                        if birth_raw.is_none() {
                            birth_raw = Some(extract_year(text));
                        }
                    }
                    Context::Death => {
                        if death_raw.is_none() {
                            death_raw = Some(extract_year(text));
                        }
                    }
                    Context::Occupation => {
                        occupation = Some(text.to_string());
                    }
                    Context::NoteDynasty => {
                        dynasty = Some(text.to_string());
                    }
                    Context::NoteConcise => {
                        // May have multiple text nodes; concatenate
                        if let Some(ref mut bio) = concise_bio {
                            bio.push_str(text);
                        } else {
                            concise_bio = Some(text.to_string());
                        }
                    }
                    Context::NotePlaceOfOrigin => {
                        if place_of_origin.is_none() && crate::normalize::contains_cjk(text) {
                            place_of_origin = Some(text.to_string());
                        }
                    }
                    Context::IdnoWikidata => {
                        wikidata_id = Some(text.to_string());
                    }
                    Context::IdnoCbdb => {
                        cbdb_id = Some(text.to_string());
                    }
                    Context::None => {}
                }
            }
            Ok(Event::End(ref e)) => {
                let ename = e.name();
                let local = local_name(ename.as_ref());
                match local {
                    b"person" if in_person => {
                        in_person = false;
                        if primary_name.is_empty() || person_id.is_empty() {
                            continue;
                        }
                        // Truncate concise bio
                        let bio = concise_bio.as_deref().map(|s| crate::dict::truncate_gloss(s, 800));
                        let alt_json = serde_json::to_string(&alt_names).unwrap_or_default();
                        let teachers_json = serde_json::to_string(&teachers).unwrap_or_default();
                        let students_json = serde_json::to_string(&students).unwrap_or_default();

                        batch.push(
                            person_id.clone(),
                            primary_name.clone(),
                            primary_name_lang.clone(),
                            alt_json,
                            gender.clone(),
                            dynasty.clone(),
                            birth_raw.clone(),
                            death_raw.clone(),
                            occupation.clone(),
                            place_of_origin.clone(),
                            bio,
                            teachers_json,
                            students_json,
                            wikidata_id.clone(),
                            cbdb_id.clone(),
                        );
                        total += 1;

                        if batch.len() >= storage::AUTHORITY_BATCH_SIZE {
                            storage::write_person_parquet(&batch, out_dir, part_index, compression)?;
                            batch.clear();
                            part_index += 1;
                        }

                        ctx = Context::None;
                    }
                    b"persName" | b"birth" | b"death" | b"occupation" | b"note" | b"idno" => {
                        ctx = Context::None;
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow::anyhow!("XML error: {e}")),
            _ => {}
        }
        buf.clear();
    }

    if !batch.is_empty() {
        storage::write_person_parquet(&batch, out_dir, part_index, compression)?;
    }

    Ok(total)
}

// ---------------------------------------------------------------------------
// Place parser
// ---------------------------------------------------------------------------

fn ingest_places(xml_path: &Path, out_dir: &Path, compression: crate::storage::ParquetCompression) -> Result<usize> {
    let data = std::fs::read(xml_path)?;
    let mut reader = Reader::from_reader(data.as_slice());
    reader.config_mut().trim_text(true);

    let mut batch = PlaceBatch::default();
    let mut part_index = 0usize;
    let mut total = 0usize;

    let mut in_place = false;
    let mut place_depth = 0u32; // tracks nested <place> elements inside an authority entry
    let mut place_id = String::new();

    let mut primary_name = String::new();
    let mut primary_name_lang = String::new();
    let mut alt_names: Vec<String> = Vec::new();
    let mut latitude: Option<f64> = None;
    let mut longitude: Option<f64> = None;
    let mut geo_confidence: Option<String> = None;
    let mut district: Option<String> = None;
    let mut category: Option<String> = None;
    let mut description: Option<String> = None;
    let mut parent_place_id: Option<String> = None;

    #[derive(PartialEq)]
    enum PlaceCtx {
        None,
        PlaceName { is_alt: bool, lang: String },
        Geo { cert: Option<String> },
        District,
        NoteCategory,
        NoteGeneral,
    }
    let mut ctx = PlaceCtx::None;
    let mut buf = Vec::new();

    loop {
        let ev = reader.read_event_into(&mut buf);
        // Track whether the current event is a self-closing element: those never emit
        // a corresponding End event, so they must not increment place_depth.
        let is_empty_element = matches!(&ev, Ok(Event::Empty(_)));
        match ev {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let ename = e.name();
                let local = local_name(ename.as_ref());
                match local {
                    b"place" => {
                        if in_place {
                            // Nested <place> inside an authority entry (e.g. inside <location>)
                            // Capture parent place reference from <place key="PLA...">
                            if let Some(key) = attr_val(e, b"key") {
                                if parent_place_id.is_none() {
                                    parent_place_id = Some(key);
                                }
                            }
                            // Only non-self-closing nested places get a depth increment;
                            // Empty elements never emit an End event so must not be counted.
                            if !is_empty_element {
                                place_depth += 1;
                            }
                        } else {
                            // Check if this is an authority entry (xml:id starts with "PL")
                            let id = attr_val(e, b"xml:id")
                                .or_else(|| attr_val(e, b"id"))
                                .unwrap_or_default();
                            if id.starts_with("PL") {
                                in_place = true;
                                place_depth = 0;
                                place_id = id;
                                primary_name.clear();
                                primary_name_lang.clear();
                                alt_names.clear();
                                latitude = None;
                                longitude = None;
                                geo_confidence = None;
                                district = None;
                                category = None;
                                description = None;
                                parent_place_id = None;
                                ctx = PlaceCtx::None;
                            }
                        }
                    }
                    b"placeName" if in_place => {
                        // Ignore nested placeNames inside <note type="placeOfOrigin">
                        let is_alt = attr_val(e, b"type").as_deref() == Some("alternative");
                        let lang = attr_val(e, b"lang")
                            .or_else(|| attr_val(e, b"xml:lang"))
                            .unwrap_or_default();
                        ctx = PlaceCtx::PlaceName { is_alt, lang };
                    }
                    b"geo" if in_place => {
                        let cert = attr_val(e, b"cert");
                        ctx = PlaceCtx::Geo { cert };
                    }
                    b"district" if in_place => {
                        ctx = PlaceCtx::District;
                    }
                    b"note" if in_place => {
                        match attr_val(e, b"type").as_deref() {
                            Some("category") => ctx = PlaceCtx::NoteCategory,
                            None => ctx = PlaceCtx::NoteGeneral,
                            _ => ctx = PlaceCtx::None,
                        }
                    }
                    b"location" if in_place => {
                        // parent reference is extracted from nested <place key=...> inside location
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) => {
                if !in_place {
                    continue;
                }
                let text_owned = String::from_utf8_lossy(e.as_ref()).into_owned();
                let text = text_owned.trim();
                if text.is_empty() {
                    continue;
                }
                match &mut ctx {
                    PlaceCtx::PlaceName { is_alt, lang } => {
                        if *is_alt {
                            if crate::normalize::contains_cjk(text) {
                                alt_names.push(text.to_string());
                            }
                        } else if primary_name.is_empty() {
                            primary_name = text.to_string();
                            primary_name_lang = lang.clone();
                        }
                    }
                    PlaceCtx::Geo { cert } => {
                        // Format is "lon lat" (two floats)
                        let parts: Vec<&str> = text.split_whitespace().collect();
                        if parts.len() == 2 {
                            if let (Ok(lon), Ok(lat)) =
                                (parts[0].parse::<f64>(), parts[1].parse::<f64>())
                            {
                                longitude = Some(lon);
                                latitude = Some(lat);
                                geo_confidence = cert.clone();
                            }
                        }
                    }
                    PlaceCtx::District => {
                        district = Some(text.to_string());
                    }
                    PlaceCtx::NoteCategory => {
                        category = Some(text.to_string());
                    }
                    PlaceCtx::NoteGeneral => {
                        if description.is_none() {
                            description = Some(crate::dict::truncate_gloss(text, 600));
                        }
                    }
                    PlaceCtx::None => {}
                }
            }
            Ok(Event::End(ref e)) => {
                let ename = e.name();
                let local = local_name(ename.as_ref());
                match local {
                    b"place" if in_place => {
                        if place_depth > 0 {
                            place_depth -= 1;
                            continue;
                        }
                        in_place = false;
                        if primary_name.is_empty() || place_id.is_empty() {
                            ctx = PlaceCtx::None;
                            continue;
                        }
                        let alt_json = serde_json::to_string(&alt_names).unwrap_or_default();
                        batch.push(
                            place_id.clone(),
                            primary_name.clone(),
                            primary_name_lang.clone(),
                            alt_json,
                            latitude,
                            longitude,
                            geo_confidence.clone(),
                            district.clone(),
                            category.clone(),
                            description.clone(),
                            parent_place_id.clone(),
                        );
                        total += 1;

                        if batch.len() >= storage::AUTHORITY_BATCH_SIZE {
                            storage::write_place_parquet(&batch, out_dir, part_index, compression)?;
                            batch.clear();
                            part_index += 1;
                        }
                        ctx = PlaceCtx::None;
                    }
                    b"placeName" | b"geo" | b"district" | b"note" => {
                        ctx = PlaceCtx::None;
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow::anyhow!("XML error: {e}")),
            _ => {}
        }
        buf.clear();
    }

    if !batch.is_empty() {
        storage::write_place_parquet(&batch, out_dir, part_index, compression)?;
    }

    Ok(total)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn local_name(qname: &[u8]) -> &[u8] {
    if let Some(pos) = qname.iter().rposition(|&b| b == b':') {
        &qname[pos + 1..]
    } else {
        qname
    }
}

fn attr_val(e: &quick_xml::events::BytesStart, name: &[u8]) -> Option<String> {
    for attr in e.attributes().flatten() {
        let key = attr.key.as_ref();
        // Match "name" or "prefix:name"
        if key == name || key.ends_with(name) && key.len() > name.len() && key[key.len() - name.len() - 1] == b':' {
            return String::from_utf8(attr.value.to_vec()).ok();
        }
    }
    None
}

/// Extract a 4-digit year from a date string like "+0602-02-01 ~ +0603-02-18".
/// BCE dates are represented as negative years, e.g. "-0563" → "-0563".
fn extract_year(s: &str) -> String {
    let s = s.trim();
    // Strip the leading sign, preserving BCE (-) vs CE (+).
    let (bce, s) = if let Some(rest) = s.strip_prefix('-') {
        (true, rest)
    } else {
        (false, s.strip_prefix('+').unwrap_or(s))
    };
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).take(4).collect();
    let year = if digits.len() == 4 {
        digits
    } else {
        s.split_whitespace().next().unwrap_or("").to_string()
    };
    if bce { format!("-{year}") } else { year }
}

