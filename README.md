# SearchEngine

Search engine for Anything — fuzzy/regex search + Everything-like query syntax.

Reads the YAML index produced by LibAnything (`~/.config/anything-index.yaml`).
Exposes a pure C ABI (`cdylib`) for FFI from any language.

## Build

```sh
cargo build --release
```

Output: `target/release/libsearchengine.{dll,so,dylib}` (includes LibAnything).

## Query Syntax

| Pattern | Example | Description |
|---------|---------|-------------|
| `word` | `report` | Fuzzy match (include) |
| `!term` | `!tmp` | Exclude files containing term |
| `"phrase"` | `"annual report"` | Exact substring match |
| `ext:ext` | `ext:pdf` | Only files with extension |
| `ext:!ext` | `ext:!tmp` | Exclude files with extension |
| `path:dir` | `path:/home` | Only files under path |
| `path:!dir` | `path:!/proc` | Exclude path |
| `*wild*` | `*report*` | Wildcard (split on `*`) |
| Regex mode | `\.(pdf\|jpg)$` | Set at FFI call site |

Noise filter (always applied): `thumbs.db`, `desktop.ini`, `.DS_Store`, `*.tmp`, `*.temp`, `*.bak`, `*~`, files in `%TEMP%`.

## FFI

| Function | Description |
|----------|-------------|
| `load_index_from_file(path)` | Load index from YAML file |
| `search_query(query, type)` | Search (0 = Fuzzy, 1 = Regex, 2 = Exact) |
| `get_result_by_index(idx)` | Get result path by position |
| `index_size()` | Number of indexed files |
| `free_c_string(ptr)` | Free a C string allocated by Rust |

## Tests

```sh
cargo test
```

9 unit tests covering: empty query, fuzzy, regex, exclude, ext filter, ext exclude, noise filter, exact phrase, YAML load.

## Dependencies

- `fuzzy-matcher` — fuzzy string matching
- `regex` — regex search mode
- `once_cell` — lazy statics
- `log` — logging facade
- `serde` + `serde_yaml` — YAML index serialization
- `libanything` — filesystem indexer (walks `/`, writes YAML)
