# MCP Server Concurrency Analysis

## Current State

### Parallel Tool Call Support

**Status: IMPROVED** (as of latest changes)

The SinoRAGD MCP server now supports parallel tool calls within a single server instance with WAL mode enabled for SQLite, significantly reducing contention issues.

## Architecture Analysis

### DataFusionStore (Concurrent-Safe)

**Implementation:**
- Wrapped in `Arc<OnceCell<DataFusionStore>>` for single initialization
- Uses DataFusion's `SessionContext` which is designed for concurrent query execution
- All methods are `async fn` taking `&self` (shared reference)
- No internal mutable state requiring exclusive access

**Concurrency:**
- ✅ **Thread-safe** for concurrent reads
- ✅ DataFusion SessionContext handles concurrent queries natively
- ✅ Parquet files are read-only, no file contention
- ✅ Single initialization pattern prevents race conditions

**Recommendation:** No changes needed. DataFusion is well-designed for concurrency.

### Registry SQLite (Improved with WAL)

**Implementation:**
- Opens new connection per operation: `Connection::open(db_path)`
- ✅ **WAL mode enabled** in `init_registry()`
- ✅ **Busy timeout set to 5 seconds** on all connections
- No connection pooling (considered for future)
- Uses transactions for batch operations

**Concurrency:**
- ✅ **Concurrent reader support** with WAL mode enabled
- ✅ Busy timeout handles contention gracefully
- ⚠️ Each call opens/closes a connection → overhead (acceptable for current load)
- ⚠️ Still not safe for multiple server instances writing to same file
- ✅ **Safe for parallel reads** within single instance

**Current MCP tools using registry:**
- `prior_work` - read-only query
- `phrase_status` - read-only query
- `work_summary` - read-only query

**Recommendation:** Enable WAL mode and connection pooling for better concurrency.

### Tool Methods

All tool methods are:
```rust
pub async fn tool_name(&self, ...) -> Result<Json<Value>, String>
```

- ✅ Take `&self` (shared reference)
- ✅ Are async, allowing tokio to schedule concurrently
- ✅ Server struct derives Clone, can be cloned if needed

**Limitation:** The MCP protocol over stdio is a single client-server connection. While the server can handle concurrent requests internally, the stdio transport may serialize requests depending on the client implementation.

## Concurrency Scenarios

### Scenario 1: Single Server Instance, Parallel Tool Calls (5+ searches)

**Expected Behavior:**
- DataFusion queries: ✅ Will run concurrently
- Registry queries: ⚠️ May contend on SQLite file locks
- Overall: ✅ Should work but may have SQLite contention

**Bottleneck:** SQLite file locking under high concurrent read load.

### Scenario 2: Multiple Server Instances, Same Data Files

**Expected Behavior:**
- DataFusion (Parquet): ✅ No issue (read-only files)
- Registry SQLite: ❌ **Will fail** - multiple processes writing to same SQLite file without coordination

**Bottleneck:** SQLite file locking across processes.

### Scenario 3: Daisy-Chain Calls (Model calls tool, uses result to call another)

**Expected Behavior:**
- ✅ Works as designed
- Sequential in the model's reasoning, but tools can execute concurrently if model requests it

## Recommended Improvements

### ✅ 1. Enable SQLite WAL Mode (COMPLETED)

**Status:** Implemented in `init_registry()` and all connection opens.

**Changes made:**
- Added `PRAGMA journal_mode=WAL` in `init_registry()`
- Added `PRAGMA synchronous=NORMAL` for better performance
- Added `PRAGMA busy_timeout=5000` (5 seconds) in all connection opens
- Applied to: `init_registry()`, `upsert_items_batch()`, `prior_work()`, `phrase_status()`, `work_summary()`

**Benefits:**
- ✅ Allows multiple concurrent readers
- ✅ Readers don't block writers
- ✅ Better performance under load
- ✅ Graceful handling of contention with busy timeout

### 2. Add SQLite Connection Pool (Optional for Future)

**Status:** Not yet implemented. Current load doesn't require it, but consider if high load is expected.

**Recommended approach:** Use `r2d2` or `deadpool` for connection pooling to reduce open/close overhead under high load.

**Benefits:**
- Reuses connections
- Reduces open/close overhead
- Better resource management

**Decision:** defer until performance testing shows connection overhead is a bottleneck.

### 3. Add Concurrency Tests (Recommended for Future)

**Status:** Not yet implemented. Recommended for validation.

Add tests to verify parallel tool calls work under load.

### ✅ 4. Document Concurrency Limits in MCP Instructions (COMPLETED)

**Status:** Added to `get_mcp_instructions()` in server.rs.

**Added documentation:**
- Batch independent queries when possible
- Avoid daisy-chaining more than 3-4 sequential tool calls
- Retry with exponential backoff on timeouts
- Single server instance only note
- Explanation of DataFusion vs SQLite backend

## Current Limitations Summary

| Component | Concurrency | Multi-Process Safe | Bottleneck |
|-----------|-------------|-------------------|------------|
| DataFusionStore | ✅ High | ✅ Yes | None |
| Registry SQLite | ✅ Improved (WAL) | ⚠️ Limited | Connection overhead |
| MCP Stdio Transport | ⚠️ Client-dependent | N/A | Serialization |
| Tool Methods | ✅ Concurrent | N/A | None |

## Testing Recommendations

1. **Load Test:** Run 10+ concurrent search calls and measure latency
2. **Contention Test:** Run 5+ concurrent registry queries and check for timeouts
3. **Multi-Process Test:** Attempt to run 2 server instances with same registry (expected to fail)

## Architecture: Why SQLite + DataFusion Instead of Just DuckDB?

### Current Architecture

The Rust port uses a **two-tier architecture**:

1. **DataFusion + Parquet** (main data store):
   - Used for: `search`, `passage`, `first_attestation`, `phrase_history`, `frontier`
   - Stores: Passage data in columnar Parquet format
   - Why DataFusion: Excellent for analytical queries on large datasets, native Parquet support, Rust-native

2. **SQLite** (metadata registry):
   - Used for: `prior_work`, `phrase_status`, `work_summary`
   - Stores: Research tracking metadata (work items, phrase observations, seed observations)
   - Why SQLite: Lightweight, embedded, ACID compliance, perfect for small transactional metadata

### Why Not DuckDB?

The Python version may have used DuckDB for everything, but the Rust port made a deliberate architectural choice:

**DuckDB advantages:**
- Single database for everything
- Good analytical performance
- Python integration

**Why DataFusion + SQLite is better for Rust:**
- **DataFusion** is more mature in the Rust ecosystem for Parquet analytics
- **Separation of concerns**: Analytical queries (DataFusion) vs transactional metadata (SQLite)
- **Best tool for the job**: Columnar storage for passages, relational for metadata
- **Rust-native**: Both crates have excellent Rust support

### DuckDB Alternative (Considered but Not Chosen)

DuckDB could replace SQLite for the registry, but:
- DuckDB is overkill for small metadata tables
- SQLite is more battle-tested for embedded use
- SQLite with WAL mode provides sufficient concurrency for registry use case

**Recommendation:** Keep current architecture. Consider DuckDB only if registry grows to analytical workloads (unlikely).

## Conclusion

The current implementation now supports parallel tool calls within a single server instance with WAL mode enabled for SQLite. The improvements made (WAL mode, busy timeout) significantly reduce contention risks for 5+ parallel tool calls.

**Completed improvements:**
- ✅ SQLite WAL mode enabled
- ✅ Busy timeout (5 seconds) on all connections
- ✅ MCP instructions updated with concurrency notes

**Optional future improvements:**
- Connection pooling if connection overhead becomes a bottleneck
- Concurrency tests for validation
- Consider DuckDB for registry if it grows to analytical scale

**Priority:** Current implementation is production-ready for expected workloads. Monitor performance and add connection pooling only if needed.
