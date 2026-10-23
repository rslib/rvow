//! The pinned grammar manifest and the loader that fills the grammar cache.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rvow_base::error::{Error, Result};
use rvow_base::hash::sha256_hex;
use tree_sitter::{Language, Parser, Tree, WasmStore, wasmtime};

/// Whole-request timeout for a grammar download. A 18 MB grammar (verilog) on
/// a slow connection still finishes; a hung host fails the check instead of
/// blocking the gate.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// How long prove and gate wait after a failed fetch before they try the same
/// language again. `rvow grammar fetch` ignores it.
pub const FETCH_RETRY_WINDOW: Duration = Duration::from_secs(3600);

/// The per-home record of languages whose last fetch failed.
const FAILURES_FILE: &str = "fetch-failures.json";

/// Environment variable that replaces the download base of pinned URLs.
/// `file://`, an HTTP(S) URL or a filesystem path; the file name stays the
/// pinned one. Lets tests and air-gapped machines supply grammars offline.
pub const URL_VAR: &str = "RVOW_GRAMMAR_URL";

/// When set to a non-empty value and no download base is set, a grammar is
/// never downloaded from the pinned URLs: the fetch fails at once. Tests set
/// it so none of them waits on the network. Users leave it unset.
pub const OFFLINE_VAR: &str = "RVOW_GRAMMAR_OFFLINE";

/// Whether the no-base refuse rule is on.
pub fn offline() -> bool {
    std::env::var(OFFLINE_VAR).is_ok_and(|v| !v.is_empty())
}

/// One pinned language: a prebuilt `tree-sitter-<name>.wasm` uploaded to a
/// GitHub release and the sha256 of its exact bytes.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub name: String,
    pub ext: Vec<String>,
    pub url: String,
    pub sha256: String,
}

#[derive(serde::Deserialize)]
struct Manifest {
    #[serde(rename = "grammar")]
    entries: Vec<Entry>,
}

/// The manifest embedded in the binary; every load treats it as read-only.
static MANIFEST: &str = include_str!("grammars.toml");

/// A loaded tree-sitter parser for `text` of one pinned language.
pub struct Session {
    engine: wasmtime::Engine,
    store: Option<WasmStore>,
    langs: BTreeMap<String, Language>,
    parses: u64,
}

impl Session {
    pub fn new() -> Result<Session> {
        let mut config = wasmtime::Config::new();
        config.cranelift_opt_level(wasmtime::OptLevel::None);
        let engine = wasmtime::Engine::new(&config).map_err(|e| Error::internal(format!("the grammar engine did not start: {e}")))?;
        Ok(Session { engine, store: None, langs: BTreeMap::new(), parses: 0 })
    }

    /// How many texts this session parsed. Tests use it to prove a saved
    /// compare result answered without parsing.
    pub fn parses(&self) -> u64 {
        self.parses
    }

    /// The loaded language of `entry`, for callers that run queries over a
    /// tree this session parsed. `None` before a parse of `entry` loaded it.
    pub fn language(&self, entry: &Entry) -> Option<Language> {
        self.langs.get(&entry.name).cloned()
    }

    /// Parses `text` with the grammar of `entry`, downloading it into the
    /// cache when it is not there yet. Returns the syntax tree; a parse with
    /// error nodes still returns a tree and the caller reads `has_error`.
    pub fn parse(&mut self, home: &Path, entry: &Entry, base: Option<&str>, text: &str) -> Result<Tree> {
        self.parses += 1;
        let bytes = acquire(home, entry, base)?;
        if !self.langs.contains_key(&entry.name) {
            self.load(entry, &bytes)?;
        }
        let lang = self.langs.get(&entry.name).ok_or_else(|| Error::internal("the grammar was not kept"))?;
        let mut parser = Parser::new();
        // A wasm language lives inside the store. set_wasm_store keeps the
        // store pointer in the FFI parser, so the Language handle must be set
        // after the store is on the parser, and the store returns here.
        let store = self.store.take().ok_or_else(|| Error::internal("the grammar wasm store is gone"))?;
        if let Err(e) = parser.set_wasm_store(store) {
            return Err(Error::internal(format!("the parser refused the grammar wasm store: {e}")));
        }
        parser.set_language(lang).map_err(|e| grammar_error(entry, format!("the grammar did not set up: {e}")))?;
        let Some(tree) = parser.parse(text, None) else {
            return Err(Error::internal("the tree-sitter parse was cancelled"));
        };
        let store = parser.take_wasm_store().ok_or_else(|| Error::internal("the grammar wasm store is gone"))?;
        self.store = Some(store);
        Ok(tree)
    }

    fn load(&mut self, entry: &Entry, bytes: &[u8]) -> Result<()> {
        if self.store.is_none() {
            self.store = Some(WasmStore::new(&self.engine).map_err(|e| grammar_error(entry, e.to_string()))?);
        }
        let store = self.store.as_mut().ok_or_else(|| Error::internal("the grammar wasm store is gone"))?;
        let lang = store.load_language(entry.name.as_str(), bytes).map_err(|e| grammar_error(entry, e.to_string()))?;
        self.langs.insert(entry.name.clone(), lang);
        Ok(())
    }
}

/// Download base override: `path/filename` from an existing `base` URL.
fn with_base(pinned: &str, base: &str) -> String {
    let file = pinned.rsplit('/').next().unwrap_or(pinned);
    let base = base.trim_end_matches('/');
    if let Some(rep) = base.strip_prefix("file://") {
        return Path::new(rep).join(file).display().to_string();
    }
    if base.contains("://") {
        return format!("{base}/{file}");
    }
    Path::new(base).join(file).display().to_string()
}

fn download(url: &str, timeout: Duration) -> Result<Vec<u8>> {
    let response = ureq::get(url)
        .config()
        .timeout_global(Some(timeout))
        .build()
        .call()
        .map_err(|e| Error::new("download", format!("{url}: {e}"), "check the network, then run the command again"))?;
    let mut bytes = Vec::new();
    response
        .into_body()
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| Error::new("download", format!("{url}: {e}"), "check the network, then run the command again"))?;
    Ok(bytes)
}

fn fetch(entry: &Entry, base: Option<&str>, timeout: Duration) -> Result<Vec<u8>> {
    if base.is_none() && offline() {
        return Err(grammar_error(entry, format!("the pinned download is refused while {OFFLINE_VAR} is set")));
    }
    let url = match base {
        Some(b) => with_base(&entry.url, b),
        None => entry.url.clone(),
    };
    if url.starts_with("http://") || url.starts_with("https://") {
        return download(&url, timeout);
    }
    if url.contains("://") {
        return Err(Error::new(
            "grammar",
            format!("{url}: unsupported scheme"),
            format!("use {URL_VAR} with a file:// URL, a path or an HTTP(S) URL"),
        ));
    }
    if !Path::new(&url).is_file() {
        return Err(grammar_error(entry, format!("{url} does not exist")));
    }
    fs::read(&url).map_err(|e| Error::io(Path::new(&url), e))
}

/// The grammar cache file for one pinned language.
pub fn cache_file(home: &Path, entry: &Entry) -> PathBuf {
    home.join("grammars").join(format!("tree-sitter-{}.wasm", entry.name))
}

fn failures_path(home: &Path) -> PathBuf {
    home.join("grammars").join(FAILURES_FILE)
}

fn read_failures(home: &Path) -> BTreeMap<String, u64> {
    std::fs::read_to_string(failures_path(home)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn write_failures(home: &Path, failures: &BTreeMap<String, u64>) {
    let path = failures_path(home);
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    if let Ok(text) = serde_json::to_string(failures) {
        let _ = std::fs::write(&path, text);
    }
}

/// Whether a failed fetch of `entry` is recorded less than `window` ago.
/// Prove and gate treat such a language like one with no grammar at all.
pub fn fetch_recently_failed(home: &Path, entry: &Entry, now_ms: u64, window: Duration) -> bool {
    read_failures(home).get(&entry.name).is_some_and(|t| now_ms.saturating_sub(*t) < window.as_millis() as u64)
}

/// Records a failed fetch of `entry` at `now_ms`.
pub fn record_fetch_failure(home: &Path, entry: &Entry, now_ms: u64) {
    let mut failures = read_failures(home);
    failures.insert(entry.name.clone(), now_ms);
    write_failures(home, &failures);
}

/// Clears the failure record of `entry` after a fetch succeeds.
pub fn clear_fetch_failure(home: &Path, entry: &Entry) {
    let mut failures = read_failures(home);
    if failures.remove(&entry.name).is_none() {
        return;
    }
    write_failures(home, &failures);
}

/// The error callers show: the grammar is missing, never a panic.
fn grammar_error(entry: &Entry, what: String) -> Error {
    Error::new(
        "grammar",
        format!("the {} grammar is missing: {what}", entry.name),
        format!("run `rvow grammar fetch {}`, or check {} when the download base is overridden", entry.name, URL_VAR),
    )
}

/// Pinned manifest entries; list is the order of grammars.toml.
pub fn pinned() -> &'static [Entry] {
    static ENTRIES: std::sync::OnceLock<Vec<Entry>> = std::sync::OnceLock::new();
    ENTRIES.get_or_init(|| match toml::from_str::<Manifest>(MANIFEST) {
        Ok(manifest) => manifest.entries,
        Err(e) => {
            eprintln!("broken pinned grammar manifest: {e}");
            Vec::new()
        }
    })
}

/// One pinned entry by language name, case exact (`c_sharp`, `cpp`).
pub fn by_name(name: &str) -> Option<&'static Entry> {
    pinned().iter().find(|e| e.name == name)
}

/// The pinned entry whose extension list holds `ext`, ignoring case.
/// A file with no extension or an unpinned one has no grammar.
pub fn by_ext(ext: &str) -> Option<&'static Entry> {
    let ext = ext.to_ascii_lowercase();
    pinned().iter().find(|e| e.ext.iter().any(|c| c.eq_ignore_ascii_case(&ext)))
}

/// Extensionless file names whose grammar is pinned, by lower-case name.
const PINNED_FILES: &[(&str, &str)] = &[("makefile", "make")];

/// The pinned entry for a file path: by final extension, else for an
/// extensionless file whose stripped file name is pinned.
pub fn for_file(file: &str) -> Option<&'static Entry> {
    let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => by_ext(ext),
        _ => {
            let lower = name.to_ascii_lowercase();
            PINNED_FILES.iter().find(|(f, _)| *f == lower).and_then(|(_, lang)| by_name(lang))
        }
    }
}

/// Whether the grammar was already in the cache or had to download.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Source {
    Cache,
    Download,
}

/// The grammar bytes from the cache alone, or `None` when the cache holds
/// no file whose hash matches the pinned one. Checks that need to stay
/// offline and instant use this and keep their own rule for `None`.
pub fn cached(home: &Path, entry: &Entry) -> Option<Vec<u8>> {
    let file = cache_file(home, entry);
    let bytes = fs::read(&file).ok()?;
    (sha256_hex(&bytes) == entry.sha256).then_some(bytes)
}

/// The grammar bytes for `entry`, from the cache or a checked download:
/// a cache or download hash that differs from the pinned one is never
/// returned, cached or written.
pub fn acquire(home: &Path, entry: &Entry, base: Option<&str>) -> Result<Vec<u8>> {
    acquire_with_timeout(home, entry, base, DOWNLOAD_TIMEOUT)
}

/// Like `acquire`, with the caller's whole-request timeout for one grammar.
/// Prove and gate pass a short one so a hung host fails fast.
pub fn acquire_with_timeout(home: &Path, entry: &Entry, base: Option<&str>, timeout: Duration) -> Result<Vec<u8>> {
    let file = cache_file(home, entry);
    if let Some(bytes) = cached(home, entry) {
        return Ok(bytes);
    }
    let bytes = fetch(entry, base, timeout)?;
    if sha256_hex(&bytes) != entry.sha256 {
        let message = format!("the download from {} hashes {} in place of the pinned {}", entry.url, sha256_hex(&bytes), entry.sha256);
        return Err(grammar_error(entry, message));
    }
    let dir = file.parent().ok_or_else(|| Error::internal("the grammar cache file has no folder"))?;
    fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    let tmp = dir.join(format!(".rvow-grammar-{}.tmp", std::process::id()));
    fs::write(&tmp, &bytes).map_err(|e| Error::io(&tmp, e))?;
    // Replace then rename: a rename over an existing file is atomic in
    // place, so a reader never sees a partial grammar file.
    if file.is_file() {
        fs::remove_file(&file).map_err(|e| Error::io(&file, e))?;
    }
    fs::rename(&tmp, &file).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        Error::io(&file, e)
    })?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_manifest_with_a_name_and_hash_per_entry() {
        let entries = pinned();
        assert!(!entries.is_empty());
        for entry in entries {
            assert_eq!(entry.sha256.len(), 64, "{}", entry.name);
            assert!(entry.url.ends_with(".wasm"), "{}", entry.url);
        }
    }

    #[test]
    fn json_is_pinned_for_the_json_extension() {
        let entry = by_ext("json").unwrap();
        assert_eq!(entry.name, "json");
    }

    #[test]
    fn unknown_files_have_no_pinned_entry() {
        assert!(by_ext("xorx").is_none());
    }
}
