# SearchEngine

Search engine for Anything — fuzzy/regex search + Everything-like query syntax.

Built on top of `libanything` for filesystem scanning. Exposes a pure C ABI (`cdylib`) for FFI from any language (Qt, Python, etc.).

## Build

```sh
cargo build --release
```

Output: `target/release/searchengine.{dll,so,dylib}`

## Query Syntax

| Pattern | Example | Description |
|---------|---------|-------------|
| `word` | `report` | Fuzzy match (include) |
| `!term` | `!tmp` | Exclude files containing term |
| `"phrase"` | `"annual report"` | Exact substring match |
| `ext:ext` | `ext:pdf` | Only files with extension |
| `ext:!ext` | `ext:!tmp` | Exclude files with extension |
| `path:dir` | `path:C:\Docs` | Only files under path |
| `path:!dir` | `path:!C:\Windows` | Exclude path |
| `*wild*` | `*report*` | Wildcard (split on `*`) |
| Regex mode | `\.(pdf\|jpg)$` | Set at FFI call site |

Noise filter (always applied): `thumbs.db`, `desktop.ini`, `.DS_Store`, `*.tmp`, `*.temp`, `*.bak`, `*~`, files in `%TEMP%`.

## FFI

| Function | Description |
|----------|-------------|
| `build_index(roots, count)` | Scan directories via LibAnything, build in-memory index |
| `load_index(records, count)` | Load pre-built index from memory |
| `search_query(query, type)` | Search (0 = Fuzzy, 1 = Regex) |
| `get_result_by_index(idx)` | Get result path by position |
| `index_size()` | Number of indexed files |
| `last_results_count()` | Number of last search results |
| `free_c_string(ptr)` | Free a C string allocated by Rust |

## Tests

```sh
cargo test
```

8 unit tests covering: empty query, fuzzy, regex, exclude, ext filter, ext exclude, noise filter, exact phrase.

## Dependencies

- `fuzzy-matcher` — fuzzy string matching
- `regex` — regex search mode
- `once_cell` — lazy statics
- `log` — logging facade
- `libanything` — filesystem indexer
