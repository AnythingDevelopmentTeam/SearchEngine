use std::env;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use libanything::IndexerStatus;
use searchengine::{SearchEngine, SearchType};

fn home() -> PathBuf {
    env::var("HOME")
        .or_else(|_| env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn search(index: &PathBuf, query: &str, st: SearchType) {
    let engine = match SearchEngine::load(index) {
        Ok(e) => e,
        Err(e) => { eprintln!("search: {e}"); return; }
    };

    let results = engine.search(query, st);
    if results.is_empty() { return; }

    for (i, r) in results.iter().enumerate() {
        println!("{}\t{}", i + 1, r.name);
    }
}

fn build(index: &PathBuf) {
    let mut indexer = libanything::Indexer::new(index.clone());
    indexer.set_ignore_config(SearchEngine::default_ignore_config());
    indexer.start();

    loop {
        match indexer.status() {
            IndexerStatus::Running | IndexerStatus::Idle => {
                print!("\rIndexing... {} files", indexer.progress());
                use std::io::Write;
                std::io::stdout().flush().ok();
            }
            IndexerStatus::Completed => {
                println!("\rIndexed {} files. Done.", indexer.progress());
                return;
            }
            IndexerStatus::Failed => {
                eprintln!("\rIndexing failed");
                return;
            }
        }
        thread::sleep(Duration::from_millis(200));
    }
}

fn help(me: &str) {
    eprintln!("Usage: {me} [-t fuzzy|regex|exact] [-i <index>] <query>");
    eprintln!("       {me} --build [-i <index>]");
    eprintln!("       {me} --help");
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let me = &args[0];
    let mut index = home().join(".config/anything-index.anythingindex");
    let mut st = SearchType::Fuzzy;
    let mut do_build = false;
    let mut q: Vec<&str> = Vec::new();
    let mut i = 1;

    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => { help(me); return; }
            "--build" => { do_build = true; }
            "-t" | "--type" => {
                i += 1;
                st = match args.get(i).map(|s| s.as_str()) {
                    Some("f" | "fuzzy") => SearchType::Fuzzy,
                    Some("r" | "regex") => SearchType::Regex,
                    Some("e" | "exact") => SearchType::Exact,
                    _ => { help(me); return; }
                };
            }
            "-i" | "--index" => {
                i += 1;
                if let Some(p) = args.get(i) { index = PathBuf::from(p); }
            }
            s => q.push(s),
        }
        i += 1;
    }

    if do_build {
        build(&index);
        return;
    }

    let query = q.join(" ");
    if query.is_empty() { help(me); return; }

    search(&index, &query, st);
}
