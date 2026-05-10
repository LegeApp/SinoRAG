SinoRAG is a CLI tool for indexing and searching Chinese Buddhist texts, with support for phrase search, TF-IDF similarity, and hierarchical catalog navigation.

## Setup

1. **Install**: `cargo install --path .`

2. **Ingest CBETA corpus** (includes phrase index, TF-IDF, catalog index):
   ```
   graphdiscovery ingest --corpus /path/to/cbeta-xml --out data/passages.parquet
   ```
   This creates `data/passages.parquet`, `derived/phrase.index`, `derived/tfidf.index`, and `derived/catalog.index`.

3. **Ingest Kanripo** (optional):
   ```
   graphdiscovery ingest --kanripo_input /path/to/kr --out data/passages.parquet
   ```

4. **Add custom corpus** (CEF format):
   ```
   graphdiscovery cef-init --out mycorpus.cef
   # Edit mycorpus.cef, then:
   graphdiscovery ingest-cef --input mycorpus.cef --out_parquet data/passages.parquet
   ```

5. **Search**:
   ```
   graphdiscovery search --phrase "如是我聞"
   graphdiscovery phrase-index-search --phrase "如是我聞"
   graphdiscovery similar --seed "佛說"
   ```