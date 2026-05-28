# Third-Party Data and Code Attributions

SinoRAG incorporates data and ideas from the following third-party projects.

---

## cbeta-reader (zhaowenping/cbeta)

**Source:** <https://github.com/zhaowenping/cbeta>
**Author:** Zhao Wenping (赵文平)
**Description:** Open-source CBETA Buddhist canon reader and management system.

SinoRAG uses the following data files from this project:

- **`static/sutra_sch.lst`** — Authoritative hierarchical work catalog for the
  CBETA corpus (canon, section, work ID, title, juan count, translator, dynasty).
  Used as an embedded sidecar for coverage verification and metadata enrichment.

- **`idx/cmp.lst`** — Cross-canon parallel work relationships (which works in
  different canons are editions of the same text). Used for the "related editions"
  research feature.

- **`cc/KXVariants.txt`**, **`cc/TSCharacters.txt`**, **`cc/JPVariants.txt`** —
  Character normalization tables (Kangxi standard variants, Traditional/Simplified
  mappings, Japanese glyph variants). Integrated into SinoRAG's text normalization
  pipeline to improve search recall across variant character forms.

The cbeta-reader repository does not include an explicit license file. These data
files are used under the assumption that they are shared openly for the benefit of
Buddhist scholarship, consistent with the project's stated goal of being an open
reading program for the Buddhist canon ("做最好的开源阅藏程序"). If the author
objects to this use, we will remove the incorporated data promptly.

---

## Soothill & Hodous, *A Dictionary of Chinese Buddhist Terms*

**Source:** Digitised by Charles Muller (Dharma Drum Buddhist College / DDB).
**Authors:** William Edward Soothill, Lewis Hodous (original, 1937);
            Charles Muller (digital edition, 2003–2008).
**License:** Creative Commons (as stated in the TEI preface).

SinoRAG embeds a preprocessed copy of the Soothill-Hodous dictionary
(`soothill.json`, derived from `ddbc.soothill-hodous.tei.p5.xml` obtained
via the cbeta-reader project). It is used to annotate tool responses with
Buddhist term definitions, helping calling models correctly interpret
Chinese Buddhist terminology.

---

## CBETA (Chinese Buddhist Electronic Text Association)

**Source:** <https://www.cbeta.org>, <https://github.com/cbeta-org>
**License:** See CBETA's distribution terms at <https://www.cbeta.org/copyright.php>

The CBETA TEI/XML corpus (xml-p5, xml-iso distributions) is the primary source
data that SinoRAG indexes. SinoRAG does not redistribute the raw CBETA XML; the
distributed pack contains only derived passage records (parquet format) produced
by SinoRAG's ingest pipeline.
