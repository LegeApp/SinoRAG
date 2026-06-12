//! Bilingual PDF generation with alternating Chinese/English paragraphs
//! 
//! Creates high-quality PDFs with professional typography, alternating paragraph layout,
//! and hOCR layers for text accessibility.

use anyhow::{Result, anyhow};
use crate::fonts::FontContext;
use crate::typography::{TextLayoutEngine, FormattedParagraph, FormattedLine};
use crate::hocr_layer::{HocrGenerator, HocrPage};
use lopdf::{
    Document, Object, Dictionary, Stream, StringFormat,
    content::{Content, Operation},
    ObjectId,
};
use std::collections::{BTreeSet, HashMap};
use std::io::Write;
use fontdue::Font;
use subsetter::GlyphRemapper;
use crate::markdown::{parse_markdown, Align, InlineStyle, MdBlock, Span};

/// Which embedded font face a glyph should be drawn with.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MdFont {
    Chinese,
    English,
    Mono,
}

impl MdFont {
    fn resource(self) -> &'static str {
        match self {
            MdFont::Chinese => "chinese",
            MdFont::English => "english",
            MdFont::Mono => "mono",
        }
    }
}

/// A positioned, single-face run on a laid-out Markdown line.
struct MdRun {
    x: f32,
    width: f32,
    text: String,
    font: MdFont,
    size: f32,
    style: InlineStyle,
}

/// A laid-out line of inline content (x positions relative to the block's left).
struct MdLine {
    runs: Vec<MdRun>,
    /// Vertical advance to the next line's top.
    height: f32,
    /// Baseline offset from the line's top.
    ascent: f32,
}

/// Intermediate token used while wrapping inline content into lines.
struct MdTok {
    text: String,
    font: MdFont,
    size: f32,
    style: InlineStyle,
    width: f32,
    is_space: bool,
    breakable_before: bool,
    force_break: bool,
}

/// Pagination state for a Markdown render pass.
struct MdCtx {
    content: Content,
    pages: Vec<Content>,
    /// Current top-down y cursor (distance from page top).
    y: f32,
    top: f32,
    bottom: f32,
    /// Characters actually drawn per face, for font subsetting.
    used_chinese: BTreeSet<char>,
    used_english: BTreeSet<char>,
    used_mono: BTreeSet<char>,
}

impl MdCtx {
    fn ops(&mut self) -> &mut Vec<Operation> {
        &mut self.content.operations
    }

    /// Record the glyphs drawn so the matching font can be subset to them.
    fn record(&mut self, font: MdFont, text: &str) {
        let set = match font {
            MdFont::Chinese => &mut self.used_chinese,
            MdFont::English => &mut self.used_english,
            MdFont::Mono => &mut self.used_mono,
        };
        set.extend(text.chars());
    }

    fn page_break(&mut self) {
        let done = std::mem::replace(
            &mut self.content,
            Content {
                operations: Vec::new(),
            },
        );
        self.pages.push(done);
        self.y = self.top;
    }
}

/// Subset a font to the glyphs needed for `used`, returning the new font bytes
/// and the old->new glyph id remapping. Returns `None` if subsetting fails.
fn build_font_subset(
    face: &Font,
    data: &[u8],
    used: &BTreeSet<char>,
) -> Option<(Vec<u8>, GlyphRemapper)> {
    let mut remapper = GlyphRemapper::new(); // always includes .notdef (gid 0)
    for &ch in used {
        remapper.remap(face.lookup_glyph_index(ch));
    }
    match subsetter::subset(data, 0, &remapper) {
        Ok(bytes) => Some((bytes, remapper)),
        Err(e) => {
            log::warn!("font subsetting failed, embedding full font: {e}");
            None
        }
    }
}

/// Full BMP CID->GID map (2 bytes/CID); CID equals the UTF-16 BMP code unit.
fn full_cid_to_gid_map(face: &Font) -> Vec<u8> {
    let mut map = vec![0u8; 65536 * 2];
    for cid in 0u32..=0xFFFF {
        if let Some(ch) = char::from_u32(cid) {
            let gid = face.lookup_glyph_index(ch);
            let off = (cid as usize) * 2;
            map[off] = (gid >> 8) as u8;
            map[off + 1] = (gid & 0xFF) as u8;
        }
    }
    map
}

/// CID->GID map pointing at the *subset's* compact glyph ids.
fn subset_cid_to_gid_map(face: &Font, remapper: &GlyphRemapper) -> Vec<u8> {
    let mut map = vec![0u8; 65536 * 2];
    for cid in 0u32..=0xFFFF {
        if let Some(ch) = char::from_u32(cid) {
            if let Some(new_gid) = remapper.get(face.lookup_glyph_index(ch)) {
                let off = (cid as usize) * 2;
                map[off] = (new_gid >> 8) as u8;
                map[off + 1] = (new_gid & 0xFF) as u8;
            }
        }
    }
    map
}

/// Deterministic 6-uppercase-letter PDF subset tag derived from the glyph set.
fn subset_tag(used: &BTreeSet<char>) -> String {
    let mut h: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
    for &ch in used {
        h ^= ch as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    let mut s = String::with_capacity(6);
    for i in 0..6 {
        let v = ((h >> (i * 5)) & 0x1F) as u8;
        s.push((b'A' + (v % 26)) as char);
    }
    s
}

/// True for characters that should be rendered with the CJK font.
fn is_cjk(ch: char) -> bool {
    use unicode_script::{Script, UnicodeScript};
    matches!(
        ch.script(),
        Script::Han | Script::Hiragana | Script::Katakana | Script::Bopomofo
    ) || ('\u{3000}'..='\u{303F}').contains(&ch) // CJK symbols & punctuation
        || ('\u{FF00}'..='\u{FFEF}').contains(&ch) // fullwidth/halfwidth forms
}

/// Bilingual PDF generator
pub struct BilingualPdfGenerator {
    font_context: FontContext,
    layout_engine: TextLayoutEngine,
    hocr_generator: HocrGenerator,
    document: Document,
    font_objects: HashMap<String, ObjectId>,
    /// Embedded image XObjects, keyed by PDF resource name (e.g. "Im0").
    xobjects: HashMap<String, ObjectId>,
    pages_id: ObjectId,
}

impl BilingualPdfGenerator {
    // Extra inner guard so text never hugs or clips at page edges.
    const SAFE_INSET_X: f32 = 10.0;
    const SAFE_INSET_Y: f32 = 10.0;

    fn safe_content_area(&self) -> (f32, f32, f32, f32) {
        let (x, y, w, h) = self.font_context.content_area();
        let safe_x = x + Self::SAFE_INSET_X;
        let safe_y = y + Self::SAFE_INSET_Y;
        let safe_w = (w - 2.0 * Self::SAFE_INSET_X).max(120.0);
        let safe_h = (h - 2.0 * Self::SAFE_INSET_Y).max(120.0);
        (safe_x, safe_y, safe_w, safe_h)
    }

    /// Create a new bilingual PDF generator
    pub fn new(font_context: FontContext) -> Self {
        let layout_engine = crate::typography::create_layout_engine(font_context.clone());
        let hocr_generator = crate::hocr_layer::create_hocr_generator();
        
        Self {
            font_context,
            layout_engine,
            hocr_generator,
            document: Document::new(),
            font_objects: HashMap::new(),
            xobjects: HashMap::new(),
            pages_id: (0, 0), // Will be set properly in initialize_document
        }
    }
    
    /// Generate a bilingual PDF with alternating paragraphs
    pub fn generate_bilingual_pdf(
        &mut self,
        chinese_sections: &[String],
        english_sections: &[String],
        output_path: &str,
    ) -> Result<()> {
        if chinese_sections.len() != english_sections.len() {
            return Err(anyhow!(
                "Chinese and English sections must have the same length: {} vs {}",
                chinese_sections.len(),
                english_sections.len()
            ));
        }
        
        // Initialize PDF document
        self.initialize_document()?;
        
        // Create alternating paragraph layout
        let all_paragraphs = self.create_alternating_layout(chinese_sections, english_sections)?;
        
        // Generate pages
        println!("DEBUG: About to create pages from {} paragraphs", all_paragraphs.len());
        let pages = self.create_pages_from_paragraphs(all_paragraphs)?;
        println!("DEBUG: Created {} pages", pages.len());

        // Add hOCR layer
        self.add_hocr_layer(&pages)?;

        // Save document
        println!("DEBUG: Document has {} objects, pages_id: {:?}, {} actual pages in Kids array", 
                 self.document.objects.len(), 
                 self.pages_id,
                 self.get_page_count()?);
        self.save_document(output_path)?;

        Ok(())
    }

    /// Generate a bilingual PDF with true side-by-side columns.
    pub fn generate_bilingual_pdf_side_by_side(
        &mut self,
        chinese_sections: &[String],
        english_sections: &[String],
        output_path: &str,
    ) -> Result<()> {
        if chinese_sections.len() != english_sections.len() {
            return Err(anyhow!(
                "Chinese and English sections must have the same length: {} vs {}",
                chinese_sections.len(),
                english_sections.len()
            ));
        }

        self.initialize_document()?;
        let pages = self.create_pages_side_by_side(chinese_sections, english_sections)?;
        self.add_hocr_layer(&pages)?;
        self.save_document(output_path)?;
        Ok(())
    }
    
    /// Initialize PDF document with fonts and metadata
    fn initialize_document(&mut self) -> Result<()> {
        // Add fonts to document (full embedding for the bilingual pipeline).
        let _chinese_id = self.add_font_to_document("chinese")?;
        let _english_id = self.add_font_to_document("english")?;

        self.init_document_structure()
    }

    /// Create the page tree, info, and catalog (no fonts). Used by paths that
    /// add fonts after layout, e.g. Markdown subsetting.
    fn init_document_structure(&mut self) -> Result<()> {
        // Create pages structure
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Kids", Object::Array(vec![])); // Will be populated later
        pages_dict.set("Count", Object::Integer(0)); // Initialize count to 0
        let pages_id = self.document.add_object(Object::Dictionary(pages_dict));

        // Create info dictionary
        let mut info_dict = Dictionary::new();
        info_dict.set("Producer", Object::string_literal("CBETA Bilingual PDF Creator"));
        info_dict.set("Creator", Object::string_literal("CBETA Project"));
        let info_id = self.document.add_object(Object::Dictionary(info_dict));

        // Create catalog with required root and metadata
        let mut catalog_dict = Dictionary::new();
        catalog_dict.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog_dict.set("Pages", Object::Reference(pages_id));
        
        let catalog_id = self.document.add_object(Object::Dictionary(catalog_dict));

        // Set catalog as root and info in trailer
        self.document.trailer.set(b"Root", Object::Reference(catalog_id));
        self.document.trailer.set(b"Info", Object::Reference(info_id));

        // Store pages ID for later use
        self.pages_id = pages_id;

        Ok(())
    }
    
    /// Create alternating layout: Chinese #1 → English #1 → Chinese #2 → English #2
    fn create_alternating_layout(
        &mut self,
        chinese_sections: &[String],
        english_sections: &[String],
    ) -> Result<Vec<FormattedParagraph>> {
        let mut all_paragraphs = Vec::new();
        
        let (content_x, content_y, content_width, _content_height) = self.safe_content_area();
        let mut current_y = content_y;
        
        // Add spacing between alternating paragraphs
        let paragraph_spacing = self.font_context.get_line_height(true) * 0.5;
        
        for (index, (chinese_text, english_text)) in chinese_sections.iter().zip(english_sections.iter()).enumerate() {
            // Add Chinese paragraph
            if !chinese_text.trim().is_empty() {
                let chinese_para = self.layout_engine.layout_paragraph(
                    chinese_text,
                    content_x,
                    current_y,
                    content_width,
                    true, // is_chinese
                )?;
                
                current_y += chinese_para.height + paragraph_spacing;
                all_paragraphs.push(chinese_para);
            }
            
            // Add English paragraph
            if !english_text.trim().is_empty() {
                let english_para = self.layout_engine.layout_paragraph(
                    english_text,
                    content_x,
                    current_y,
                    content_width,
                    false, // is_chinese
                )?;
                
                current_y += english_para.height + paragraph_spacing;
                all_paragraphs.push(english_para);
            }
            
            // Add section separator (optional)
            if index < chinese_sections.len() - 1 {
                current_y += paragraph_spacing * 2.0; // Extra space between sections
            }
        }
        
        Ok(all_paragraphs)
    }
    
    /// Create PDF pages from formatted paragraphs
    fn create_pages_from_paragraphs(&mut self, paragraphs: Vec<FormattedParagraph>) -> Result<Vec<HocrPage>> {
        let mut pages = Vec::new();
        let (_content_x, _content_y, _content_width, content_height) = self.safe_content_area();
        
        // Group paragraphs into pages
        let mut current_page_paragraphs = Vec::new();
        let mut current_page_height = 0.0;
        
        for paragraph in &paragraphs {
            if current_page_height + paragraph.height > content_height && !current_page_paragraphs.is_empty() {
                // Current page is full, create it
                let page = self.create_single_page(&current_page_paragraphs)?;
                pages.push(page);
                
                // Start new page
                current_page_paragraphs.clear();
                current_page_height = 0.0;
            }
            
            current_page_paragraphs.push(paragraph.clone());
            current_page_height += paragraph.height;
        }
        
        // Create the last page if there are remaining paragraphs
        if !current_page_paragraphs.is_empty() {
            let page = self.create_single_page(&current_page_paragraphs)?;
            pages.push(page);
        }
        
        Ok(pages)
    }

    /// Create pages for side-by-side layout (left column Chinese, right column English).
    fn create_pages_side_by_side(
        &mut self,
        chinese_sections: &[String],
        english_sections: &[String],
    ) -> Result<Vec<HocrPage>> {
        let mut pages = Vec::new();
        let (content_x, content_y, content_width, content_height) = self.safe_content_area();
        let page_bottom = content_y + content_height;

        let column_gutter = 24.0_f32;
        let column_width = ((content_width - column_gutter) / 2.0).max(120.0);
        let right_column_x = content_x + column_width + column_gutter;
        let row_spacing = (self.font_context.get_line_height(true) * self.font_context.paragraph_spacing.max(0.2))
            .max(4.0);

        let mut current_y = content_y;
        let mut current_page_paragraphs: Vec<FormattedParagraph> = Vec::new();

        for (zh_text, en_text) in chinese_sections.iter().zip(english_sections.iter()) {
            let (mut zh_para, mut en_para, mut row_height) = self.layout_side_by_side_row(
                zh_text,
                en_text,
                content_x,
                right_column_x,
                current_y,
                column_width,
            )?;

            if current_y + row_height > page_bottom && !current_page_paragraphs.is_empty() {
                let page = self.create_single_page(&current_page_paragraphs)?;
                pages.push(page);
                current_page_paragraphs.clear();
                current_y = content_y;

                let relaid = self.layout_side_by_side_row(
                    zh_text,
                    en_text,
                    content_x,
                    right_column_x,
                    current_y,
                    column_width,
                )?;
                zh_para = relaid.0;
                en_para = relaid.1;
                row_height = relaid.2;
            }

            if let Some(p) = zh_para {
                current_page_paragraphs.push(p);
            }
            if let Some(p) = en_para {
                current_page_paragraphs.push(p);
            }

            current_y += row_height + row_spacing;
        }

        if !current_page_paragraphs.is_empty() {
            let page = self.create_single_page(&current_page_paragraphs)?;
            pages.push(page);
        }

        Ok(pages)
    }

    fn layout_side_by_side_row(
        &mut self,
        zh_text: &str,
        en_text: &str,
        left_x: f32,
        right_x: f32,
        row_y: f32,
        column_width: f32,
    ) -> Result<(Option<FormattedParagraph>, Option<FormattedParagraph>, f32)> {
        let zh_para = if zh_text.trim().is_empty() {
            None
        } else {
            Some(self.layout_engine.layout_paragraph(
                zh_text,
                left_x,
                row_y,
                column_width,
                true,
            )?)
        };

        let en_para = if en_text.trim().is_empty() {
            None
        } else {
            Some(self.layout_engine.layout_paragraph(
                en_text,
                right_x,
                row_y,
                column_width,
                false,
            )?)
        };

        let row_height = zh_para.as_ref().map(|p| p.height).unwrap_or(0.0)
            .max(en_para.as_ref().map(|p| p.height).unwrap_or(0.0))
            .max(self.font_context.get_line_height(true));

        Ok((zh_para, en_para, row_height))
    }
    
    /// Create a single PDF page from paragraphs
    fn create_single_page(&mut self, paragraphs: &[FormattedParagraph]) -> Result<HocrPage> {
        let page_id = self.document.new_object_id();
        
        // Create page content
        let mut content = Content {
            operations: Vec::new(),
        };
        
        // Add each paragraph to the page content
        for paragraph in paragraphs {
            self.add_paragraph_to_content(&mut content, paragraph)?;
        }
        
        // Create page object
        let mut page_dict = Dictionary::new();
        page_dict.set("Type", Object::Name(b"Page".to_vec()));
        page_dict.set("Parent", Object::Reference(self.pages_id)); // Reference to pages object
        page_dict.set("Resources", self.create_resources_dict()?);
        page_dict.set("MediaBox", Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Real(self.font_context.page_width),
            Object::Real(self.font_context.page_height),
        ]));

        let content_stream = Stream::new(Dictionary::new(), content.encode()?);
        let content_id = self.document.add_object(content_stream);
        page_dict.set("Contents", Object::Reference(content_id));

        // Add page to document
        self.document.objects.insert(page_id, Object::Dictionary(page_dict));

        // Add to pages tree
        self.add_page_to_tree(page_id)?;
        
        // Generate hOCR for this page
        let hocr_page = self.hocr_generator.generate_hocr(paragraphs)?;
        
        Ok(hocr_page)
    }
    
    /// Add a paragraph to the page content with professional typography
    fn add_paragraph_to_content(&mut self, content: &mut Content, paragraph: &FormattedParagraph) -> Result<()> {
        let font_name = if paragraph.is_chinese { "chinese" } else { "english" };
        let font_size = paragraph.font_size;

        // Clip only by column width (full page height), so text never bleeds across columns
        // while avoiding vertical clipping artifacts on glyph ascenders/descenders.
        content.operations.push(Operation::new("q", vec![]));
        content.operations.push(Operation::new("re", vec![
            Object::Real(paragraph.x),
            Object::Real(0.0),
            Object::Real(paragraph.width),
            Object::Real(self.font_context.page_height),
        ]));
        content.operations.push(Operation::new("W", vec![]));
        content.operations.push(Operation::new("n", vec![]));

        content.operations.push(Operation::new("BT", vec![]));
        content.operations.push(Operation::new("Tf", vec![
            Object::Name(font_name.as_bytes().to_vec()),
            Object::Real(font_size),
        ]));

        for line in &paragraph.lines {
            // Use baseline positioning for proper leading
            let pdf_y = self.font_context.page_height - (paragraph.y + line.baseline);

            content.operations.push(Operation::new("Tm", vec![
                Object::Real(1.0), Object::Real(0.0),
                Object::Real(0.0), Object::Real(1.0),
                Object::Real(paragraph.x + line.x), Object::Real(pdf_y),
            ]));

            // Build TJ array with kerning + tracking
            let tj = self.build_tj_array(line)?;
            content.operations.push(Operation::new("TJ", vec![Object::Array(tj)]));
        }

        content.operations.push(Operation::new("ET", vec![]));
        content.operations.push(Operation::new("Q", vec![]));
        Ok(())
    }

    /// Build TJ array with proper Chinese character handling
    fn build_tj_array(&mut self, line: &FormattedLine) -> Result<Vec<Object>> {
        let font = if line.is_chinese { 
            self.font_context.chinese_font.clone() 
        } else { 
            self.font_context.english_font.clone() 
        };
        let size = line.font_size;
        let tracking = if line.is_chinese { self.font_context.tracking_chinese } else { self.font_context.tracking_english };
        let _scale = size / font.units_per_em() as f32;

        let chars: Vec<char> = line.text.chars().collect();
        let mut tj = Vec::with_capacity(chars.len() * 2);

        // Create a map of space positions to adjustments
        let mut space_adjustments_map = std::collections::HashMap::new();
        for adj in &line.space_adjustments {
            space_adjustments_map.insert(adj.position, adj.adjustment_ratio);
        }

        let mut char_position = 0;
        for (i, &ch) in chars.iter().enumerate() {
            // Use UTF-16BE hex strings for both Chinese and English Type0 fonts.
            let mut utf16be = Vec::new();
            for unit in ch.encode_utf16(&mut [0; 2]).iter().copied() {
                utf16be.extend_from_slice(&unit.to_be_bytes());
            }
            tj.push(Object::String(utf16be, StringFormat::Hexadecimal));

            if i < chars.len() - 1 {
                let mut adjust: f32 = 0.0;

                // Kerning
                if let Some(kern) = font.horizontal_kern(ch, chars[i + 1], size) {
                    adjust += kern * 1000.0 / font.units_per_em() as f32;
                }

                // Tracking
                adjust += tracking;

                // Apply space adjustment if this is a space character
                if ch == ' ' {
                    if let Some(&ratio) = space_adjustments_map.get(&char_position) {
                        // Convert adjustment ratio to thousandths of em
                        let space_width = self.font_context.calculate_text_width(" ", line.is_chinese);
                        let extra_space = space_width * ratio;
                        adjust += extra_space * 1000.0 / size;
                    }
                }

                tj.push(Object::Real(adjust));
            }

            // Update character position for space tracking
            if ch != ' ' {
                char_position += 1;
            }
        }
        Ok(tj)
    }

    /// Create resources dictionary for fonts
    fn create_resources_dict(&self) -> Result<Object> {
        let mut resources = Dictionary::new();
        let mut font_dict = Dictionary::new();

        for (font_name, &font_id) in &self.font_objects {
            font_dict.set(font_name.as_str(), Object::Reference(font_id));
        }

        resources.set("Font", Object::Dictionary(font_dict));

        if !self.xobjects.is_empty() {
            let mut xobject_dict = Dictionary::new();
            for (name, &id) in &self.xobjects {
                xobject_dict.set(name.as_str(), Object::Reference(id));
            }
            resources.set("XObject", Object::Dictionary(xobject_dict));
        }

        Ok(Object::Dictionary(resources))
    }
    
    /// Add a fully-embedded font (no subsetting).
    fn add_font_to_document(&mut self, name: &str) -> Result<ObjectId> {
        self.add_font_to_document_impl(name, None)
    }

    /// Add a font embedded with only the glyphs in `used`.
    fn add_font_to_document_subset(
        &mut self,
        name: &str,
        used: &BTreeSet<char>,
    ) -> Result<ObjectId> {
        self.add_font_to_document_impl(name, Some(used))
    }

    /// Add font to document with composite Type0 + CIDFont support, optionally
    /// subsetting the embedded program to `subset`.
    fn add_font_to_document_impl(
        &mut self,
        name: &str,
        subset: Option<&BTreeSet<char>>,
    ) -> Result<ObjectId> {
        let (base_font_name, font_path, full_data, is_chinese_font) = match name {
            "chinese" => (
                self.chinese_pdf_font_name(),
                self.font_context.chinese_font_path.clone(),
                self.font_context.chinese_font_data.clone(),
                true,
            ),
            "mono" => (
                self.mono_pdf_font_name(),
                self.font_context.mono_font_path.clone(),
                self.font_context.mono_font_data.clone(),
                false,
            ),
            _ => (
                self.english_pdf_font_name(),
                self.font_context.english_font_path.clone(),
                self.font_context.english_font_data.clone(),
                false,
            ),
        };

        // Phase 1 (read-only): compute the bytes to embed and the CID->GID map,
        // optionally subsetting. Done before any &mut self.document calls so the
        // immutable borrow of the font face is released first.
        let (embed_data, cid_map_bytes, pdf_font_name) = {
            let face: &Font = match name {
                "chinese" => &self.font_context.chinese_font,
                "mono" => &self.font_context.mono_font,
                _ => &self.font_context.english_font,
            };
            match subset {
                Some(used) => match build_font_subset(face, &full_data, used) {
                    Some((sub, remapper)) => (
                        sub,
                        subset_cid_to_gid_map(face, &remapper),
                        format!("{}+{}", subset_tag(used), base_font_name),
                    ),
                    None => (full_data, full_cid_to_gid_map(face), base_font_name.clone()),
                },
                None => (full_data, full_cid_to_gid_map(face), base_font_name.clone()),
            }
        };

        // Phase 2: build the font object graph.
        let mut font_descriptor = Dictionary::new();
        font_descriptor.set("Type", Object::Name(b"FontDescriptor".to_vec()));
        font_descriptor.set("FontName", Object::Name(pdf_font_name.clone().into_bytes()));
        font_descriptor.set("Flags", Object::Integer(if is_chinese_font { 4 } else { 32 }));
        font_descriptor.set("FontBBox", Object::Array(vec![
            Object::Integer(-200),
            Object::Integer(-300),
            Object::Integer(1400),
            Object::Integer(1100),
        ]));
        font_descriptor.set("ItalicAngle", Object::Integer(0));
        font_descriptor.set("Ascent", Object::Integer(880));
        font_descriptor.set("Descent", Object::Integer(-220));
        font_descriptor.set("CapHeight", Object::Integer(700));
        font_descriptor.set("StemV", Object::Integer(80));

        let mut is_embedded = false;
        if let Some((font_file_key, font_file_obj)) =
            self.create_embeddable_font_stream(&font_path, &embed_data)
        {
            let font_stream_id = self.document.add_object(font_file_obj);
            font_descriptor.set(font_file_key.as_str(), Object::Reference(font_stream_id));
            is_embedded = true;
        }
        let font_descriptor_id = self.document.add_object(Object::Dictionary(font_descriptor));

        let mut cidfont = Dictionary::new();
        cidfont.set("Type", Object::Name(b"Font".to_vec()));
        cidfont.set("Subtype", Object::Name(b"CIDFontType2".to_vec()));
        cidfont.set("BaseFont", Object::Name(pdf_font_name.clone().into_bytes()));
        cidfont.set("CIDSystemInfo", Object::Dictionary({
            let mut d = Dictionary::new();
            d.set("Registry", Object::string_literal("Adobe"));
            d.set("Ordering", Object::string_literal("Identity"));
            d.set("Supplement", Object::Integer(0));
            d
        }));
        cidfont.set("FontDescriptor", Object::Reference(font_descriptor_id));
        cidfont.set("DW", Object::Integer(1000));
        if is_embedded {
            let map_stream = Object::Stream(Stream::new(Dictionary::new(), cid_map_bytes));
            let cid_to_gid_map_id = self.document.add_object(map_stream);
            cidfont.set("CIDToGIDMap", Object::Reference(cid_to_gid_map_id));
        } else {
            cidfont.set("CIDToGIDMap", Object::Name(b"Identity".to_vec()));
        }
        let cidfont_id = self.document.add_object(Object::Dictionary(cidfont));

        let tounicode_id = self.document.add_object(self.create_identity_tounicode_cmap_stream());

        let mut type0 = Dictionary::new();
        type0.set("Type", Object::Name(b"Font".to_vec()));
        type0.set("Subtype", Object::Name(b"Type0".to_vec()));
        type0.set("BaseFont", Object::Name(pdf_font_name.into_bytes()));
        type0.set("Encoding", Object::Name(b"Identity-H".to_vec()));
        type0.set("DescendantFonts", Object::Array(vec![Object::Reference(cidfont_id)]));
        type0.set("ToUnicode", Object::Reference(tounicode_id));

        let font_id = self.document.add_object(Object::Dictionary(type0));
        self.font_objects.insert(name.to_string(), font_id);
        Ok(font_id)
    }

    fn sanitize_pdf_font_name(&self, raw: &str) -> String {
        let mut out = String::with_capacity(raw.len());
        for ch in raw.chars() {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                out.push(ch);
            } else if ch.is_whitespace() {
                out.push('-');
            }
        }
        if out.is_empty() {
            "CJKFont".to_string()
        } else {
            out
        }
    }

    fn chinese_pdf_font_name(&self) -> String {
        match self.font_context.chinese_font_name.as_str() {
            "Source Han Serif TC" => "SourceHanSerifTC-Regular".to_string(),
            "Noto Serif CJK TC" => "NotoSerifCJKTC-Regular".to_string(),
            "Microsoft JhengHei" => "MicrosoftJhengHei".to_string(),
            "Microsoft YaHei" => "MicrosoftYaHei".to_string(),
            "SimSun" => "SimSun".to_string(),
            "SimSun Bold" => "SimSun-Bold".to_string(),
            _ => self.sanitize_pdf_font_name(&self.font_context.chinese_font_name),
        }
    }

    fn english_pdf_font_name(&self) -> String {
        match self.font_context.english_font_name.as_str() {
            "EB Garamond" => "EBGaramond-Regular".to_string(),
            "Noto Serif" => "NotoSerif-Regular".to_string(),
            "Liberation Serif" => "LiberationSerif-Regular".to_string(),
            "DejaVu Serif" => "DejaVuSerif".to_string(),
            _ => self.sanitize_pdf_font_name(&self.font_context.english_font_name),
        }
    }

    fn mono_pdf_font_name(&self) -> String {
        // Suffix avoids colliding with the English BaseFont when the mono font
        // fell back to the same file.
        format!(
            "{}-Mono",
            self.sanitize_pdf_font_name(&self.font_context.mono_font_name)
        )
    }

    fn create_identity_tounicode_cmap_stream(&self) -> Object {
        let cmap = b"/CIDInit /ProcSet findresource begin
12 dict begin
begincmap
/CIDSystemInfo
<< /Registry (Adobe)
/Ordering (UCS)
/Supplement 0
>> def
/CMapName /Adobe-Identity-UCS def
/CMapType 2 def
1 begincodespacerange
<0000> <FFFF>
endcodespacerange
1 beginbfrange
<0000> <FFFF> <0000>
endbfrange
endcmap
CMapName currentdict /CMap defineresource pop
end
end"
        .to_vec();
        Object::Stream(Stream::new(Dictionary::new(), cmap))
    }

    /// Build a full BMP CID -> glyph index map (2 bytes per CID) for a font.
    /// CID code equals the UTF-16 BMP code unit in our content stream.
    fn create_cid_to_gid_map_stream(&self, font: &fontdue::Font) -> Object {
        let mut map = vec![0u8; 65536 * 2];
        for cid in 0u32..=0xFFFF {
            if let Some(ch) = char::from_u32(cid) {
                let gid = font.lookup_glyph_index(ch);
                let offset = (cid as usize) * 2;
                map[offset] = (gid >> 8) as u8;
                map[offset + 1] = (gid & 0xFF) as u8;
            }
        }
        Object::Stream(Stream::new(Dictionary::new(), map))
    }

    fn create_embeddable_font_stream(&self, font_path: &str, font_data: &[u8]) -> Option<(String, Object)> {
        if font_data.is_empty() {
            return None;
        }

        let path = font_path.to_ascii_lowercase();
        let mut stream_dict = Dictionary::new();
        stream_dict.set("Length1", Object::Integer(font_data.len() as i64));

        if path.ends_with(".ttf") {
            return Some((
                "FontFile2".to_string(),
                Object::Stream(Stream::new(stream_dict, font_data.to_vec())),
            ));
        }

        if path.ends_with(".otf") {
            stream_dict.set("Subtype", Object::Name(b"OpenType".to_vec()));
            return Some((
                "FontFile3".to_string(),
                Object::Stream(Stream::new(stream_dict, font_data.to_vec())),
            ));
        }

        // TTC collections are not embedded yet in this pipeline.
        None
    }
    
    /// Add page to pages tree
    fn add_page_to_tree(&mut self, page_id: ObjectId) -> Result<()> {
        // Get the current count of kids to update the Count field
        let current_kids_count = {
            let pages_obj = self.document.get_object(self.pages_id)?;
            if let Object::Dictionary(ref pages_dict) = pages_obj {
                let kids = pages_dict.get(b"Kids")?.as_array()?;
                kids.len() + 1  // Adding one new page
            } else {
                return Err(anyhow!("Pages object is not a dictionary"));
            }
        };
        
        // Now update both Kids and Count
        let pages_obj = self.document.get_object_mut(self.pages_id)?;
        if let Object::Dictionary(ref mut pages_dict) = pages_obj {
            let kids = pages_dict.get_mut(b"Kids")?.as_array_mut()?;
            kids.push(Object::Reference(page_id));
            // Update the Count field to reflect the number of pages
            pages_dict.set("Count", Object::Integer(current_kids_count as i64));
        } else {
            return Err(anyhow!("Pages object is not a dictionary"));
        }

        Ok(())
    }
    
    /// Add hOCR layer to the document
    fn add_hocr_layer(&mut self, pages: &[HocrPage]) -> Result<()> {
        // Generate hOCR HTML
        let _hocr_html = self.hocr_generator.generate_hocr_html(pages)?;
        
        // Add hOCR as attachment or separate layer
        // This would be implemented with PDF annotations or attachments
        
        Ok(())
    }
    
    /// Save the document to file
    fn save_document(&mut self, output_path: &str) -> Result<()> {
        // Ensure the document is properly structured before saving
        self.document.compress();
        self.document.save(output_path)?;
        Ok(())
    }
    
    /// Helper function to get the page count from the Kids array
    fn get_page_count(&self) -> Result<usize> {
        let pages_obj = self.document.get_object(self.pages_id)?;
        if let Object::Dictionary(ref pages_dict) = pages_obj {
            let kids = pages_dict.get(b"Kids")?.as_array()?;
            Ok(kids.len())
        } else {
            Ok(0)
        }
    }
}

// ===========================================================================
// Markdown -> PDF rendering
//
// Reuses the document/font machinery above (Type0 CID embedding, page tree)
// and lays out a styled block model on top of the existing CJK-aware fonts.
// ===========================================================================

impl BilingualPdfGenerator {
    // Layout proportions, derived from the base English size so callers can
    // rescale the whole document by adjusting font_size_english.
    fn md_base(&self) -> f32 {
        self.font_context.font_size_english
    }

    fn md_heading_size(&self, level: u8) -> f32 {
        let base = self.md_base();
        let ratio = match level {
            1 => 2.0,
            2 => 1.6,
            3 => 1.333,
            4 => 1.15,
            5 => 1.0,
            _ => 0.92,
        };
        base * ratio
    }

    fn md_line_height(&self, size: f32) -> f32 {
        size * 1.34
    }

    fn md_ascent(&self, size: f32) -> f32 {
        size * 0.82
    }

    fn md_font_ref(&self, f: MdFont) -> &fontdue::Font {
        match f {
            MdFont::Chinese => &self.font_context.chinese_font,
            MdFont::English => &self.font_context.english_font,
            MdFont::Mono => &self.font_context.mono_font,
        }
    }

    fn md_glyph_advance(&self, ch: char, f: MdFont, size: f32) -> f32 {
        self.md_font_ref(f).metrics(ch, size).advance_width
    }

    /// Render a complete Markdown document to a PDF file.
    pub fn generate_markdown_pdf(&mut self, markdown: &str, output_path: &str) -> Result<()> {
        // Page tree / catalog only; fonts are added after layout so they can be
        // subset to the glyphs actually used.
        self.init_document_structure()?;

        let blocks = parse_markdown(markdown);

        let (cx, cy, cw, ch) = self.safe_content_area();
        let mut ctx = MdCtx {
            content: Content {
                operations: Vec::new(),
            },
            pages: Vec::new(),
            y: cy,
            top: cy,
            bottom: cy + ch,
            used_chinese: BTreeSet::new(),
            used_english: BTreeSet::new(),
            used_mono: BTreeSet::new(),
        };

        for block in &blocks {
            self.md_render_block(&mut ctx, block, cx, cw)?;
        }

        // Flush the final page.
        let last = std::mem::replace(
            &mut ctx.content,
            Content {
                operations: Vec::new(),
            },
        );
        ctx.pages.push(last);

        // Embed subsetted fonts now that we know which glyphs are referenced.
        let used_chinese = std::mem::take(&mut ctx.used_chinese);
        let used_english = std::mem::take(&mut ctx.used_english);
        let used_mono = std::mem::take(&mut ctx.used_mono);
        let pages = std::mem::take(&mut ctx.pages);

        self.add_font_to_document_subset("chinese", &used_chinese)?;
        self.add_font_to_document_subset("english", &used_english)?;
        self.add_font_to_document_subset("mono", &used_mono)?;

        for content in pages {
            if content.operations.is_empty() {
                continue;
            }
            self.emit_markdown_page(content)?;
        }

        self.save_document(output_path)?;
        Ok(())
    }

    /// Create a page object from a finished content stream.
    fn emit_markdown_page(&mut self, content: Content) -> Result<()> {
        let page_id = self.document.new_object_id();

        let mut page_dict = Dictionary::new();
        page_dict.set("Type", Object::Name(b"Page".to_vec()));
        page_dict.set("Parent", Object::Reference(self.pages_id));
        page_dict.set("Resources", self.create_resources_dict()?);
        page_dict.set(
            "MediaBox",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Real(self.font_context.page_width),
                Object::Real(self.font_context.page_height),
            ]),
        );

        let content_stream = Stream::new(Dictionary::new(), content.encode()?);
        let content_id = self.document.add_object(content_stream);
        page_dict.set("Contents", Object::Reference(content_id));

        self.document
            .objects
            .insert(page_id, Object::Dictionary(page_dict));
        self.add_page_to_tree(page_id)?;
        Ok(())
    }

    /// Break to a new page if `h` doesn't fit and we're not already at the top.
    fn md_ensure(&self, ctx: &mut MdCtx, h: f32) {
        if ctx.y + h > ctx.bottom && ctx.y > ctx.top + 0.5 {
            ctx.page_break();
        }
    }

    fn md_render_block(
        &mut self,
        ctx: &mut MdCtx,
        block: &MdBlock,
        x_left: f32,
        max_width: f32,
    ) -> Result<()> {
        let base = self.md_base();
        match block {
            MdBlock::Heading { level, spans } => {
                let size = self.md_heading_size(*level);
                if ctx.y > ctx.top + 0.5 {
                    ctx.y += base * if *level <= 2 { 1.0 } else { 0.8 };
                }
                let lines = self.md_layout_inlines(spans, size, true, max_width);
                self.md_flow_lines(ctx, &lines, x_left);
                // Underline rule for H1/H2.
                if *level <= 2 {
                    ctx.y += base * 0.25;
                    self.md_ensure(ctx, 2.0);
                    let y = self.font_context.page_height - ctx.y;
                    self.md_rule(ctx, x_left, x_left + max_width, y, 0.75, 0.6);
                    ctx.y += base * 0.4;
                } else {
                    ctx.y += base * 0.35;
                }
            }
            MdBlock::Paragraph { spans } => {
                self.md_render_spans_with_images(ctx, spans, base, x_left, max_width)?;
                ctx.y += base * 0.6;
            }
            MdBlock::BlockQuote(inner) => {
                let indent = base * 1.2;
                let start_page = ctx.pages.len();
                let y_start = ctx.y;
                ctx.y += base * 0.2;
                for b in inner {
                    self.md_render_block(ctx, b, x_left + indent, max_width - indent)?;
                }
                ctx.y += base * 0.2;
                // Draw the quote bar only when the quote stayed on one page.
                if ctx.pages.len() == start_page {
                    let bar_w = 3.0;
                    let top = self.font_context.page_height - y_start;
                    let bot = self.font_context.page_height - ctx.y;
                    self.md_fill_rect(
                        ctx,
                        x_left,
                        bot,
                        bar_w,
                        top - bot,
                        (0.6, 0.6, 0.65),
                    );
                }
                ctx.y += base * 0.4;
            }
            MdBlock::List {
                ordered,
                start,
                items,
            } => {
                let indent = base * 1.6;
                let mut number = *start;
                ctx.y += base * 0.15;
                for item in items {
                    let marker = if *ordered {
                        let m = format!("{}.", number);
                        number += 1;
                        m
                    } else {
                        "\u{2022}".to_string()
                    };
                    self.md_ensure(ctx, self.md_line_height(base));
                    // Draw the marker aligned to the item's first baseline.
                    let baseline = self.font_context.page_height - (ctx.y + self.md_ascent(base));
                    let marker_run = MdRun {
                        x: 0.0,
                        width: 0.0,
                        text: marker,
                        font: MdFont::English,
                        size: base,
                        style: InlineStyle::default(),
                    };
                    self.md_draw_run(ctx, &marker_run, x_left + base * 0.4, baseline);

                    for b in item {
                        self.md_render_block(ctx, b, x_left + indent, max_width - indent)?;
                    }
                }
                ctx.y += base * 0.3;
            }
            MdBlock::CodeBlock { text, .. } => {
                self.md_render_code_block(ctx, text, x_left, max_width);
            }
            MdBlock::Table {
                aligns,
                header,
                rows,
            } => {
                self.md_render_table(ctx, aligns, header, rows, x_left, max_width)?;
            }
            MdBlock::Rule => {
                ctx.y += base * 0.4;
                self.md_ensure(ctx, 2.0);
                let y = self.font_context.page_height - ctx.y;
                self.md_rule(ctx, x_left, x_left + max_width, y, 0.75, 0.7);
                ctx.y += base * 0.6;
            }
        }
        Ok(())
    }

    /// Render paragraph spans, splitting block-level images out onto their own lines.
    fn md_render_spans_with_images(
        &mut self,
        ctx: &mut MdCtx,
        spans: &[Span],
        size: f32,
        x_left: f32,
        max_width: f32,
    ) -> Result<()> {
        let mut buffer: Vec<Span> = Vec::new();
        for span in spans {
            match span {
                Span::Image { url, alt } => {
                    if !buffer.is_empty() {
                        let lines = self.md_layout_inlines(&buffer, size, false, max_width);
                        self.md_flow_lines(ctx, &lines, x_left);
                        buffer.clear();
                    }
                    self.md_render_image(ctx, url, alt, x_left, max_width)?;
                }
                other => buffer.push(other.clone()),
            }
        }
        if !buffer.is_empty() {
            let lines = self.md_layout_inlines(&buffer, size, false, max_width);
            self.md_flow_lines(ctx, &lines, x_left);
        }
        Ok(())
    }

    /// Tokenize + greedily wrap inline spans into positioned lines.
    fn md_layout_inlines(
        &self,
        spans: &[Span],
        base_size: f32,
        force_bold: bool,
        max_width: f32,
    ) -> Vec<MdLine> {
        let space_w = self.md_glyph_advance(' ', MdFont::English, base_size);

        let mut toks: Vec<MdTok> = Vec::new();
        let mut buf = String::new();
        let mut buf_font = MdFont::English;
        let mut buf_style = InlineStyle::default();
        let mut buf_break_before = true;
        let mut break_next = true;

        macro_rules! flush_buf {
            () => {
                if !buf.is_empty() {
                    let w = buf
                        .chars()
                        .map(|c| self.md_glyph_advance(c, buf_font, base_size))
                        .sum();
                    toks.push(MdTok {
                        text: std::mem::take(&mut buf),
                        font: buf_font,
                        size: base_size,
                        style: buf_style.clone(),
                        width: w,
                        is_space: false,
                        breakable_before: buf_break_before,
                        force_break: false,
                    });
                }
            };
        }

        for span in spans {
            match span {
                Span::LineBreak { hard } => {
                    flush_buf!();
                    if *hard {
                        toks.push(MdTok {
                            text: String::new(),
                            font: MdFont::English,
                            size: base_size,
                            style: InlineStyle::default(),
                            width: 0.0,
                            is_space: false,
                            breakable_before: true,
                            force_break: true,
                        });
                    } else {
                        toks.push(MdTok {
                            text: " ".to_string(),
                            font: MdFont::English,
                            size: base_size,
                            style: InlineStyle::default(),
                            width: space_w,
                            is_space: true,
                            breakable_before: true,
                            force_break: false,
                        });
                    }
                    break_next = true;
                }
                Span::Image { alt, .. } => {
                    // Inline image fallback inside a styled context: show alt text.
                    flush_buf!();
                    let txt = format!("[{}]", alt);
                    let w = txt
                        .chars()
                        .map(|c| self.md_glyph_advance(c, MdFont::English, base_size))
                        .sum();
                    let mut st = InlineStyle::default();
                    st.italic = true;
                    toks.push(MdTok {
                        text: txt,
                        font: MdFont::English,
                        size: base_size,
                        style: st,
                        width: w,
                        is_space: false,
                        breakable_before: break_next,
                        force_break: false,
                    });
                    break_next = false;
                }
                Span::Text { text, style } => {
                    let mut style = style.clone();
                    if force_bold {
                        style.bold = true;
                    }
                    for ch in text.chars() {
                        if ch.is_whitespace() {
                            flush_buf!();
                            toks.push(MdTok {
                                text: " ".to_string(),
                                font: MdFont::English,
                                size: base_size,
                                style: style.clone(),
                                width: space_w,
                                is_space: true,
                                breakable_before: true,
                                force_break: false,
                            });
                            break_next = true;
                            continue;
                        }

                        let cjk = is_cjk(ch);
                        let font = if cjk {
                            MdFont::Chinese
                        } else if style.code {
                            MdFont::Mono
                        } else {
                            MdFont::English
                        };

                        if cjk {
                            flush_buf!();
                            let w = self.md_glyph_advance(ch, font, base_size);
                            toks.push(MdTok {
                                text: ch.to_string(),
                                font,
                                size: base_size,
                                style: style.clone(),
                                width: w,
                                is_space: false,
                                breakable_before: true,
                                force_break: false,
                            });
                            break_next = true;
                        } else {
                            // Extend the current Latin word if same face/style.
                            if !buf.is_empty() && (buf_font != font || buf_style != style) {
                                flush_buf!();
                            }
                            if buf.is_empty() {
                                buf_font = font;
                                buf_style = style.clone();
                                buf_break_before = break_next;
                            }
                            buf.push(ch);
                            break_next = false;
                        }
                    }
                }
            }
        }
        flush_buf!();

        // Greedy line breaking.
        let mut lines: Vec<MdLine> = Vec::new();
        let mut cur: Vec<MdTok> = Vec::new();
        let mut cur_w = 0.0f32;

        let finalize = |cur: &mut Vec<MdTok>, lines: &mut Vec<MdLine>, base_size: f32, this: &Self| {
            // Trim trailing spaces.
            while cur.last().map_or(false, |t| t.is_space) {
                cur.pop();
            }
            this.md_tokens_to_line(cur, base_size, lines);
            cur.clear();
        };

        for tok in toks {
            if tok.force_break {
                finalize(&mut cur, &mut lines, base_size, self);
                cur_w = 0.0;
                continue;
            }
            if tok.is_space && cur.is_empty() {
                continue; // no leading space
            }
            if tok.breakable_before && !cur.is_empty() && cur_w + tok.width > max_width {
                finalize(&mut cur, &mut lines, base_size, self);
                cur_w = 0.0;
                if tok.is_space {
                    continue;
                }
            }
            cur_w += tok.width;
            cur.push(tok);
        }
        finalize(&mut cur, &mut lines, base_size, self);

        if lines.is_empty() {
            // Preserve vertical space for an otherwise empty block.
            lines.push(MdLine {
                runs: Vec::new(),
                height: self.md_line_height(base_size),
                ascent: self.md_ascent(base_size),
            });
        }
        lines
    }

    /// Merge a line's tokens into positioned runs (same face/style coalesced).
    fn md_tokens_to_line(&self, toks: &[MdTok], base_size: f32, lines: &mut Vec<MdLine>) {
        let mut runs: Vec<MdRun> = Vec::new();
        let mut x = 0.0f32;
        let mut max_size = base_size;

        for tok in toks {
            max_size = max_size.max(tok.size);
            let merge = runs.last().map_or(false, |r| {
                r.font == tok.font && r.size == tok.size && r.style == tok.style
            });
            if merge {
                let r = runs.last_mut().unwrap();
                r.text.push_str(&tok.text);
                r.width += tok.width;
            } else {
                runs.push(MdRun {
                    x,
                    width: tok.width,
                    text: tok.text.clone(),
                    font: tok.font,
                    size: tok.size,
                    style: tok.style.clone(),
                });
            }
            x += tok.width;
        }

        lines.push(MdLine {
            runs,
            height: self.md_line_height(max_size),
            ascent: self.md_ascent(max_size),
        });
    }

    /// Draw a sequence of laid-out lines, paginating as needed.
    fn md_flow_lines(&self, ctx: &mut MdCtx, lines: &[MdLine], x_left: f32) {
        for line in lines {
            self.md_ensure(ctx, line.height);
            let baseline = self.font_context.page_height - (ctx.y + line.ascent);

            // Backgrounds (inline code) behind the text.
            for run in &line.runs {
                if run.style.code && !run.text.trim().is_empty() {
                    self.md_fill_rect(
                        ctx,
                        x_left + run.x - 1.0,
                        baseline - run.size * 0.22,
                        run.width + 2.0,
                        run.size * 1.15,
                        (0.94, 0.94, 0.95),
                    );
                }
            }
            for run in &line.runs {
                self.md_draw_run(ctx, run, x_left + run.x, baseline);
            }
            ctx.y += line.height;
        }
    }

    /// Emit a single styled run at the given baseline (PDF coordinates).
    fn md_draw_run(&self, ctx: &mut MdCtx, run: &MdRun, x: f32, baseline: f32) {
        if run.text.is_empty() {
            return;
        }
        ctx.record(run.font, &run.text);
        let font = run.font;
        let size = run.size;
        let (r, g, b) = if run.style.link.is_some() {
            (0.0, 0.2, 0.65)
        } else {
            (0.0, 0.0, 0.0)
        };

        let ops = ctx.ops();
        ops.push(Operation::new("BT", vec![]));
        ops.push(Operation::new(
            "Tf",
            vec![
                Object::Name(font.resource().as_bytes().to_vec()),
                Object::Real(size),
            ],
        ));
        ops.push(Operation::new(
            "rg",
            vec![Object::Real(r), Object::Real(g), Object::Real(b)],
        ));
        if run.style.bold {
            // Faux bold: fill + stroke the glyphs.
            ops.push(Operation::new("Tr", vec![Object::Integer(2)]));
            ops.push(Operation::new("w", vec![Object::Real(size * 0.03)]));
            ops.push(Operation::new(
                "RG",
                vec![Object::Real(r), Object::Real(g), Object::Real(b)],
            ));
        }
        // Faux italic via horizontal shear in the text matrix.
        let shear = if run.style.italic { 0.2126 } else { 0.0 };
        ops.push(Operation::new(
            "Tm",
            vec![
                Object::Real(1.0),
                Object::Real(0.0),
                Object::Real(shear),
                Object::Real(1.0),
                Object::Real(x),
                Object::Real(baseline),
            ],
        ));
        ops.push(Operation::new("TJ", vec![Object::Array(self.md_run_tj(run))]));
        ops.push(Operation::new("ET", vec![]));

        // Underline (links) and strikethrough.
        if run.style.link.is_some() {
            self.md_fill_rect(
                ctx,
                x,
                baseline - size * 0.1,
                run.width,
                size * 0.05,
                (r, g, b),
            );
        }
        if run.style.strike {
            self.md_fill_rect(
                ctx,
                x,
                baseline + size * 0.28,
                run.width,
                size * 0.05,
                (0.0, 0.0, 0.0),
            );
        }
    }

    /// Build a TJ array with per-glyph advance correction (DW=1000 -> real width).
    fn md_run_tj(&self, run: &MdRun) -> Vec<Object> {
        let font = self.md_font_ref(run.font);
        let mut tj = Vec::new();
        for ch in run.text.chars() {
            let mut bytes = Vec::new();
            let mut buf = [0u16; 2];
            for unit in ch.encode_utf16(&mut buf).iter() {
                bytes.extend_from_slice(&unit.to_be_bytes());
            }
            tj.push(Object::String(bytes, StringFormat::Hexadecimal));

            let adv = font.metrics(ch, run.size).advance_width;
            let t = 1000.0 * (1.0 - adv / run.size);
            tj.push(Object::Real(t));
        }
        tj
    }

    fn md_render_code_block(&self, ctx: &mut MdCtx, text: &str, x_left: f32, max_width: f32) {
        let base = self.md_base();
        let size = self.font_context.font_size_mono;
        let pad = base * 0.5;
        let line_h = size * 1.4;
        let inner_w = (max_width - 2.0 * pad).max(40.0);

        ctx.y += base * 0.3;

        // Wrap each source line to the inner width (character wrapping).
        let mut rendered: Vec<String> = Vec::new();
        for raw in text.split('\n') {
            let mut cur = String::new();
            let mut cur_w = 0.0f32;
            for ch in raw.chars() {
                let w = self.md_glyph_advance(ch, MdFont::Mono, size);
                if cur_w + w > inner_w && !cur.is_empty() {
                    rendered.push(std::mem::take(&mut cur));
                    cur_w = 0.0;
                }
                cur.push(ch);
                cur_w += w;
            }
            rendered.push(cur);
        }

        let mut first = true;
        for src in &rendered {
            self.md_ensure(ctx, line_h);
            // Full-width background for the line (plus top/bottom padding).
            let top_pad = if first { pad } else { 0.0 };
            let bg_top = self.font_context.page_height - (ctx.y - top_pad);
            let bg_h = line_h + top_pad;
            self.md_fill_rect(
                ctx,
                x_left,
                bg_top - bg_h,
                max_width,
                bg_h,
                (0.96, 0.96, 0.97),
            );

            let baseline = self.font_context.page_height - (ctx.y + size);
            let run = MdRun {
                x: 0.0,
                width: inner_w,
                text: src.clone(),
                font: MdFont::Mono,
                size,
                style: InlineStyle::default(),
            };
            self.md_draw_run(ctx, &run, x_left + pad, baseline);
            ctx.y += line_h;
            first = false;
        }
        // Bottom padding background.
        self.md_ensure(ctx, pad);
        let bg_top = self.font_context.page_height - ctx.y;
        self.md_fill_rect(ctx, x_left, bg_top - pad, max_width, pad, (0.96, 0.96, 0.97));
        ctx.y += pad + base * 0.5;
    }

    fn md_render_table(
        &self,
        ctx: &mut MdCtx,
        aligns: &[Align],
        header: &[Vec<Span>],
        rows: &[Vec<Vec<Span>>],
        x_left: f32,
        max_width: f32,
    ) -> Result<()> {
        let base = self.md_base();
        let pad = base * 0.4;

        let col_count = header
            .len()
            .max(rows.iter().map(|r| r.len()).max().unwrap_or(0))
            .max(1);

        // Natural width of each column (capped), then normalized to max_width.
        let mut natural = vec![0.0f32; col_count];
        let consider = |cells: &[Vec<Span>], natural: &mut Vec<f32>| {
            for (c, cell) in cells.iter().enumerate() {
                if c >= col_count {
                    break;
                }
                let w: f32 = cell
                    .iter()
                    .map(|s| match s {
                        Span::Text { text, .. } => text
                            .chars()
                            .map(|ch| {
                                let f = if is_cjk(ch) {
                                    MdFont::Chinese
                                } else {
                                    MdFont::English
                                };
                                self.md_glyph_advance(ch, f, base)
                            })
                            .sum(),
                        _ => 0.0,
                    })
                    .sum();
                natural[c] = natural[c].max(w.min(max_width * 0.6));
            }
        };
        consider(header, &mut natural);
        for row in rows {
            consider(row, &mut natural);
        }
        let total: f32 = natural.iter().sum::<f32>() + 2.0 * pad * col_count as f32;
        let scale = if total > max_width {
            max_width / total
        } else {
            1.0
        };
        let col_w: Vec<f32> = natural
            .iter()
            .map(|n| (n + 2.0 * pad) * scale)
            .collect();

        ctx.y += base * 0.3;

        // Header then data rows.
        self.md_render_table_row(ctx, header, aligns, &col_w, x_left, pad, true);
        for row in rows {
            self.md_render_table_row(ctx, row, aligns, &col_w, x_left, pad, false);
        }
        ctx.y += base * 0.5;
        Ok(())
    }

    fn md_render_table_row(
        &self,
        ctx: &mut MdCtx,
        cells: &[Vec<Span>],
        aligns: &[Align],
        col_w: &[f32],
        x_left: f32,
        pad: f32,
        header: bool,
    ) {
        let base = self.md_base();

        // Lay out every cell first to find the row height.
        let mut cell_lines: Vec<Vec<MdLine>> = Vec::new();
        let mut row_h = self.md_line_height(base);
        for (c, cw) in col_w.iter().enumerate() {
            let empty = Vec::new();
            let spans = cells.get(c).unwrap_or(&empty);
            let lines = self.md_layout_inlines(spans, base, header, (cw - 2.0 * pad).max(10.0));
            let h: f32 = lines.iter().map(|l| l.height).sum();
            row_h = row_h.max(h);
            cell_lines.push(lines);
        }
        row_h += 2.0 * pad;

        self.md_ensure(ctx, row_h);

        let row_top = ctx.y;
        // Header background.
        if header {
            let top = self.font_context.page_height - row_top;
            self.md_fill_rect(
                ctx,
                x_left,
                top - row_h,
                col_w.iter().sum::<f32>(),
                row_h,
                (0.92, 0.92, 0.94),
            );
        }

        // Cell borders + text.
        let mut cx = x_left;
        for (c, cw) in col_w.iter().enumerate() {
            // Border rectangle.
            let top = self.font_context.page_height - row_top;
            self.md_stroke_rect(ctx, cx, top - row_h, *cw, row_h, (0.55, 0.55, 0.6));

            // Text, aligned within the cell.
            let align = aligns.get(c).copied().unwrap_or(Align::Left);
            let lines = &cell_lines[c];
            let mut yy = row_top + pad;
            for line in lines {
                let line_w: f32 = line.runs.iter().map(|r| r.width).sum();
                let avail = cw - 2.0 * pad;
                let off = match align {
                    Align::Left => 0.0,
                    Align::Center => (avail - line_w).max(0.0) / 2.0,
                    Align::Right => (avail - line_w).max(0.0),
                };
                let baseline = self.font_context.page_height - (yy + line.ascent);
                for run in &line.runs {
                    self.md_draw_run(ctx, run, cx + pad + off + run.x, baseline);
                }
                yy += line.height;
            }
            cx += cw;
        }

        ctx.y += row_h;
    }

    /// Embed a local raster image and draw it scaled to fit the content width.
    fn md_render_image(
        &mut self,
        ctx: &mut MdCtx,
        url: &str,
        alt: &str,
        x_left: f32,
        max_width: f32,
    ) -> Result<()> {
        if let Some((name, iw, ih)) = self.md_embed_image(url) {
            let avail_h = ctx.bottom - ctx.top;
            let mut scale = if iw > max_width { max_width / iw } else { 1.0 };
            if ih * scale > avail_h {
                scale = avail_h / ih;
            }
            let dw = iw * scale;
            let dh = ih * scale;

            ctx.y += self.md_base() * 0.3;
            self.md_ensure(ctx, dh);
            let pdf_y = self.font_context.page_height - (ctx.y + dh);

            let ops = ctx.ops();
            ops.push(Operation::new("q", vec![]));
            ops.push(Operation::new(
                "cm",
                vec![
                    Object::Real(dw),
                    Object::Real(0.0),
                    Object::Real(0.0),
                    Object::Real(dh),
                    Object::Real(x_left),
                    Object::Real(pdf_y),
                ],
            ));
            ops.push(Operation::new("Do", vec![Object::Name(name.into_bytes())]));
            ops.push(Operation::new("Q", vec![]));
            ctx.y += dh + self.md_base() * 0.4;
        } else {
            // Fallback: render alt text in muted italic.
            let mut style = InlineStyle::default();
            style.italic = true;
            let spans = vec![Span::Text {
                text: format!("[image: {}]", alt),
                style,
            }];
            let base = self.md_base();
            let lines = self.md_layout_inlines(&spans, base, false, max_width);
            self.md_flow_lines(ctx, &lines, x_left);
        }
        Ok(())
    }

    /// Decode a local image to an RGB XObject and register it. Returns
    /// `(resource_name, width_px, height_px)` on success.
    fn md_embed_image(&mut self, url: &str) -> Option<(String, f32, f32)> {
        if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("data:") {
            return None;
        }
        let path = url.strip_prefix("file://").unwrap_or(url);
        let raw = std::fs::read(path).ok()?;
        let img = image::load_from_memory(&raw).ok()?;
        let rgb = img.to_rgb8();
        let (w, h) = (rgb.width(), rgb.height());

        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(rgb.as_raw()).ok()?;
        let compressed = encoder.finish().ok()?;

        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"XObject".to_vec()));
        dict.set("Subtype", Object::Name(b"Image".to_vec()));
        dict.set("Width", Object::Integer(w as i64));
        dict.set("Height", Object::Integer(h as i64));
        dict.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
        dict.set("BitsPerComponent", Object::Integer(8));
        dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));

        let id = self.document.add_object(Stream::new(dict, compressed));
        let name = format!("Im{}", self.xobjects.len());
        self.xobjects.insert(name.clone(), id);
        Some((name, w as f32, h as f32))
    }

    /// Filled rectangle (PDF coordinates), saved/restored so it doesn't leak state.
    fn md_fill_rect(&self, ctx: &mut MdCtx, x: f32, y: f32, w: f32, h: f32, color: (f32, f32, f32)) {
        let ops = ctx.ops();
        ops.push(Operation::new("q", vec![]));
        ops.push(Operation::new(
            "rg",
            vec![
                Object::Real(color.0),
                Object::Real(color.1),
                Object::Real(color.2),
            ],
        ));
        ops.push(Operation::new(
            "re",
            vec![
                Object::Real(x),
                Object::Real(y),
                Object::Real(w),
                Object::Real(h),
            ],
        ));
        ops.push(Operation::new("f", vec![]));
        ops.push(Operation::new("Q", vec![]));
    }

    fn md_stroke_rect(&self, ctx: &mut MdCtx, x: f32, y: f32, w: f32, h: f32, color: (f32, f32, f32)) {
        let ops = ctx.ops();
        ops.push(Operation::new("q", vec![]));
        ops.push(Operation::new(
            "RG",
            vec![
                Object::Real(color.0),
                Object::Real(color.1),
                Object::Real(color.2),
            ],
        ));
        ops.push(Operation::new("w", vec![Object::Real(0.5)]));
        ops.push(Operation::new(
            "re",
            vec![
                Object::Real(x),
                Object::Real(y),
                Object::Real(w),
                Object::Real(h),
            ],
        ));
        ops.push(Operation::new("S", vec![]));
        ops.push(Operation::new("Q", vec![]));
    }

    /// Horizontal rule drawn as a thin filled bar.
    fn md_rule(&self, ctx: &mut MdCtx, x0: f32, x1: f32, y: f32, thickness: f32, gray: f32) {
        self.md_fill_rect(ctx, x0, y, x1 - x0, thickness, (gray, gray, gray));
    }
}

/// Create a bilingual PDF generator
pub fn create_bilingual_generator(font_context: FontContext) -> BilingualPdfGenerator {
    BilingualPdfGenerator::new(font_context)
}

/// Render Markdown text to a PDF file using bundled CJK-aware typography.
pub fn create_markdown_pdf(markdown: &str, output_path: &str) -> Result<()> {
    let font_context = crate::fonts::initialize_fonts()?;
    let mut generator = create_bilingual_generator(font_context);
    generator.generate_markdown_pdf(markdown, output_path)
}

/// Render Markdown to a PDF file with a caller-provided font context.
pub fn create_markdown_pdf_with_context(
    markdown: &str,
    output_path: &str,
    font_context: &crate::fonts::FontContext,
) -> Result<()> {
    let mut generator = create_bilingual_generator(font_context.clone());
    generator.generate_markdown_pdf(markdown, output_path)
}

/// Main entry point for bilingual PDF creation
pub fn create_bilingual_pdf(
    chinese_sections: &[String],
    english_sections: &[String],
    output_path: &str,
) -> Result<()> {
    let font_context = crate::fonts::initialize_fonts()?;
    let mut generator = create_bilingual_generator(font_context);
    
    generator.generate_bilingual_pdf(chinese_sections, english_sections, output_path)
}

/// Create bilingual PDF with custom font context (for professional typography)
pub fn create_bilingual_pdf_with_context(
    chinese_sections: &[String],
    english_sections: &[String],
    output_path: &str,
    font_context: &crate::fonts::FontContext,
) -> Result<()> {
    let mut generator = create_bilingual_generator(font_context.clone());
    
    generator.generate_bilingual_pdf(chinese_sections, english_sections, output_path)
}

/// Create bilingual PDF with side-by-side columns and custom font context.
pub fn create_bilingual_pdf_side_by_side_with_context(
    chinese_sections: &[String],
    english_sections: &[String],
    output_path: &str,
    font_context: &crate::fonts::FontContext,
) -> Result<()> {
    let mut generator = create_bilingual_generator(font_context.clone());
    generator.generate_bilingual_pdf_side_by_side(chinese_sections, english_sections, output_path)
}
