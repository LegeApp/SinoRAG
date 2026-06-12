//! `md2pdf` — convert Markdown file(s) to PDF using the CBETA CJK-aware engine.
//!
//! Usage:
//!     md2pdf [OPTIONS] <INPUT.md>...
//!
//! Options:
//!     -o, --output <PATH>   Output PDF file (single input) or output directory
//!                           (one or more inputs). Defaults to each input's path
//!                           with a `.pdf` extension.
//!     -h, --help            Show this help.
//!
//! Examples:
//!     md2pdf report.md                  # -> report.pdf
//!     md2pdf report.md -o out.pdf       # -> out.pdf
//!     md2pdf a.md b.md c.md             # -> a.pdf b.pdf c.pdf (alongside inputs)
//!     md2pdf docs/*.md -o build/        # -> build/<name>.pdf for each input

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cbeta_pdf_creator::create_markdown_pdf_with_context;
use cbeta_pdf_creator::fonts::initialize_fonts;

const USAGE: &str = "\
md2pdf — convert Markdown to PDF (with Chinese/CJK typography)

USAGE:
    md2pdf [OPTIONS] <INPUT.md>...

OPTIONS:
    -o, --output <PATH>   Output PDF file (single input) or output directory
                          (one or more inputs). Defaults to each input's path
                          with a .pdf extension.
    -h, --help            Show this help.

EXAMPLES:
    md2pdf report.md
    md2pdf report.md -o out.pdf
    md2pdf a.md b.md c.md
    md2pdf docs/*.md -o build/";

fn main() -> ExitCode {
    let mut inputs: Vec<PathBuf> = Vec::new();
    let mut output: Option<PathBuf> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "-o" | "--output" => match args.next() {
                Some(p) => output = Some(PathBuf::from(p)),
                None => {
                    eprintln!("error: {arg} requires a value\n\n{USAGE}");
                    return ExitCode::FAILURE;
                }
            },
            other if other.starts_with('-') && other != "-" => {
                eprintln!("error: unknown option '{other}'\n\n{USAGE}");
                return ExitCode::FAILURE;
            }
            other => inputs.push(PathBuf::from(other)),
        }
    }

    if inputs.is_empty() {
        eprintln!("error: no input files given\n\n{USAGE}");
        return ExitCode::FAILURE;
    }

    // Resolve output targets before doing any expensive work so we can fail fast.
    let targets = match resolve_targets(&inputs, output.as_deref()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Load fonts once and reuse for every document.
    let font_context = match initialize_fonts() {
        Ok(fc) => fc,
        Err(e) => {
            eprintln!("error: failed to initialize fonts: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut failures = 0usize;
    for (input, out) in &targets {
        match convert_one(input, out, &font_context) {
            Ok(()) => println!("{} -> {}", input.display(), out.display()),
            Err(e) => {
                eprintln!("error: {}: {e}", input.display());
                failures += 1;
            }
        }
    }

    if failures > 0 {
        eprintln!(
            "{} of {} file(s) failed",
            failures,
            targets.len()
        );
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Map each input file to its output PDF path.
fn resolve_targets(
    inputs: &[PathBuf],
    output: Option<&Path>,
) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let single = inputs.len() == 1;

    // Decide whether `output` is a directory target.
    let out_is_dir = match output {
        None => false,
        Some(p) => !single || looks_like_dir(p),
    };

    if let Some(p) = output {
        if out_is_dir {
            std::fs::create_dir_all(p)
                .map_err(|e| format!("cannot create output directory '{}': {e}", p.display()))?;
        } else if let Some(parent) = p.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    format!("cannot create output directory '{}': {e}", parent.display())
                })?;
            }
        }
    }

    let mut targets = Vec::with_capacity(inputs.len());
    for input in inputs {
        let out = match (output, out_is_dir) {
            // Explicit single output file.
            (Some(p), false) => p.to_path_buf(),
            // Output directory: <dir>/<stem>.pdf
            (Some(dir), true) => dir.join(pdf_file_name(input)),
            // Default: alongside the input, with a .pdf extension.
            (None, _) => input.with_extension("pdf"),
        };
        targets.push((input.clone(), out));
    }
    Ok(targets)
}

/// A path is treated as a directory target if it exists as one or ends with a
/// path separator.
fn looks_like_dir(p: &Path) -> bool {
    if p.is_dir() {
        return true;
    }
    let s = p.to_string_lossy();
    s.ends_with('/') || s.ends_with('\\')
}

/// `<stem>.pdf` for an input path (falls back to "output.pdf").
fn pdf_file_name(input: &Path) -> PathBuf {
    let stem = input
        .file_stem()
        .map(|s| s.to_owned())
        .unwrap_or_else(|| std::ffi::OsString::from("output"));
    let mut name = PathBuf::from(stem);
    name.set_extension("pdf");
    name
}

fn convert_one(
    input: &Path,
    out: &Path,
    font_context: &cbeta_pdf_creator::fonts::FontContext,
) -> Result<(), String> {
    let markdown = std::fs::read_to_string(input)
        .map_err(|e| format!("cannot read input: {e}"))?;
    let out_str = out
        .to_str()
        .ok_or_else(|| "output path is not valid UTF-8".to_string())?;
    create_markdown_pdf_with_context(&markdown, out_str, font_context)
        .map_err(|e| format!("conversion failed: {e}"))?;
    Ok(())
}
