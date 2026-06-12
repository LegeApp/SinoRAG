//! End-to-end smoke test for Markdown -> PDF conversion.

use cbeta_pdf_creator::create_markdown_pdf;

const SAMPLE: &str = r#"# SinoRAG Report 报告

This is a **bold** statement with *italic* and `inline code`, plus a
[link](https://example.com) and ~~struck~~ text.

## 中文小节

观自在菩薩，行深般若波羅蜜多時，照見五蘊皆空，度一切苦厄。这是一段中文段落，
用来测试中文字符的排版与换行是否正常工作。

- First bullet item
- Second bullet with 中文 mixed in
- Third item

1. Ordered one
2. Ordered two

> A blockquote line.
> 引用的中文内容。

```rust
fn main() {
    println!("Hello, 世界!");
}
```

| Name | 数量 | Note |
|:-----|-----:|:----:|
| Alpha | 1 | ok |
| 贝塔 | 22 | 好 |

---

Final paragraph after a horizontal rule.
"#;

#[test]
fn markdown_to_pdf_smoke() {
    let out = std::env::temp_dir().join("sinorag_md_smoke.pdf");
    let out_str = out.to_str().unwrap();

    create_markdown_pdf(SAMPLE, out_str).expect("markdown conversion failed");

    let bytes = std::fs::read(&out).expect("output pdf not written");
    assert!(bytes.len() > 1000, "pdf suspiciously small: {} bytes", bytes.len());
    assert!(bytes.starts_with(b"%PDF-"), "output is not a PDF");

    // Re-open with lopdf and confirm it has at least one page.
    let doc = lopdf::Document::load(&out).expect("produced PDF does not parse");
    let pages = doc.get_pages();
    assert!(!pages.is_empty(), "PDF has no pages");

    println!(
        "Wrote {} ({} bytes, {} page(s))",
        out_str,
        bytes.len(),
        pages.len()
    );
}
