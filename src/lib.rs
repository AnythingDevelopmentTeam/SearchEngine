use std::ffi::{c_char, CStr, CString};
use std::path::Path;

use fuzzy_matcher::FuzzyMatcher;
use regex::Regex;
use libanything::FileRecord;

#[derive(Debug, Default)]
struct ParsedQuery {
    include_terms: Vec<String>,
    exclude_terms: Vec<String>,
    exact_phrases: Vec<String>,
    ext_filters: Vec<(String, bool)>,
    path_filters: Vec<(String, bool)>,
}

pub struct SearchEngine {
    records: Vec<FileRecord>,
}

impl SearchEngine {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let reader = libanything::IndexReader::open(path.as_ref())?;

        let records: Vec<FileRecord> = reader
            .entries
            .iter()
            .map(|e| FileRecord {
                id: e.id as u64,
                parent_id: e.parent_id as u64,
                name: reader.get_name(e).to_string(),
            })
            .collect();

        if !reader.changed_drives.is_empty() {
            log::warn!("Changed/missing drives: {:?}", reader.changed_drives);
        }

        log::info!("Loaded {} records from index", records.len());
        Ok(SearchEngine { records })
    }

    pub fn from_records(records: Vec<FileRecord>) -> Self {
        SearchEngine { records }
    }

    pub fn index_size(&self) -> usize {
        self.records.len()
    }

    pub fn default_ignore_config() -> libanything::IgnoreConfig {
        let mut skip_dirs: Vec<String> = vec![
            "/proc".into(),
            "/sys".into(),
            "/dev".into(),
            "/run".into(),
            "/snap".into(),
            "/lost+found".into(),
            "/tmp".into(),
            "/boot".into(),
            "/lib".into(),
            "/lib64".into(),
            "/usr/lib".into(),
            "/usr/lib64".into(),
            "/usr/share/zoneinfo".into(),
            "/usr/share/doc".into(),
            "/usr/share/help".into(),
            "/usr/share/man".into(),
            "/usr/include".into(),
            "/usr/src".into(),
            "/var/cache".into(),
            "/var/log".into(),
            "/var/tmp".into(),
            "/opt".into(),
            "/sysroot".into(),
            "/var/lib/docker".into(),
            "/var/lib/flatpak".into(),
        ];

        #[cfg(windows)]
        skip_dirs.extend([
            "C:\\Windows".into(),
            "C:\\Program Files".into(),
            "C:\\Program Files (x86)".into(),
            "C:\\ProgramData".into(),
            "C:\\Recovery".into(),
            "C:\\System Volume Information".into(),
            "C:\\$Recycle.Bin".into(),
            "C:\\MSOCache".into(),
            "C:\\PerfLogs".into(),
        ]);

        libanything::IgnoreConfig {
            skip_dir_prefixes: skip_dirs,
            skip_file_names: vec![
                "thumbs.db".into(),
                "desktop.ini".into(),
                ".ds_store".into(),
                "icon\r".into(),
            ],
            skip_file_exts: vec![
                "tmp".into(),
                "temp".into(),
                "bak".into(),
            ],
        }
    }

    pub fn load_ignore_config_yaml(path: &Path) -> Result<libanything::IgnoreConfig, String> {
        let content = std::fs::read_to_string(path).map_err(|e| format!("read: {}", e))?;
        #[derive(serde::Deserialize)]
        struct RawConfig {
            skip_dir_prefixes: Option<Vec<String>>,
            skip_file_names: Option<Vec<String>>,
            skip_file_exts: Option<Vec<String>>,
        }
        let raw: RawConfig = serde_yaml::from_str(&content).map_err(|e| format!("yaml: {}", e))?;
        let mut config = libanything::IgnoreConfig::new();
        if let Some(v) = raw.skip_dir_prefixes {
            config.skip_dir_prefixes = v;
        }
        if let Some(v) = raw.skip_file_names {
            config.skip_file_names = v;
        }
        if let Some(v) = raw.skip_file_exts {
            config.skip_file_exts = v;
        }
        Ok(config)
    }

    pub fn search(&self, query: &str, search_type: SearchType) -> Vec<&FileRecord> {
        if query.is_empty() {
            return Vec::new();
        }

        let parsed = parse_query(query);
        if self.records.is_empty() {
            log::warn!("search: index is empty");
            return Vec::new();
        }

        let candidates: Vec<&FileRecord> = if has_any_positive_term(&parsed) {
            let search_terms = build_search_string(&parsed);
            match search_type {
                SearchType::Fuzzy => {
                    let matcher = fuzzy_matcher::skim::SkimMatcherV2::default();
                    let mut scored: Vec<(i64, &FileRecord)> = self.records
                        .iter()
                        .filter_map(|r| {
                            let mut total: i64 = 0;
                            for term in &search_terms {
                                total += matcher.fuzzy_match(&r.name, term)?;
                            }
                            if total > 40 { Some((total, r)) } else { None }
                        })
                        .collect();
                    scored.sort_by(|a, b| b.0.cmp(&a.0));
                    scored.into_iter().map(|(_, r)| r).collect()
                }
                SearchType::Regex => {
                    let joined: String = search_terms.join(" ");
                    let re = match Regex::new(&joined) {
                        Ok(r) => r,
                        Err(_) => return Vec::new(),
                    };
                    self.records
                        .iter()
                        .filter(|r| re.is_match(&r.name))
                        .collect()
                }
                SearchType::Exact => {
                    self.records
                        .iter()
                        .filter(|r| {
                            let lower_name = r.name.to_lowercase();
                            search_terms.iter().all(|t| lower_name.contains(t))
                        })
                        .collect()
                }
            }
        } else {
            self.records.iter().collect()
        };

        candidates
            .into_iter()
            .filter(|r| passes_all_filters(r, &parsed))
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchType {
    Fuzzy,
    Regex,
    Exact,
}

fn tokenize(raw: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    for c in raw.chars() {
        match c {
            '"' => {
                current.push(c);
                in_quote = !in_quote;
            }
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
                q.ext_filters
                    .push((val.trim_start_matches('.').to_lowercase(), exclude));
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
            q.exact_phrases.push(token[1..token.len() - 1].to_lowercase());
        } else {
            q.include_terms.push(token.to_lowercase());
        }
    }
    q
}

fn build_search_string(q: &ParsedQuery) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    for t in &q.include_terms {
        if !t.contains('*') {
            parts.push(t.clone());
        }
    }
    for p in &q.exact_phrases {
        parts.push(p.clone());
    }
    parts
}

fn has_any_positive_term(q: &ParsedQuery) -> bool {
    !q.include_terms.is_empty() || !q.exact_phrases.is_empty()
}

fn contains_exclude_terms(lower_path: &str, q: &ParsedQuery) -> bool {
    q.exclude_terms.iter().any(|t| lower_path.contains(t))
}

fn matches_exact_phrases(lower_path: &str, q: &ParsedQuery) -> bool {
    q.exact_phrases.iter().all(|p| lower_path.contains(p))
}

fn matches_ext_filters(lower_path: &str, q: &ParsedQuery) -> bool {
    if q.ext_filters.is_empty() {
        return true;
    }
    let ext = Path::new(lower_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    for (filter_ext, exclude) in &q.ext_filters {
        let match_ext = ext == *filter_ext;
        if *exclude && match_ext {
            return false;
        }
        if !*exclude && !match_ext {
            return false;
        }
    }
    true
}

fn matches_path_filters(lower_path: &str, q: &ParsedQuery) -> bool {
    if q.path_filters.is_empty() {
        return true;
    }
    for (filter_path, exclude) in &q.path_filters {
        let match_path = lower_path.starts_with(filter_path);
        if *exclude && match_path {
            return false;
        }
        if !*exclude && !match_path {
            return false;
        }
    }
    true
}

fn matches_wildcard_terms(lower_name: &str, q: &ParsedQuery) -> bool {
    for term in &q.include_terms {
        if !term.contains('*') {
            continue;
        }
        let parts: Vec<&str> = term.split('*').filter(|p| !p.is_empty()).collect();
        if parts.is_empty() {
            continue;
        }

        let mut pos = 0;
        for (i, part) in parts.iter().enumerate() {
            match lower_name[pos..].find(part) {
                Some(idx) => {
                    if i == 0 && !term.starts_with('*') && idx != 0 {
                        return false;
                    }
                    pos += idx + part.len();
                }
                None => return false,
            }
        }
        if !term.ends_with('*') && pos < lower_name.len() {
            return false;
        }
    }
    true
}

fn passes_all_filters(record: &FileRecord, q: &ParsedQuery) -> bool {
    let lower_path = record.name.to_lowercase();
    let lower_name = Path::new(&record.name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();

    let ignore = SearchEngine::default_ignore_config();
    if ignore.is_noise(&record.name) {
        return false;
    }

    if contains_exclude_terms(&lower_path, q) {
        return false;
    }
    if !matches_exact_phrases(&lower_path, q) {
        return false;
    }
    if !matches_ext_filters(&lower_path, q) {
        return false;
    }
    if !matches_path_filters(&lower_path, q) {
        return false;
    }
    if !matches_wildcard_terms(&lower_name, q) {
        return false;
    }

    true
}

use std::sync::Mutex;

static ENGINE: once_cell::sync::Lazy<Mutex<Option<SearchEngine>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(None));

static LAST_RESULTS: once_cell::sync::Lazy<Mutex<Vec<String>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(Vec::new()));

unsafe fn cstr_to_str<'a>(ptr: *const c_char) -> &'a str {
    if ptr.is_null() {
        return "";
    }
    CStr::from_ptr(ptr).to_str().unwrap_or_default()
}

fn str_to_cstr_owned(s: &str) -> *mut c_char {
    CString::new(s).expect("CString::new failed").into_raw()
}

#[no_mangle]
pub extern "C" fn load_index_from_file(path: *const c_char) -> i32 {
    let file_path = unsafe { cstr_to_str(path) };
    match SearchEngine::load(file_path) {
        Ok(engine) => {
            if let Ok(mut guard) = ENGINE.lock() {
                *guard = Some(engine);
            }
            0
        }
        Err(e) => {
            log::error!("load_index_from_file: {}", e);
            -1
        }
    }
}

#[no_mangle]
pub extern "C" fn search_query(query: *const c_char, search_type: i32) -> u64 {
    let query_str = unsafe { cstr_to_str(query) };
    let st = match search_type {
        0 => SearchType::Fuzzy,
        1 => SearchType::Regex,
        2 => SearchType::Exact,
        _ => SearchType::Fuzzy,
    };

    let guard = match ENGINE.lock() {
        Ok(g) => g,
        Err(_) => return 0,
    };
    let engine = match guard.as_ref() {
        Some(e) => e,
        None => return 0,
    };

    let results = engine.search(query_str, st);
    let paths: Vec<String> = results.iter().map(|r| r.name.clone()).collect();
    let count = paths.len();
    drop(guard);

    if let Ok(mut last) = LAST_RESULTS.lock() {
        *last = paths;
    }
    count as u64
}

#[no_mangle]
pub extern "C" fn get_result_by_index(idx: u64) -> *mut c_char {
    let guard = match LAST_RESULTS.lock() {
        Ok(g) => g,
        Err(_) => return std::ptr::null_mut(),
    };
    guard
        .get(idx as usize)
        .map(|s| str_to_cstr_owned(s))
        .unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn index_size() -> u64 {
    let guard = match ENGINE.lock() {
        Ok(g) => g,
        Err(_) => return 0,
    };
    guard.as_ref().map(|e| e.index_size() as u64).unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn free_c_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        unsafe {
            let _ = CString::from_raw(ptr);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_records() -> Vec<FileRecord> {
        vec![
            FileRecord { id: 1, parent_id: 0, name: "/home/user/documents/report.pdf".into() },
            FileRecord { id: 2, parent_id: 0, name: "/home/user/documents/photo.jpg".into() },
            FileRecord { id: 3, parent_id: 0, name: "/home/user/music/song.mp3".into() },
            FileRecord { id: 4, parent_id: 0, name: "/home/user/videos/movie.mp4".into() },
            FileRecord { id: 5, parent_id: 0, name: "/home/user/documents/notes.txt".into() },
            FileRecord { id: 6, parent_id: 0, name: "/home/user/temp/12345.tmp".into() },
            FileRecord { id: 7, parent_id: 0, name: "/home/user/temp/notes.tmp".into() },
        ]
    }

    fn make_engine() -> SearchEngine {
        SearchEngine::from_records(test_records())
    }

    #[test]
    fn test_fuzzy_search() {
        let engine = make_engine();
        let results = engine.search("repo", SearchType::Fuzzy);
        assert!(!results.is_empty());
        assert!(results[0].name.contains("report"));
    }

    #[test]
    fn test_regex_search() {
        let engine = make_engine();
        let results = engine.search(r"\.(pdf|jpg)$", SearchType::Regex);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_exclude_filter() {
        let engine = make_engine();
        let results = engine.search("doc !photo", SearchType::Fuzzy);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_ext_filter() {
        let engine = make_engine();
        let results = engine.search("ext:pdf", SearchType::Fuzzy);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_ext_exclude_filter() {
        let engine = make_engine();
        let results = engine.search("ext:!tmp", SearchType::Fuzzy);
        assert_eq!(results.len(), 5);
    }

    #[test]
    fn test_noise_filter_skips_numeric_tmp() {
        let engine = make_engine();
        let results = engine.search("12345", SearchType::Fuzzy);
        assert!(results.is_empty());
    }

    #[test]
    fn test_exact_phrase() {
        let engine = make_engine();
        let results = engine.search("\"report.pdf\"", SearchType::Fuzzy);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_empty_query() {
        let engine = make_engine();
        let results = engine.search("", SearchType::Fuzzy);
        assert!(results.is_empty());
    }

    #[test]
    fn test_binary_load() {
        let tmp = std::env::temp_dir().join("searchengine-test.anythingindex");
        let records = vec![
            FileRecord { id: 1, parent_id: 0, name: "/home/test/file.txt".into() },
            FileRecord { id: 2, parent_id: 1, name: "/home/test/other.pdf".into() },
        ];
        let cancel = std::sync::atomic::AtomicBool::new(false);
        libanything::build_index_file(&records, &tmp, false, &cancel).unwrap();

        let engine = SearchEngine::load(&tmp).unwrap();
        assert_eq!(engine.index_size(), 2);
        let results = engine.search("file", SearchType::Fuzzy);
        assert_eq!(results.len(), 1);
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn test_ignore_config_default() {
        let ig = SearchEngine::default_ignore_config();

        #[cfg(unix)]
        {
            assert!(ig.is_skip_dir(std::path::Path::new("/proc")));
            assert!(ig.is_skip_dir(std::path::Path::new("/proc/self")));
            assert!(ig.is_skip_dir(std::path::Path::new("/tmp")));
            assert!(ig.is_skip_dir(std::path::Path::new("/boot")));
            assert!(ig.is_skip_dir(std::path::Path::new("/lib")));
            assert!(ig.is_skip_dir(std::path::Path::new("/lib64")));
            assert!(ig.is_skip_dir(std::path::Path::new("/usr/share/zoneinfo")));
            assert!(ig.is_skip_dir(std::path::Path::new("/usr/share/doc")));
            assert!(ig.is_skip_dir(std::path::Path::new("/usr/include")));
            assert!(ig.is_skip_dir(std::path::Path::new("/var/cache")));
            assert!(ig.is_skip_dir(std::path::Path::new("/var/log")));
            assert!(ig.is_skip_dir(std::path::Path::new("/opt")));
            assert!(ig.is_skip_dir(std::path::Path::new("/sysroot")));
            assert!(!ig.is_skip_dir(std::path::Path::new("/home/user/proc")));
        }

        #[cfg(windows)]
        {
            assert!(ig.is_skip_dir(std::path::Path::new("C:\\Windows")));
            assert!(ig.is_skip_dir(std::path::Path::new("C:\\Windows\\System32")));
            assert!(ig.is_skip_dir(std::path::Path::new("C:\\Program Files\\Google")));
            assert!(ig.is_skip_dir(std::path::Path::new("C:\\Program Files (x86)")));
            assert!(ig.is_skip_dir(std::path::Path::new("C:\\ProgramData")));
            assert!(!ig.is_skip_dir(std::path::Path::new("C:\\Users\\User\\Documents")));
        }

        assert!(ig.is_noise("/home/user/thumbs.db"));
        assert!(ig.is_noise("/tmp/foo.tmp"));
        assert!(!ig.is_noise("/home/user/report.pdf"));
    }
}
