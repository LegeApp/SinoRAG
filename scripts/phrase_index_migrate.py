#!/usr/bin/env python3
"""
PhraseIndex v1 to v2 Migration Tool (Pure Python)

Converts a v1 PhraseIndex (bincode) to v2 format (mmap-friendly binary).
Does NOT require the Rust binary - pure Python implementation.

Usage:
    python phrase_index_migrate.py \
        --input /path/to/phrase.index \
        --doc-table /path/to/doc_table.bin \
        --output /path/to/phrase_v2.index

Requirements:
    pip install xxhash
"""

import argparse
import os
import struct
import sys

try:
    import xxhash
except ImportError:
    print("Error: xxhash package not found", file=sys.stderr)
    print("Install with: pip install xxhash", file=sys.stderr)
    sys.exit(1)

# Constants
MAGIC_V2 = b'SGV2'
HEADER_SIZE = 256
GRAM_ENTRY_SIZE = 20

def varint_encode(value: int) -> bytes:
    """Encode an integer as varint (positive only)."""
    if value < 0:
        raise ValueError("Varint encoding requires non-negative integers")
    result = []
    while value >= 0x80:
        result.append((value & 0x7F) | 0x80)
        value >>= 7
    result.append(value)
    return bytes(result)

def varint_decode(data: bytes, pos: int) -> tuple[int, int]:
    """Decode a varint, return (value, bytes_consumed)."""
    result = 0
    shift = 0
    consumed = 0
    while pos < len(data):
        byte = data[pos + consumed]
        consumed += 1
        result |= (byte & 0x7F) << shift
        if byte & 0x80 == 0:
            return result, consumed
        shift += 7
        if shift >= 64:
            raise ValueError("Varint overflow")
    raise ValueError("Truncated varint")

def encode_sorted_docids(doc_ids: list) -> bytes:
    """Encode sorted DocIds with delta-varint."""
    if not doc_ids:
        return b''
    result = []
    prev = doc_ids[0]
    result.append(varint_encode(prev))
    for doc_id in doc_ids[1:]:
        delta = doc_id - prev
        result.append(varint_encode(delta))
        prev = doc_id
    return b''.join(result)

def read_varint(data: bytes, pos: int) -> tuple[int, int]:
    """Read a varint from binary data."""
    result = 0
    shift = 0
    consumed = 0
    while pos < len(data):
        byte = data[pos]
        consumed += 1
        result |= (byte & 0x7F) << shift
        if byte & 0x80 == 0:
            return result, consumed
        pos += 1
        shift += 7
    raise ValueError("Truncated varint")

def read_string(data: bytes, pos: int) -> tuple[str, int]:
    """Read a length-prefixed string (bincode format)."""
    length, consumed = read_varint(data, pos)
    pos += consumed
    if pos + length > len(data):
        raise ValueError(f"String extends past end: length={length}, pos={pos}, len={len(data)}")
    string = data[pos:pos+length].decode('utf-8')
    pos += length
    return string, pos

def read_vec(data: bytes, pos: int) -> tuple[list, int]:
    """Read a length-prefixed vec."""
    length, consumed = read_varint(data, pos)
    pos += consumed
    return [], pos  # Placeholder - will be specialized per type

def read_string_vec(data: bytes, pos: int) -> tuple[list, int]:
    """Read a Vec<String>."""
    length, consumed = read_varint(data, pos)
    pos += consumed

    strings = []
    for _ in range(length):
        s, pos = read_string(data, pos)
        strings.append(s)
    return strings, pos

def read_u32_vec(data: bytes, pos: int) -> tuple[list, int]:
    """Read a Vec<u32>."""
    length, consumed = read_varint(data, pos)
    pos += consumed

    values = []
    for _ in range(length):
        if pos + 4 > len(data):
            raise ValueError("Truncated u32 vec")
        value = struct.unpack('<I', data[pos:pos+4])[0]
        values.append(value)
        pos += 4
    return values, pos

def read_option_string(data: bytes, pos: int) -> tuple[str | None, int]:
    """Read an Option<String>."""
    if pos >= len(data):
        return None, pos
    tag = data[pos]
    pos += 1
    if tag == 0:
        return None, pos
    elif tag == 1:
        return read_string(data, pos)
    else:
        raise ValueError(f"Invalid option tag: {tag}")

def parse_v1_postings(data: bytes, start_pos: int) -> dict:
    """
    Parse v1 PhraseIndex postings from bincode data.
    postings: FxHashMap<String, Vec<DocId>>
    """
    pos = start_pos
    length, consumed = read_varint(data, pos)
    pos += consumed

    postings = {}
    for _ in range(length):
        # Read key (String)
        key, pos = read_string(data, pos)

        # Read value (Vec<DocId>)
        val_length, val_consumed = read_varint(data, pos)
        pos += val_consumed

        doc_ids = []
        for _ in range(val_length):
            if pos + 4 > len(data):
                raise ValueError("Truncated DocId")
            doc_id = struct.unpack('<I', data[pos:pos+4])[0]
            doc_ids.append(doc_id)
            pos += 4

        postings[key] = sorted(set(doc_ids))

    return postings

def load_v1_index(path: str) -> dict:
    """Load v1 PhraseIndex from bincode file."""
    with open(path, 'rb') as f:
        data = f.read()

    pos = 0

    # Read schema: String
    schema, pos = read_string(data, pos)

    # Read gram_len: usize (varint)
    gram_len, consumed = read_varint(data, pos)
    pos += consumed

    # Read passage_ids: Vec<String>
    passage_ids, pos = read_string_vec(data, pos)

    # Read postings: HashMap<String, Vec<DocId>>
    postings = parse_v1_postings(data, pos)

    # Read source_fingerprint: Option<String>
    source_fingerprint, _ = read_option_string(data, pos)

    return {
        'schema': schema,
        'gram_len': gram_len,
        'passage_ids': passage_ids,
        'postings': postings,
        'source_fingerprint': source_fingerprint,
    }

def load_doc_table(path: str) -> dict:
    """Load DocumentTable from bincode file."""
    with open(path, 'rb') as f:
        data = f.read()

    pos = 0

    # Read schema
    schema, pos = read_string(data, pos)

    # Read source_fingerprint
    source_fingerprint, pos = read_string(data, pos)

    # Read passage_ids
    passage_ids, pos = read_string_vec(data, pos)

    return {
        'schema': schema,
        'source_fingerprint': source_fingerprint,
        'passage_ids': passage_ids,
    }

def create_v2_index(v1_index: dict, doc_table: dict) -> bytes:
    """Create v2 binary format from v1 index and doc table."""

    # Build gram entries with xxhash
    gram_entries = []
    for gram in v1_index['postings'].keys():
        gram_hash = xxhash.xxh3_64(gram.encode('utf-8'))
        gram_entries.append({
            'gram': gram,
            'hash': gram_hash,
            'doc_ids': v1_index['postings'][gram],
        })

    # Sort by hash for binary search
    gram_entries.sort(key=lambda x: x['hash'])

    # Build postings blob with delta-varint
    postings_blob = bytearray()
    for entry in gram_entries:
        entry['offset'] = len(postings_blob)
        encoded = encode_sorted_docids(entry['doc_ids'])
        entry['length'] = len(encoded)
        postings_blob.extend(encoded)

    # Build header
    header = bytearray(HEADER_SIZE)

    # Magic (4 bytes)
    header[0:4] = MAGIC_V2

    # Version (2 bytes) - little endian
    header[4:6] = struct.pack('<H', 2)

    # gram_len (2 bytes)
    header[6:8] = struct.pack('<H', v1_index['gram_len'])

    # num_grams (4 bytes)
    header[8:12] = struct.pack('<I', len(gram_entries))

    # doc_table_fingerprint (64 bytes, padded)
    fp = doc_table['source_fingerprint'].encode('utf-8')
    header[12:12+len(fp)] = fp

    # schema (64 bytes, padded)
    schema_bytes = b'sinorag-phrase-index-v2'
    header[76:76+len(schema_bytes)] = schema_bytes

    # gram_table_offset (8 bytes)
    gram_table_offset = HEADER_SIZE
    header[140:148] = struct.pack('<Q', gram_table_offset)

    # gram_table_size (4 bytes)
    gram_table_size = len(gram_entries) * GRAM_ENTRY_SIZE
    header[148:152] = struct.pack('<I', gram_table_size)

    # postings_blob_size (8 bytes)
    postings_blob_size = len(postings_blob)
    header[152:160] = struct.pack('<Q', postings_blob_size)

    # Build gram table
    gram_table = bytearray()
    for entry in gram_entries:
        gram_table.extend(struct.pack('<Q', entry['hash']))
        gram_table.extend(struct.pack('<Q', entry['offset']))
        gram_table.extend(struct.pack('<I', entry['length']))

    return bytes(header) + bytes(gram_table) + bytes(postings_blob)

def migrate(input_path: str, doc_table_path: str, output_path: str):
    """Perform migration."""
    print(f"Loading v1 index from {input_path}...")
    v1_index = load_v1_index(input_path)
    print(f"  schema: {v1_index['schema']}")
    print(f"  gram_len: {v1_index['gram_len']}")
    print(f"  num_grams: {len(v1_index['postings'])}")

    print(f"Loading doc_table from {doc_table_path}...")
    doc_table = load_doc_table(doc_table_path)
    print(f"  num_passages: {len(doc_table['passage_ids'])}")

    print("Converting to v2 format...")
    v2_data = create_v2_index(v1_index, doc_table)
    print(f"  v2 size: {len(v2_data)} bytes")

    print(f"Writing to {output_path}...")
    os.makedirs(os.path.dirname(output_path) or '.', exist_ok=True)
    with open(output_path, 'wb') as f:
        f.write(v2_data)

    print(f"Migration complete: {output_path}")

def main():
    parser = argparse.ArgumentParser(
        description="Migrate PhraseIndex from v1 to v2 format (pure Python)"
    )
    parser.add_argument(
        "--input", "-i",
        required=True,
        help="Input v1 phrase index path"
    )
    parser.add_argument(
        "--doc-table", "-d",
        required=True,
        help="Document table path (doc_table.bin)"
    )
    parser.add_argument(
        "--output", "-o",
        required=True,
        help="Output v2 phrase index path"
    )
    parser.add_argument(
        "--verbose", "-v",
        action="store_true",
        help="Verbose output"
    )

    args = parser.parse_args()

    # Validate input files exist
    if not os.path.exists(args.input):
        print(f"Error: Input file not found: {args.input}", file=sys.stderr)
        sys.exit(1)

    if not os.path.exists(args.doc_table):
        print(f"Error: Document table not found: {args.doc_table}", file=sys.stderr)
        sys.exit(1)

    migrate(args.input, args.doc_table, args.output)

if __name__ == "__main__":
    main()