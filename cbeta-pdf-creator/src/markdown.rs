//! Markdown parsing into a styled block model.
//!
//! Uses `pulldown-cmark` (CommonMark + GFM tables/strikethrough) and lowers the
//! event stream into a small nested block tree (`MdBlock`) with inline styled
//! spans. The PDF renderer in `bilingual_generator.rs` consumes this model and
//! reuses the existing CJK-aware font machinery to lay it out.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag};

/// Column alignment for a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// Inline styling that applies to a run of text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InlineStyle {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    /// `Some(url)` when this run is part of a hyperlink.
    pub link: Option<String>,
}

/// A piece of inline content within a block.
#[derive(Debug, Clone)]
pub enum Span {
    /// Styled run of text.
    Text { text: String, style: InlineStyle },
    /// Image reference (rendered as an embedded raster, or alt text on failure).
    Image { url: String, alt: String },
    /// Line break inside a block (`hard` = explicit `\` / two-space break).
    LineBreak { hard: bool },
}

/// A block-level Markdown element.
#[derive(Debug, Clone)]
pub enum MdBlock {
    Heading {
        level: u8,
        spans: Vec<Span>,
    },
    Paragraph {
        spans: Vec<Span>,
    },
    BlockQuote(Vec<MdBlock>),
    List {
        ordered: bool,
        start: u64,
        items: Vec<Vec<MdBlock>>,
    },
    CodeBlock {
        text: String,
        lang: Option<String>,
    },
    Table {
        aligns: Vec<Align>,
        header: Vec<Vec<Span>>,
        rows: Vec<Vec<Vec<Span>>>,
    },
    Rule,
}

/// Parse Markdown source into a block tree.
pub fn parse_markdown(src: &str) -> Vec<MdBlock> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_TASKLISTS);

    let events: Vec<Event> = Parser::new_ext(src, options).collect();
    let mut i = 0;
    parse_blocks(&events, &mut i)
}

/// Read block-level elements until the End of the enclosing container (which is
/// left unconsumed for the caller) or the end of the event stream.
fn parse_blocks(ev: &[Event], i: &mut usize) -> Vec<MdBlock> {
    let mut blocks = Vec::new();

    while *i < ev.len() {
        match &ev[*i] {
            Event::End(_) => break, // belongs to our parent container
            Event::Rule => {
                blocks.push(MdBlock::Rule);
                *i += 1;
            }
            // Block-level container.
            Event::Start(tag) if !is_inline_tag(tag) => {
                let tag = tag.clone();
                *i += 1;
                if let Some(block) = parse_block(&tag, ev, i) {
                    blocks.push(block);
                } else {
                    skip_to_end(ev, i);
                }
            }
            // Bare inline content (e.g. a tight list item) — wrap as a paragraph.
            Event::Text(_)
            | Event::Code(_)
            | Event::SoftBreak
            | Event::HardBreak
            | Event::Start(_) => {
                let spans = parse_inlines(ev, i, InlineStyle::default());
                if !spans.is_empty() {
                    blocks.push(MdBlock::Paragraph { spans });
                }
            }
            // Other events (HTML, footnote refs, etc.) — ignore.
            _ => *i += 1,
        }
    }

    blocks
}

/// True for inline-level tags (which never open a new block).
fn is_inline_tag(tag: &Tag) -> bool {
    matches!(
        tag,
        Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. } | Tag::Image { .. }
    )
}

/// Parse a single block whose `Start(tag)` has already been consumed. Consumes
/// the matching `End`.
fn parse_block(tag: &Tag, ev: &[Event], i: &mut usize) -> Option<MdBlock> {
    match tag {
        Tag::Paragraph => {
            let spans = parse_inlines(ev, i, InlineStyle::default());
            consume_end(ev, i);
            Some(MdBlock::Paragraph { spans })
        }
        Tag::Heading { level, .. } => {
            let spans = parse_inlines(ev, i, InlineStyle::default());
            consume_end(ev, i);
            Some(MdBlock::Heading {
                level: *level as u8,
                spans,
            })
        }
        Tag::BlockQuote(_) => {
            let inner = parse_blocks(ev, i);
            consume_end(ev, i);
            Some(MdBlock::BlockQuote(inner))
        }
        Tag::List(start) => {
            let ordered = start.is_some();
            let start_num = start.unwrap_or(1);
            let mut items = Vec::new();
            while *i < ev.len() {
                match &ev[*i] {
                    Event::Start(Tag::Item) => {
                        *i += 1;
                        let item_blocks = parse_blocks(ev, i);
                        consume_end(ev, i); // End(Item)
                        items.push(item_blocks);
                    }
                    Event::End(_) => break,
                    _ => *i += 1,
                }
            }
            consume_end(ev, i); // End(List)
            Some(MdBlock::List {
                ordered,
                start: start_num,
                items,
            })
        }
        Tag::CodeBlock(kind) => {
            let lang = match kind {
                CodeBlockKind::Fenced(info) => {
                    let first = info.split_whitespace().next().unwrap_or("");
                    if first.is_empty() {
                        None
                    } else {
                        Some(first.to_string())
                    }
                }
                CodeBlockKind::Indented => None,
            };
            let mut text = String::new();
            while *i < ev.len() {
                match &ev[*i] {
                    Event::Text(t) => {
                        text.push_str(t);
                        *i += 1;
                    }
                    Event::End(_) => break,
                    _ => *i += 1,
                }
            }
            consume_end(ev, i);
            // Drop a single trailing newline emitted by the fence.
            if text.ends_with('\n') {
                text.pop();
            }
            Some(MdBlock::CodeBlock { text, lang })
        }
        Tag::Table(aligns) => {
            let aligns: Vec<Align> = aligns
                .iter()
                .map(|a| match a {
                    Alignment::Center => Align::Center,
                    Alignment::Right => Align::Right,
                    _ => Align::Left,
                })
                .collect();

            let mut header = Vec::new();
            let mut rows = Vec::new();

            while *i < ev.len() {
                match &ev[*i] {
                    Event::Start(Tag::TableHead) => {
                        *i += 1;
                        header = parse_table_row(ev, i);
                        consume_end(ev, i); // End(TableHead)
                    }
                    Event::Start(Tag::TableRow) => {
                        *i += 1;
                        rows.push(parse_table_row(ev, i));
                        consume_end(ev, i); // End(TableRow)
                    }
                    Event::End(_) => break,
                    _ => *i += 1,
                }
            }
            consume_end(ev, i); // End(Table)
            Some(MdBlock::Table {
                aligns,
                header,
                rows,
            })
        }
        // Unknown / unsupported container: signal caller to skip its subtree.
        _ => None,
    }
}

/// Parse the cells of a single table row (cursor positioned after the row Start).
fn parse_table_row(ev: &[Event], i: &mut usize) -> Vec<Vec<Span>> {
    let mut cells = Vec::new();
    while *i < ev.len() {
        match &ev[*i] {
            Event::Start(Tag::TableCell) => {
                *i += 1;
                let spans = parse_inlines(ev, i, InlineStyle::default());
                consume_end(ev, i); // End(TableCell)
                cells.push(spans);
            }
            Event::End(_) => break,
            _ => *i += 1,
        }
    }
    cells
}

/// Parse inline content until the End of the enclosing inline/leaf container.
/// Nested inline Starts consume their own matching End.
fn parse_inlines(ev: &[Event], i: &mut usize, style: InlineStyle) -> Vec<Span> {
    let mut spans = Vec::new();

    while *i < ev.len() {
        match &ev[*i] {
            Event::End(_) => break,
            Event::Text(t) => {
                spans.push(Span::Text {
                    text: t.to_string(),
                    style: style.clone(),
                });
                *i += 1;
            }
            Event::Code(t) => {
                let mut s = style.clone();
                s.code = true;
                spans.push(Span::Text {
                    text: t.to_string(),
                    style: s,
                });
                *i += 1;
            }
            Event::SoftBreak => {
                spans.push(Span::LineBreak { hard: false });
                *i += 1;
            }
            Event::HardBreak => {
                spans.push(Span::LineBreak { hard: true });
                *i += 1;
            }
            // A block-level Start ends the current inline run (e.g. a nested
            // list inside a tight list item). Leave it for the block parser.
            Event::Start(tag) if !is_inline_tag(tag) => break,
            Event::Start(tag) => {
                let tag = tag.clone();
                *i += 1;
                match tag {
                    Tag::Emphasis => {
                        let mut s = style.clone();
                        s.italic = true;
                        spans.extend(parse_inlines(ev, i, s));
                        consume_end(ev, i);
                    }
                    Tag::Strong => {
                        let mut s = style.clone();
                        s.bold = true;
                        spans.extend(parse_inlines(ev, i, s));
                        consume_end(ev, i);
                    }
                    Tag::Strikethrough => {
                        let mut s = style.clone();
                        s.strike = true;
                        spans.extend(parse_inlines(ev, i, s));
                        consume_end(ev, i);
                    }
                    Tag::Link { dest_url, .. } => {
                        let mut s = style.clone();
                        s.link = Some(dest_url.to_string());
                        spans.extend(parse_inlines(ev, i, s));
                        consume_end(ev, i);
                    }
                    Tag::Image { dest_url, .. } => {
                        // Collect alt text from the image's inner inlines.
                        let inner = parse_inlines(ev, i, InlineStyle::default());
                        consume_end(ev, i);
                        let alt = spans_to_plain(&inner);
                        spans.push(Span::Image {
                            url: dest_url.to_string(),
                            alt,
                        });
                    }
                    _ => {
                        // Unknown inline container — keep its text, drop styling.
                        spans.extend(parse_inlines(ev, i, style.clone()));
                        consume_end(ev, i);
                    }
                }
            }
            // TaskListMarker, HTML, FootnoteReference, etc. — ignore.
            _ => *i += 1,
        }
    }

    spans
}

/// Flatten spans to plain text (used for image alt text).
fn spans_to_plain(spans: &[Span]) -> String {
    let mut out = String::new();
    for span in spans {
        if let Span::Text { text, .. } = span {
            out.push_str(text);
        }
    }
    out
}

/// Consume a single `End` event if present.
fn consume_end(ev: &[Event], i: &mut usize) {
    if *i < ev.len() {
        if let Event::End(_) = ev[*i] {
            *i += 1;
        }
    }
}

/// Skip a subtree whose `Start` was already consumed, balancing Start/End.
fn skip_to_end(ev: &[Event], i: &mut usize) {
    let mut depth = 1usize;
    while *i < ev.len() && depth > 0 {
        match &ev[*i] {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth -= 1,
            _ => {}
        }
        *i += 1;
    }
}
