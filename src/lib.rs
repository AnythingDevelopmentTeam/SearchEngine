use std::ffi::{c_char, CStr, CString};
use std::sync::RwLock;
use std::path::Path;

use fuzzy_matcher::FuzzyMatcher;
use once_cell::sync::Lazy;
use regex::Regex;

// ──────────────────────────────────────────────────────────────────────────────
// Структуры данных
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FileRecord {
    pub id: u64,
    pub parent_id: u64,
    pub name: String,
}

#[derive(Debug, Default)]
struct ParsedQuery {
    include_terms: Vec<String>,
    exclude_terms: Vec<String>,
    exact_phrases: Vec<String>,
    ext_filters: Vec<(String, bool)>,
    path_filters: Vec<(String, bool)>,
}

// ──────────────────────────────────────────────────────────────────────────────
// Глобальное состояние
// ──────────────────────────────────────────────────────────────────────────────

static INDEX: Lazy<RwLock<Vec<FileRecord>>> = Lazy::new(|| RwLock::new(Vec::new()));
static LAST_RESULTS: Lazy<RwLock<Vec<u64>>> = Lazy::new(|| RwLock::new(Vec::new()));

// ──────────────────────────────────────────────────────────────────────────────
// Cached temp‑directory paths for noise filter
// ──────────────────────────────────────────────────────────────────────────────

static TEMP_PATHS: Lazy<Vec<String>> = Lazy::new(|| {
    let mut paths = Vec::new();
    for var in &["TEMP", "TMP", "LOCALAPPDATA", "USERPROFILE"] {
        if let Ok(val) = std::env::var(var) {
            let lower = val.to_lowercase();
            match *var {
                "LOCALAPPDATA" => paths.push(format!("{}\\temp", lower)),
                "USERPROFILE" => paths.push(format!("{}\\appdata\\local\\temp", lower)),
                _ => paths.push(lower),
            }
        }
    }
    paths
});

// ──────────────────────────────────────────────────────────────────────────────
// Утилиты
// ──────────────────────────────────────────────────────────────────────────────

unsafe fn cstr_to_str<'a>(ptr: *const c_char) -> &'a str {
    if ptr.is_null() { return ""; }
    CStr::from_ptr(ptr).to_str().unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn free_c_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        unsafe { let _ = CString::from_raw(ptr); }
    }
}

fn str_to_cstr_owned(s: &str) -> *mut c_char {
    CString::new(s).expect("CString::new failed").into_raw()
}

// ──────────────────────────────────────────────────────────────────────────────
// Парсинг запроса (Everything‑like синтаксис)
// ──────────────────────────────────────────────────────────────────────────────

fn tokenize(raw: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;

    for c in raw.chars() {
        match c {
            '"' => { current.push(c); in_quote = !in_quote; }
            ' ' if !in_quote => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn parse_query(raw: &str) -> ParsedQuery {
    let mut q = ParsedQuery::default();
    for token in &tokenize(raw) {
        if let Some(rest) = token.strip_prefix("ext:") {
            let (exclude, val) = if let Some(v) = rest.strip_prefix('!') {
                (true, v)
            } else {
                (false, rest)
            };
            if !val.is_empty() {
                q.ext_filters.push((val.trim_start_matches('.').to_lowercase(), exclude));
            }
        } else if let Some(rest) = token.strip_prefix("path:") {
            let (exclude, val) = if let Some(v) = rest.strip_prefix('!') {
                (true, v)
            } else {
                (false, rest)
            };
            if !val.is_empty() {
                q.path_filters.push((val.to_lowercase(), exclude));
            }
        } else if let Some(rest) = token.strip_prefix('!') {
            if !rest.is_empty() {
                q.exclude_terms.push(rest.to_lowercase());
            }
        } else if token.starts_with('"') && token.ends_with('"') && token.len() > 1 {
            q.exact_phrases.push(token[1..token.len()-1].to_lowercase());
        } else {
            q.include_terms.push(token.to_lowercase());
        }
    }
    q
}

fn build_search_string(q: &ParsedQuery) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for t in &q.include_terms {
        if !t.contains('*') { parts.push(t); }
    }
    for p in &q.exact_phrases { parts.push(p); }
    parts.join(" ")
}

fn has_any_positive_term(q: &ParsedQuery) -> bool {
    !q.include_terms.is_empty() || !q.exact_phrases.is_empty()
}

// ──────────────────────────────────────────────────────────────────────────────
// Фильтры (пост‑обработка)
// ──────────────────────────────────────────────────────────────────────────────

fn contains_exclude_terms(lower_path: &str, q: &ParsedQuery) -> bool {
    q.exclude_terms.iter().any(|t| lower_path.contains(t))
}

fn matches_exact_phrases(lower_path: &str, q: &ParsedQuery) -> bool {
    q.exact_phrases.iter().all(|p| lower_path.contains(p))
}

fn matches_ext_filters(lower_path: &str, q: &ParsedQuery) -> bool {
    if q.ext_filters.is_empty() { return true; }
    let ext = Path::new(lower_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    for (filter_ext, exclude) in &q.ext_filters {
        let match_ext = ext == *filter_ext;
        if *exclude && match_ext { return false; }
        if !*exclude && !match_ext { return false; }
    }
    true
}

fn matches_path_filters(lower_path: &str, q: &ParsedQuery) -> bool {
    if q.path_filters.is_empty() { return true; }
    for (filter_path, exclude) in &q.path_filters {
        let match_path = lower_path.starts_with(filter_path);
        if *exclude && match_path { return false; }
        if !*exclude && !match_path { return false; }
    }
    true
}

fn matches_wildcard_terms(lower_name: &str, q: &ParsedQuery) -> bool {
    for term in &q.include_terms {
        if !term.contains('*') { continue; }
        let parts: Vec<&str> = term.split('*').filter(|p| !p.is_empty()).collect();
        if parts.is_empty() { continue; }

        let mut pos = 0;
        for (i, part) in parts.iter().enumerate() {
            match lower_name[pos..].find(part) {
                Some(idx) => {
                    if i == 0 && !term.starts_with('*') && idx != 0 { return false; }
                    pos += idx + part.len();
                }
                None => return false,
            }
        }
        if !term.ends_with('*') && pos < lower_name.len() { return false; }
    }
    true
}

fn is_noise_file(lower_path: &str) -> bool {
    let name = match Path::new(lower_path).file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return false,
    };
    let lower_name = name.to_lowercase();

    // Known OS / editor noise files
    if matches!(
        lower_name.as_str(),
        "thumbs.db" | "desktop.ini" | ".ds_store" | "icon\r"
    ) {
        return true;
    }

    if lower_name.ends_with('~') { return true; }

    if let Some(ext) = Path::new(name).extension().and_then(|e| e.to_str()) {
        let ext_lower = ext.to_lowercase();
        if matches!(ext_lower.as_str(), "tmp" | "temp" | "bak") {
            return true;
        }
        // Numeric‑only names with .tmp (VS Code temp files)
        if ext_lower == "tmp" {
            let stem = Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if stem.len() > 4 && stem.chars().all(|c| c.is_ascii_digit()) {
                return true;
            }
        }
    }

    // Files in system temp directories
    for temp_path in TEMP_PATHS.iter() {
        if lower_path.starts_with(temp_path) {
            return true;
        }
    }

    false
}

fn passes_all_filters(record: &FileRecord, q: &ParsedQuery) -> bool {
    let lower_path = record.name.to_lowercase();
    let lower_name = Path::new(&record.name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();

    // Noise filter (skip if matches noise pattern)
    if is_noise_file(&lower_path) {
        return false;
    }

    // Exclusion terms
    if contains_exclude_terms(&lower_path, q) { return false; }

    // Exact phrases
    if !matches_exact_phrases(&lower_path, q) { return false; }

    // Extension filters
    if !matches_ext_filters(&lower_path, q) { return false; }

    // Path filters
    if !matches_path_filters(&lower_path, q) { return false; }

    // Wildcard terms
    if !matches_wildcard_terms(&lower_name, q) { return false; }

    true
}

// ──────────────────────────────────────────────────────────────────────────────
// FFI: загрузка индекса
// ──────────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct FileRecordFFI {
    pub id: u64,
    pub parent_id: u64,
    pub name: *const c_char,
}

#[no_mangle]
pub extern "C" fn load_index(records: *const FileRecordFFI, count: u64) -> i32 {
    if records.is_null() { log::error!("load_index: records is null"); return -1; }
    let slice = unsafe { std::slice::from_raw_parts(records, count as usize) };
    let mut vector: Vec<FileRecord> = Vec::with_capacity(slice.len());
    for rec in slice {
        let name = unsafe { cstr_to_str(rec.name) };
        vector.push(FileRecord { id: rec.id, parent_id: rec.parent_id, name: name.to_string() });
    }
    if let Ok(mut idx) = INDEX.write() { *idx = vector; } else { return -1; }
    0
}

#[no_mangle]
pub extern "C" fn build_index(roots: *const *const c_char, count: u64) -> i32 {
    if roots.is_null() { log::error!("build_index: roots is null"); return -1; }
    let root_slice = unsafe { std::slice::from_raw_parts(roots, count as usize) };
    let root_strs: Vec<String> = root_slice
        .iter().map(|&p| unsafe { cstr_to_str(p) }.to_string()).collect();
    let root_refs: Vec<&str> = root_strs.iter().map(|s| s.as_str()).collect();

    let paths = libanything::scan_directories(&root_refs);
    if paths.is_empty() { log::warn!("build_index: no files found"); return -1; }

    let records: Vec<FileRecord> = paths.into_iter().enumerate().map(|(i, name)| {
        FileRecord { id: (i + 1) as u64, parent_id: 0, name }
    }).collect();

    if let Ok(mut idx) = INDEX.write() { *idx = records; } else { return -1; }
    if let Ok(mut results) = LAST_RESULTS.write() { results.clear(); }
    0
}

// ──────────────────────────────────────────────────────────────────────────────
// FFI: поиск (Everything‑like синтаксис + фильтры встроены)
// ──────────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub enum SearchType { Fuzzy = 0, Regex = 1 }

fn do_search(query_str: &str, search_type: SearchType) -> u64 {
    if query_str.is_empty() {
        if let Ok(mut results) = LAST_RESULTS.write() { results.clear(); }
        return 0;
    }

    let parsed = parse_query(query_str);
    let index = match INDEX.read() { Ok(idx) => idx, Err(_) => return 0 };
    if index.is_empty() { log::warn!("search_query: index is empty"); return 0; }

    // Step 1: get candidate ids (fuzzy/regex match or all)
    let candidate_ids: Vec<u64> = if has_any_positive_term(&parsed) {
        let search_str = build_search_string(&parsed);
        match search_type {
            SearchType::Regex => {
                let re = match Regex::new(&search_str) { Ok(r) => r, Err(_) => return 0 };
                index.iter().filter(|r| re.is_match(&r.name)).map(|r| r.id).collect()
            }
            SearchType::Fuzzy => {
                let matcher = fuzzy_matcher::skim::SkimMatcherV2::default();
                index.iter()
                    .filter(|r| matcher.fuzzy_match(&r.name, &search_str).is_some())
                    .map(|r| r.id)
                    .collect()
            }
        }
    } else {
        // Only filters / exclusions — start with all items
        index.iter().map(|r| r.id).collect()
    };

    // Step 2: apply all filters
    let filtered: Vec<u64> = candidate_ids.into_iter()
        .filter(|&id| {
            index.iter().find(|r| r.id == id).is_some_and(|r| passes_all_filters(r, &parsed))
        })
        .collect();

    let count = filtered.len() as u64;
    if let Ok(mut results) = LAST_RESULTS.write() { *results = filtered; }
    count
}

#[no_mangle]
pub extern "C" fn search_query(query: *const c_char, search_type: SearchType) -> u64 {
    let query_str = unsafe { cstr_to_str(query) };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        do_search(query_str, search_type)
    })).unwrap_or(0)
}

// ──────────────────────────────────────────────────────────────────────────────
// FFI: получение результатов
// ──────────────────────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn get_result_by_index(idx: u64) -> *mut c_char {
    let results = LAST_RESULTS.read().unwrap();
    let Some(&record_id) = results.get(idx as usize) else {
        return std::ptr::null_mut();
    };
    drop(results);
    let index = INDEX.read().unwrap();
    let record = match index.iter().find(|r| r.id == record_id) {
        Some(r) => r, None => return std::ptr::null_mut(),
    };
    str_to_cstr_owned(&record.name)
}

#[no_mangle]
pub extern "C" fn index_size() -> u64 {
    INDEX.read().unwrap().len() as u64
}

#[no_mangle]
pub extern "C" fn last_results_count() -> u64 {
    LAST_RESULTS.read().unwrap().len() as u64
}

// ──────────────────────────────────────────────────────────────────────────────
// Тесты
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn build_test_data() -> Vec<FileRecordFFI> {
        let names = [
            "/home/user/documents/report.pdf",
            "/home/user/documents/photo.jpg",
            "/home/user/music/song.mp3",
            "/home/user/videos/movie.mp4",
            "/home/user/documents/notes.txt",
            "/home/user/temp/12345.tmp",
            "/home/user/temp/notes.tmp",
        ];
        names.iter().enumerate().map(|(i, name)| {
            let c_name = CString::new(*name).unwrap();
            FileRecordFFI { id: i as u64 + 1, parent_id: 0, name: c_name.into_raw() }
        }).collect()
    }

    fn free_test_data(data: &[FileRecordFFI]) {
        for rec in data {
            if !rec.name.is_null() {
                unsafe { let _ = CString::from_raw(rec.name as *mut c_char); }
            }
        }
    }

    #[test]
    fn test_fuzzy_search() {
        let data = build_test_data();
        load_index(data.as_ptr(), data.len() as u64);
        let query = CString::new("repo").unwrap();
        let count = search_query(query.as_ptr(), SearchType::Fuzzy);
        assert!(count > 0);
        let ptr = get_result_by_index(0);
        assert!(!ptr.is_null());
        free_c_string(ptr);
        free_test_data(&data);
    }

    #[test]
    fn test_regex_search() {
        let data = build_test_data();
        load_index(data.as_ptr(), data.len() as u64);
        let query = CString::new(r"\.(pdf|jpg)$").unwrap();
        assert_eq!(search_query(query.as_ptr(), SearchType::Regex), 2);
        free_test_data(&data);
    }

    #[test]
    fn test_exclude_filter() {
        let data = build_test_data();
        load_index(data.as_ptr(), data.len() as u64);
        let query = CString::new("doc !photo").unwrap();
        let count = search_query(query.as_ptr(), SearchType::Fuzzy);
        // "doc" matches 3 files in "documents/", !photo excludes photo.jpg
        assert_eq!(count, 2);
        free_test_data(&data);
    }

    #[test]
    fn test_ext_filter() {
        let data = build_test_data();
        load_index(data.as_ptr(), data.len() as u64);
        let query = CString::new("ext:pdf").unwrap();
        let count = search_query(query.as_ptr(), SearchType::Fuzzy);
        assert_eq!(count, 1);
        free_test_data(&data);
    }

    #[test]
    fn test_ext_exclude_filter() {
        let data = build_test_data();
        load_index(data.as_ptr(), data.len() as u64);
        let query = CString::new("ext:!tmp").unwrap();
        let count = search_query(query.as_ptr(), SearchType::Fuzzy);
        assert_eq!(count, 5);
        free_test_data(&data);
    }

    #[test]
    fn test_noise_filter_skips_numeric_tmp() {
        let data = build_test_data();
        load_index(data.as_ptr(), data.len() as u64);
        let query = CString::new("12345").unwrap();
        let count = search_query(query.as_ptr(), SearchType::Fuzzy);
        assert_eq!(count, 0);
        free_test_data(&data);
    }

    #[test]
    fn test_exact_phrase() {
        let data = build_test_data();
        load_index(data.as_ptr(), data.len() as u64);
        let query = CString::new("\"report.pdf\"").unwrap();
        let count = search_query(query.as_ptr(), SearchType::Fuzzy);
        assert_eq!(count, 1);
        free_test_data(&data);
    }

    #[test]
    fn test_empty_query() {
        let data = build_test_data();
        load_index(data.as_ptr(), data.len() as u64);
        let query = CString::new("").unwrap();
        assert_eq!(search_query(query.as_ptr(), SearchType::Fuzzy), 0);
        free_test_data(&data);
    }
}
