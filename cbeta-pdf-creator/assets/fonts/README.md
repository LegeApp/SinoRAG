# Bundled Fonts (Optional but Recommended)

For portable, print-quality bilingual / Markdown PDFs, the loader prefers these
bundled files before falling back to system fonts.

Currently bundled:

- `NotoSerifCJKtc-Regular.ttf` — Chinese (Traditional) body text.
- `NotoSerif-Regular.ttf` — English body text.
- `DejaVuSansMono.ttf` — monospace, used for Markdown code blocks / inline code.

Recognized fallback names (used if the above are absent):

- `NotoSerifCJKtc-Regular.otf`, `SourceHanSerifTC-Regular.otf` (Chinese)
- `EBGaramond-Regular.ttf` (English)

## Why the CJK font is a `.ttf`, not the upstream `.otf`

Noto Serif CJK ships with **CFF (PostScript) outlines** (`.otf`/`.ttc`). The PDF
embedder declares the descendant font as `CIDFontType2` with an explicit
CIDToGIDMap, which is only valid for **TrueType (`glyf`) outlines**. Embedding the
CFF `.otf` produces a "mismatch between font type and embedded font file" that
strict viewers (Acrobat, some print RIPs) may reject.

The bundled `NotoSerifCJKtc-Regular.ttf` is the upstream font converted to
TrueType outlines so it embeds as a valid `CIDFontType2`/`FontFile2`. It was
produced from the system `NotoSerifCJK-Regular.ttc` (TC face) with:

    pip install otf2ttf   # pulls in fontTools + cu2qu
    # extract the TC face from the .ttc, then:
    python -m otf2ttf NotoSerifCJKtc-Regular.otf -o NotoSerifCJKtc-Regular.ttf

Upstream source for the per-language OTFs (if you prefer to regenerate):
https://github.com/notofonts/noto-cjk/releases (Serif → `10_NotoSerifCJKtc.zip`).

## Licensing

Noto fonts are licensed under the SIL Open Font License (OFL); DejaVu under its
permissive Bitstream Vera/Arev license. Both are freely redistributable.

## Note on file size

The Markdown pipeline (`create_markdown_pdf*` / the `md2pdf` binary) **subsets**
each embedded font to only the glyphs actually used, so output is small — a
CJK-heavy page lands around 30–40 KB rather than ~17 MB. The bilingual pipeline
still embeds the full fonts.
