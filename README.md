# SearchEngine

Search engine for Anything — fuzzy/regex/exact search with Everything-like query syntax.
Reads the binary `.anythingindex` produced by LibAnything. Exposes both a Rust API and a C ABI (`cdylib`).

## Public API

### SearchEngine

```rust
pub struct SearchEngine { /* records: Vec<FileRecord> */ }

impl SearchEngine {
    /// Load index from a .anythingindex file.
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, String>;

    /// Construct from in-memory records (e.g. partial indexing).
    pub fn from_records(records: Vec<FileRecord>) -> Self;

    /// Number of indexed entries.
    pub fn index_size(&self) -> usize;

    /// Run a search query.
    pub fn search(&self, query: &str, search_type: SearchType) -> Vec<&FileRecord>;

    /// Default ignore rules for the indexer (skip /proc, /sys, /tmp, etc.).
    pub fn default_ignore_config() -> libanything::IgnoreConfig;

    /// Load ignore rules from a YAML file.
    pub fn load_ignore_config_yaml(path: &Path) -> Result<libanything::IgnoreConfig, String>;
}
```

### SearchType

```rust
pub enum SearchType {
    Fuzzy,  // skim-based fuzzy matching (score > 40)
    Regex,  // regex match on full path
    Exact,  // case-insensitive substring match
}
```

## Query Syntax

| Pattern       | Example              | Description                    |
|---------------|----------------------|--------------------------------|
| `word`        | `report`             | Fuzzy match (include term)     |
| `!term`       | `!tmp`               | Exclude files containing term  |
| `"phrase"`    | `"annual report"`    | Exact substring match          |
| `ext:ext`     | `ext:pdf`            | Only files with extension      |
| `ext:!ext`    | `ext:!tmp`           | Exclude files with extension   |
| `path:dir`    | `path:/home`         | Only files under path          |
| `path:!dir`   | `path:!/proc`        | Exclude path                   |
| `*wild*`      | `*report*`           | Wildcard (split on `*`)        |

Noise filter (always applied): `thumbs.db`, `desktop.ini`, `.DS_Store`, `*.tmp`, `*.temp`, `*.bak`, files ending in `~`.

## C FFI

The library compiles as `cdylib` for use from non-Rust languages.

```c
/// Load index from .anythingindex file. Returns 0 on success, -1 on error.
int load_index_from_file(const char *path);

/// Search with given type (0=Fuzzy, 1=Regex, 2=Exact). Returns result count.
uint64_t search_query(const char *query, int32_t search_type);

/// Get result path by index (0-based). Returns null if out of range.
/// Caller must free with free_c_string().
char *get_result_by_index(uint64_t idx);

/// Number of indexed entries.
uint64_t index_size(void);

/// Free a C string allocated by the library.
void free_c_string(char *ptr);
```

## Usage (Rust)

```rust
use searchengine::{SearchEngine, SearchType};

let engine = SearchEngine::load("/home/user/.config/anything-index.anythingindex")
    .expect("load index");

println!("Index size: {}", engine.index_size());

let results = engine.search("ext:pdf report", SearchType::Fuzzy);
for r in &results {
    println!("{}", r.name);
}
```

## Tests

```sh
cargo test
```

9 unit tests covering: fuzzy search, regex search, exclude filter, ext filter,
ext exclude, noise filter, exact phrase, empty query, binary index load, default ignore config.

## Dependencies

- `fuzzy-matcher` — Skim-based fuzzy matching
- `regex` — regex search mode
- `serde` + `serde_yaml` — YAML ignore config parsing
- `libanything` — filesystem indexer + binary format
