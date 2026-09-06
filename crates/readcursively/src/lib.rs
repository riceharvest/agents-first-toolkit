//! readcursively - read->read 49.8%, search->search 37.2%, search->read 32% (4201 bigrams). One call returns matches + file contents.

pub mod update;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Result, anyhow, bail};
use globset::GlobBuilder;
use ignore::{WalkBuilder, WalkState};
use rayon::prelude::*;
use regex::Regex;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Limits (adversarial caps). Library constants; CLI exposes overrides.
// ---------------------------------------------------------------------------

/// Maximum matches returned per search query.
pub const DEFAULT_MAX_RESULTS: usize = 200;
/// Maximum regex pattern length in chars; longer patterns are rejected at
/// compile time. Measured worst case at 1024: ~5ms per adversarial match
/// confirmation, so a fully adversarial file with the 200-hit cap stays ~1s.
/// Catastrophic backtracking is impossible (RE2-style linear engine), but
/// huge literals make the engine's lazy-DFA/prefix scan pathological.
pub const MAX_PATTERN_CHARS: usize = 1024;
/// Maximum preview length per hit, in chars.
pub const PREVIEW_LEN: usize = 240;
/// Skip files larger than this many bytes.
pub const DEFAULT_MAX_FILE_SIZE: u64 = 5 * 1024 * 1024;
/// Truncate total output after this many characters.
pub const DEFAULT_MAX_TOTAL_CHARS: usize = 500_000;
/// Maximum directory depth relative to the root.
pub const DEFAULT_MAX_DEPTH: usize = 64;

// ---------------------------------------------------------------------------
// Request schema
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SearchQuery {
    /// Ripgrep-style regex pattern (matched against file contents).
    pub pattern: String,
    /// Optional glob filter, e.g. "*.rs" or "src/**/*.ts".
    #[serde(default)]
    pub glob: Option<String>,
    /// Optional file-type filter (rust, python, js, ts, go, md, json, toml, yaml, txt).
    #[serde(default)]
    pub file_type: Option<String>,
    /// Per-query match cap (default 200).
    #[serde(default)]
    pub max_results: Option<usize>,
    /// Lines of context before/after each hit (0 = off, max 5).
    #[serde(default)]
    pub context: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReadRequest {
    /// File path relative to the root (or absolute when root is empty).
    pub path: String,
    /// 1-indexed first line to return.
    #[serde(default)]
    pub offset: Option<usize>,
    /// Maximum number of lines to return.
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BatchRequest {
    /// Searches run in parallel; each returns its own hit list.
    #[serde(default)]
    pub searches: Vec<SearchQuery>,
    /// Reads run in parallel; deduplicated by path.
    #[serde(default)]
    pub reads: Vec<ReadRequest>,
    /// Root directory for relative paths and searches (default: current dir).
    #[serde(default)]
    pub root: Option<String>,
    /// If true, files hit by searches are automatically read and returned too.
    #[serde(default)]
    pub auto_read: bool,
    /// Default lines of context around each hit for all searches (0 = off).
    #[serde(default)]
    pub context: Option<usize>,
    /// Cap on lines returned per auto-read file (default 2000).
    #[serde(default)]
    pub auto_read_limit: Option<usize>,
    #[serde(default)]
    pub max_results: Option<usize>,
    #[serde(default)]
    pub max_file_size: Option<u64>,
    #[serde(default)]
    pub max_total_chars: Option<usize>,
    #[serde(default)]
    pub max_depth: Option<usize>,
    /// Respect .gitignore / .ignore files (default true). Disable with false.
    #[serde(default = "default_true")]
    pub respect_ignore: bool,
    /// Follow symlinked directories during the walk (default false).
    #[serde(default)]
    pub follow_symlinks: bool,
    /// Include hidden files (default false).
    #[serde(default)]
    pub hidden: bool,
}

fn default_true() -> bool {
    true
}

impl Default for BatchRequest {
    fn default() -> Self {
        Self {
            searches: vec![],
            reads: vec![],
            root: None,
            auto_read: false,
            context: None,
            auto_read_limit: None,
            max_results: None,
            max_file_size: None,
            max_total_chars: None,
            max_depth: None,
            respect_ignore: true,
            follow_symlinks: false,
            hidden: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Response schema
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    /// Path as given (relative to root when root is set).
    pub path: String,
    /// 1-indexed line number.
    pub line: usize,
    /// 1-indexed column of the first match.
    pub col: usize,
    /// The matched line, truncated to PREVIEW_LEN chars.
    pub preview: String,
    /// Context lines before the hit: "LNUM|content", oldest first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<Vec<String>>,
    /// Context lines after the hit: "LNUM|content".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchOutcome {
    pub pattern: String,
    /// Ok, or a human-readable reason the pattern produced nothing (bad regex, bad glob).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Hit count before truncation, if the cap bit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<usize>,
    pub hits: Vec<Hit>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReadOutcome {
    pub path: String,
    /// Ok, or a human-readable reason the file was skipped (missing, binary, too large).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_count: Option<usize>,
    /// "LNUM|content" lines, 1-indexed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatchResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub searches: Option<Vec<SearchOutcome>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reads: Option<Vec<ReadOutcome>>,
    /// Total output characters; useful when the global cap truncated.
    pub total_chars: usize,
}

/// Back-compat shim used by older docs/tests: entries of {type: search|read, ...}.
#[derive(Debug, Clone, Deserialize)]
pub struct LegacyItem {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(flatten)]
    pub rest: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// Effective limits for a batch run.
#[derive(Debug, Clone)]
pub struct Limits {
    pub max_results: usize,
    pub max_file_size: u64,
    pub max_total_chars: usize,
    pub max_depth: usize,
    pub auto_read_limit: usize,
    pub context: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_results: DEFAULT_MAX_RESULTS,
            max_file_size: DEFAULT_MAX_FILE_SIZE,
            max_total_chars: DEFAULT_MAX_TOTAL_CHARS,
            max_depth: DEFAULT_MAX_DEPTH,
            auto_read_limit: 2000,
            context: 0,
        }
    }
}

impl Limits {
    fn from_request(req: &BatchRequest) -> Self {
        let d = Limits::default();
        Limits {
            max_results: req.max_results.unwrap_or(d.max_results).max(1),
            max_file_size: req.max_file_size.unwrap_or(d.max_file_size).max(1),
            max_total_chars: req.max_total_chars.unwrap_or(d.max_total_chars).max(1),
            max_depth: req.max_depth.unwrap_or(d.max_depth).max(1),
            auto_read_limit: req.auto_read_limit.unwrap_or(d.auto_read_limit).max(1),
            context: req.context.unwrap_or(d.context).min(5),
        }
    }
}

/// Detect text vs binary: NUL byte in the first 8KiB, or invalid UTF-8 anywhere
/// in that window. Mirrors the hermes read_file behavior loosely enough to be
/// useful and cheap.
pub fn is_binary(buf: &[u8]) -> bool {
    let window = &buf[..buf.len().min(8192)];
    if window.contains(&0) {
        return true;
    }
    match std::str::from_utf8(window) {
        Ok(_) => false,
        Err(e) => {
            // Truncated multibyte at the window edge is fine; real garbage is not.
            e.error_len().is_some()
                || std::str::from_utf8(&window[..e.valid_up_to()])
                    .is_err_and(|_| e.valid_up_to() < window.len() && window.len() == 8192 && false)
                || {
                    // If the error is only "incomplete sequence at end of window",
                    // treat as text. Otherwise binary.
                    !(e.error_len().is_none() && e.valid_up_to() + 4 > window.len())
                }
        }
    }
}

fn build_matcher(
    pattern: &str,
    glob: Option<&str>,
    file_type: Option<&str>,
) -> Result<(Regex, Option<globset::GlobSet>)> {
    if pattern.chars().count() > MAX_PATTERN_CHARS {
        bail!(
            "pattern too long ({} chars > {} cap); long patterns make matching pathological - use a shorter regex or a glob + post-filter",
            pattern.chars().count(),
            MAX_PATTERN_CHARS
        );
    }
    let re = Regex::new(pattern).map_err(|e| anyhow!("bad regex {pattern:?}: {e}"))?;
    let gs = match (glob, file_type) {
        (Some(g), None) => Some(glob_from_str(g)?),
        (None, Some(ft)) => Some(glob_from_file_type(ft)?),
        (Some(g), Some(ft)) => Some(glob_union(&[g, ft])?),
        (None, None) => None,
    };
    Ok((re, gs))
}

/// Compile one glob pattern (any-depth normalization for bare names/extensions).
fn compile_glob(pattern: &str) -> Result<globset::Glob> {
    let normalized = if pattern.contains('/') {
        pattern.to_owned()
    } else {
        format!("**/{pattern}")
    };
    GlobBuilder::new(&normalized)
        .literal_separator(true)
        .build()
        .map_err(|e| anyhow!("bad glob {pattern:?}: {e}"))
}

/// Combine glob strings and file-type names into one GlobSet (match either).
fn glob_union(parts: &[&str]) -> Result<globset::GlobSet> {
    let mut builder = globset::GlobSetBuilder::new();
    let mut added = false;
    for p in parts {
        let looks_like_glob = p.contains('*')
            || p.contains('?')
            || p.contains('[')
            || p.contains('/')
            || p.contains('.');
        let exts: &[&str] = if looks_like_glob {
            &[]
        } else {
            match p.to_ascii_lowercase().as_str() {
                "rust" | "rs" => &["rs"],
                "python" | "py" => &["py"],
                "javascript" | "js" | "jsx" => &["js", "jsx", "mjs", "cjs"],
                "typescript" | "ts" | "tsx" => &["ts", "tsx"],
                "go" => &["go"],
                "markdown" | "md" => &["md", "markdown"],
                "json" => &["json", "jsonc", "jsonl"],
                "toml" => &["toml"],
                "yaml" | "yml" => &["yaml", "yml"],
                "shell" | "sh" => &["sh", "bash", "zsh"],
                "c" => &["c", "h"],
                "cpp" | "c++" => &["cpp", "cc", "cxx", "hpp", "hh"],
                "text" | "txt" => &["txt"],
                "html" => &["html", "htm"],
                "css" => &["css", "scss", "less"],
                other => bail!(
                    "unknown file type {other:?}; try rust/python/js/ts/go/md/json/toml/yaml/sh/c/cpp/text/html/css or a --glob instead"
                ),
            }
        };
        if looks_like_glob {
            builder.add(compile_glob(p)?);
            added = true;
        } else {
            for e in exts {
                builder.add(compile_glob(&format!("*.{e}"))?);
                added = true;
            }
        }
    }
    if added {
        Ok(builder.build()?)
    } else {
        bail!("empty glob union")
    }
}

fn glob_from_str(g: &str) -> Result<globset::GlobSet> {
    let mut builder = globset::GlobSetBuilder::new();
    builder.add(compile_glob(g)?);
    Ok(builder.build()?)
}

fn glob_from_file_type(ft: &str) -> Result<globset::GlobSet> {
    glob_union(&[ft])
}

fn rel_path(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .map(|r| r.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| p.to_string_lossy().replace('\\', "/"))
}

/// Walk the root once, returning candidate text files that pass ignore rules,
/// hidden/size filters, and (optionally) a glob.
fn collect_files(
    req: &BatchRequest,
    limits: &Limits,
    matcher: Option<&globset::GlobSet>,
) -> Result<Vec<PathBuf>> {
    let root = PathBuf::from(req.root.as_deref().unwrap_or("."));
    if !root.is_dir() {
        bail!("root {:?} is not a directory", root);
    }
    let found = AtomicUsize::new(0);
    let hard_cap = 1_000_000usize; // absolute file ceiling; keep memory sane even on huge repos
    let mut builder = WalkBuilder::new(&root);
    builder.hidden(!req.hidden);
    builder.require_git(false); // honor .gitignore even outside a git repo
    builder.git_ignore(req.respect_ignore);
    builder.git_global(req.respect_ignore);
    builder.git_exclude(req.respect_ignore);
    builder.ignore(req.respect_ignore);
    builder.parents(req.respect_ignore);
    builder.follow_links(req.follow_symlinks);
    builder.max_depth(Some(limits.max_depth.min(128)));
    if req.respect_ignore {
        builder.filter_entry(|e| {
            // Never descend into .git even with hidden=true; nothing good is in there.
            e.file_name() != ".git"
        });
    }

    let walker = builder.build_parallel();
    let files = std::sync::Mutex::new(Vec::<PathBuf>::new());
    walker.run(|| {
        let files = &files;
        let found = &found;
        Box::new(move |entry: Result<ignore::DirEntry, ignore::Error>| {
            if found.load(Ordering::Relaxed) >= hard_cap {
                return WalkState::Quit;
            }
            let Ok(entry) = entry else {
                return WalkState::Continue;
            };
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                return WalkState::Continue;
            }
            let path = entry.path();
            // Size pre-filter: stat is cheap relative to reading a huge file.
            let skip = match entry.metadata() {
                Ok(md) => md.len() > limits.max_file_size,
                Err(_) => false,
            };
            if skip {
                return WalkState::Continue;
            }
            if let Some(gs) = matcher {
                let rel = path.to_string_lossy();
                let rel_norm = rel.replace('\\', "/");
                let ok = gs.is_match(&rel_norm)
                    || gs.is_match(
                        Path::new(&rel_norm)
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .as_ref(),
                    );
                if !ok {
                    return WalkState::Continue;
                }
            }
            found.fetch_add(1, Ordering::Relaxed);
            files.lock().expect("files lock").push(path.to_path_buf());
            WalkState::Continue
        })
    });
    let mut files = files.into_inner().expect("files lock");
    files.sort();
    Ok(files)
}

/// Search a single file with a compiled pattern. Returns (had_hits, more_lines_unscanned).
fn search_file(
    path: &Path,
    root: &Path,
    re: &Regex,
    limits: &Limits,
    out: &mut Vec<Hit>,
) -> Result<(bool, bool)> {
    // Refuse unreadable/too-large files without failing the whole batch.
    let md = match std::fs::metadata(path) {
        Ok(md) => md,
        Err(_) => return Ok((false, false)),
    };
    if !md.is_file() || md.len() > limits.max_file_size {
        return Ok((false, false));
    }
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return Ok((false, false)),
    };
    if is_binary(&bytes) {
        return Ok((false, false));
    }
    let text = String::from_utf8_lossy(&bytes);
    let all_lines: Vec<&str> = text.lines().collect();
    let ctx = limits.context;
    let mut count = 0usize;
    for (i, line) in all_lines.iter().enumerate() {
        if count >= limits.max_results {
            // Cap reached with at least one more line unexamined.
            return Ok((true, true));
        }
        if let Some(m) = re.find(line) {
            let preview: String = truncate_chars(line, PREVIEW_LEN);
            let (before, after) = if ctx > 0 {
                (
                    Some(
                        all_lines[i.saturating_sub(ctx)..i]
                            .iter()
                            .enumerate()
                            .map(|(j, l)| {
                                format!("{}|{}", i - ctx + j + 1, truncate_chars(l, 2000))
                            })
                            .collect(),
                    ),
                    Some(
                        all_lines[i + 1..(i + 1 + ctx).min(all_lines.len())]
                            .iter()
                            .enumerate()
                            .map(|(j, l)| format!("{}|{}", i + 2 + j, truncate_chars(l, 2000)))
                            .collect(),
                    ),
                )
            } else {
                (None, None)
            };
            out.push(Hit {
                path: rel_path(root, path),
                line: i + 1,
                col: m.start() + 1,
                preview,
                before,
                after,
            });
            count += 1;
        }
    }
    Ok((count > 0, false))
}

fn truncate_chars(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_owned();
    }
    s.chars().take(n).collect()
}

/// Search every query over a shared file list. Queries run in parallel; files
/// within a query run in parallel via rayon.
fn run_searches(
    req: &BatchRequest,
    limits: &Limits,
    queries: &[SearchQuery],
) -> Vec<SearchOutcome> {
    // Group queries by identical glob+file_type so files are walked once per distinct filter.
    use std::collections::HashMap;
    type QueryGroup = (Option<String>, Option<String>);
    let mut groups: HashMap<QueryGroup, Vec<(usize, &SearchQuery)>> = HashMap::new();
    for (i, q) in queries.iter().enumerate() {
        groups
            .entry((q.glob.clone(), q.file_type.clone()))
            .or_default()
            .push((i, q));
    }

    let mut outcomes: Vec<Option<SearchOutcome>> = (0..queries.len()).map(|_| None).collect();
    rayon::scope(|_| {
        type CompiledQuery<'a> = (
            usize,
            Result<(Regex, Option<globset::GlobSet>)>,
            &'a SearchQuery,
        );
        // Compile each query's regex up front so bad patterns fail fast, not per-file.
        let compiled: Vec<CompiledQuery> = queries
            .iter()
            .enumerate()
            .map(|(i, q)| {
                (
                    i,
                    build_matcher(&q.pattern, q.glob.as_deref(), q.file_type.as_deref()),
                    q,
                )
            })
            .collect();

        // Build one file list per distinct (glob, file_type) group, in parallel.
        let group_files: Vec<(QueryGroup, Vec<PathBuf>)> = {
            let mut v: Vec<(QueryGroup, Result<Vec<PathBuf>>)> = groups
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .into_par_iter()
                .map(|key| {
                    let (glob, ft) = &key;
                    let gs = match (glob.as_deref(), ft.as_deref()) {
                        (Some(g), None) => glob_from_str(g).ok(),
                        (None, Some(f)) => glob_from_file_type(f).ok(),
                        (Some(g), Some(f)) => glob_union(&[g, f]).ok(),
                        (None, None) => None,
                    };
                    let m = gs.as_ref();
                    let res = collect_files(req, limits, m);
                    (key, res)
                })
                .collect();
            v.sort_by(|a, b| a.0.cmp(&b.0));
            v.into_iter()
                .map(|(k, r)| (k, r.unwrap_or_default()))
                .collect()
        };

        let file_map: HashMap<QueryGroup, &Vec<PathBuf>> =
            group_files.iter().map(|(k, v)| (k.clone(), v)).collect();

        // Run all queries in parallel over their group's file list.
        let results: Vec<(usize, SearchOutcome)> = compiled
            .into_par_iter()
            .map(|(i, compiled, q)| {
                let outcome = match compiled {
                    Err(e) => SearchOutcome {
                        pattern: q.pattern.clone(),
                        error: Some(e.to_string()),
                        truncated: None,
                        hits: vec![],
                    },
                    Ok((re, _)) => {
                        let key = (q.glob.clone(), q.file_type.clone());
                        let empty: Vec<PathBuf> = Vec::new();
                        let files: &Vec<PathBuf> = file_map.get(&key).copied().unwrap_or(&empty);
                        let root = PathBuf::from(req.root.as_deref().unwrap_or("."));
                        let per_query_cap = q.max_results.unwrap_or(limits.max_results).max(1);
                        // Track truncation properly: search_file returns whether it hit the
                        // per-file cap with more matching lines left unscanned.
                        let any_more = AtomicUsize::new(0);
                        let mut hits: Vec<Hit> = files
                            .par_iter()
                            .filter_map(|f| {
                                let mut local = Vec::new();
                                match search_file(
                                    f,
                                    &root,
                                    &re,
                                    &Limits {
                                        max_results: per_query_cap,
                                        ..limits.clone()
                                    },
                                    &mut local,
                                ) {
                                    Ok((true, more)) => {
                                        if more {
                                            any_more.fetch_add(1, Ordering::Relaxed);
                                        }
                                        Some(local)
                                    }
                                    _ => None,
                                }
                            })
                            .flatten()
                            .collect();
                        hits.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
                        let truncated =
                            any_more.load(Ordering::Relaxed) > 0 || hits.len() > per_query_cap;
                        let hits = if truncated {
                            hits.into_iter().take(per_query_cap).collect::<Vec<_>>()
                        } else {
                            hits
                        };
                        SearchOutcome {
                            pattern: q.pattern.clone(),
                            error: None,
                            truncated: truncated.then_some(per_query_cap),
                            hits,
                        }
                    }
                };
                (i, outcome)
            })
            .collect();

        for (i, o) in results {
            outcomes[i] = Some(o);
        }
    });

    outcomes
        .into_iter()
        .map(|o| {
            o.unwrap_or(SearchOutcome {
                pattern: String::new(),
                error: Some("internal: query missing".into()),
                truncated: None,
                hits: vec![],
            })
        })
        .collect()
}

/// Read one file honoring offset/limit, binary skip, size cap.
pub fn read_one(
    path: &Path,
    limits: &Limits,
    offset: Option<usize>,
    limit: Option<usize>,
) -> ReadOutcome {
    let display_path = path.to_string_lossy().replace('\\', "/");
    let bad = |e: String| ReadOutcome {
        path: display_path.clone(),
        error: Some(e),
        truncated: None,
        line_count: None,
        display: None,
    };
    let md = match std::fs::metadata(path) {
        Ok(md) => md,
        Err(e) => return bad(format!("stat failed: {e}")),
    };
    if md.is_dir() {
        return bad("is a directory".into());
    }
    if md.len() > limits.max_file_size {
        return bad(format!(
            "file too large ({} > {} bytes)",
            md.len(),
            limits.max_file_size
        ));
    }
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return bad(format!("read failed: {e}")),
    };
    if is_binary(&bytes) {
        return bad("binary file skipped".into());
    }
    let text = String::from_utf8_lossy(&bytes);
    let all_lines: Vec<&str> = text.lines().collect();
    let line_count = all_lines.len();
    let start = offset.unwrap_or(1).max(1); // 1-indexed
    let end = match limit {
        Some(l) => start.saturating_add(l.saturating_sub(1)).min(line_count),
        None => line_count,
    };
    let mut truncated = false;
    let start_i = (start - 1).min(line_count);
    let mut display: Vec<String> = Vec::new();
    let mut chars = 0usize;
    for (i, line) in all_lines
        .iter()
        .enumerate()
        .skip(start_i)
        .take(end.saturating_sub(start_i))
    {
        let t = truncate_chars(line, 2000);
        let entry = format!("{}|{}", i + 1, t);
        chars += entry.len() + 1;
        if chars > limits.max_total_chars && limits.max_total_chars > 0 && display.len() > 100 {
            // Only truncate very long single reads; searches stay intact.
            truncated = true;
            break;
        }
        display.push(entry);
    }
    if start_i < line_count && start + display.len() - 1 < line_count && !display.is_empty() {
        truncated = true;
    }
    ReadOutcome {
        path: display_path,
        error: None,
        truncated: Some(
            truncated
                || end < line_count
                || (limit.is_some() && start_i + (end - start_i) < line_count),
        ),
        line_count: Some(line_count),
        display: Some(display),
    }
}

/// Cold-start probe: list one directory's entries as JSON so an agent can
/// orient in an unfamiliar cwd without a separate listing tool. Respects
/// hidden=false by default; caps entries and name length. Deterministic order.
pub fn probe(dir: &str) -> Result<String> {
    use serde_json::json;

    const MAX_ENTRIES: usize = 500;
    const MAX_NAME_CHARS: usize = 200;

    let path = Path::new(dir);
    if !path.is_dir() {
        bail!("probe: {dir} is not a directory");
    }
    let mut entries = std::fs::read_dir(path)
        .map_err(|e| anyhow!("probe: cannot read {dir}: {e}"))?
        .filter_map(|e| e.ok())
        .map(|e| {
            let ft = e.file_type().ok();
            let kind = if ft.as_ref().is_some_and(|f| f.is_dir()) {
                "dir"
            } else if ft.as_ref().is_some_and(|f| f.is_symlink()) {
                "symlink"
            } else {
                "file"
            };
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            let name = truncate_chars(&e.file_name().to_string_lossy(), MAX_NAME_CHARS);
            json!({"name": name, "kind": kind, "size": size})
        })
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .cmp(b["name"].as_str().unwrap_or(""))
    });
    let total = entries.len();
    entries.truncate(MAX_ENTRIES);
    let out = json!({
        "dir": dir,
        "total": total,
        "truncated": total > MAX_ENTRIES,
        "entries": entries,
    });
    Ok(serde_json::to_string(&out)?)
}

/// Run a full batch: searches + reads + optional auto_read of hit files.
pub async fn batch(req: BatchRequest) -> Result<BatchResult> {
    if req.searches.is_empty() && req.reads.is_empty() {
        bail!("empty batch: provide at least one search or read");
    }
    let limits = Limits::from_request(&req);
    let searches = if req.searches.is_empty() {
        vec![]
    } else {
        run_searches(&req, &limits, &req.searches)
    };

    // Collect read requests: explicit reads (deduped, order-preserving) + auto_read of hit paths.
    let mut read_paths: Vec<(String, Option<usize>, Option<usize>)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for r in &req.reads {
        if seen.insert(r.path.clone()) {
            read_paths.push((r.path.clone(), r.offset, r.limit));
        }
    }
    let mut auto_read_paths: Vec<String> = Vec::new();
    if req.auto_read {
        let mut auto_seen = std::collections::HashSet::new();
        for s in &searches {
            for h in &s.hits {
                if auto_seen.insert(h.path.clone()) {
                    auto_read_paths.push(h.path.clone());
                }
            }
        }
    }

    let root = PathBuf::from(req.root.as_deref().unwrap_or("."));
    let mut reads: Vec<ReadOutcome> = read_paths
        .par_iter()
        .map(|(p, off, lim)| {
            let resolved = resolve_path(&root, p);
            read_one(&resolved, &limits, *off, *lim)
        })
        .collect();
    if !auto_read_paths.is_empty() {
        let auto: Vec<ReadOutcome> = auto_read_paths
            .par_iter()
            .map(|p| {
                let resolved = resolve_path(&root, p);
                read_one(&resolved, &limits, None, Some(limits.auto_read_limit))
                // note: truncated flag already accounts for the auto cap via limit arithmetic
            })
            .collect();
        reads.extend(auto);
    }

    // Serialize paths to forward slashes for consistency in error paths too.
    for r in &mut reads {
        r.path = r.path.replace('\\', "/");
    }

    let total_chars = {
        let mut n = 0usize;
        for s in &searches {
            for h in &s.hits {
                n += h.preview.len() + h.path.len() + 12;
            }
        }
        for r in &reads {
            if let Some(d) = &r.display {
                for l in d {
                    n += l.len() + 1;
                }
            }
        }
        n
    };

    Ok(BatchResult {
        ok: searches.iter().all(|s| s.error.is_none()) && reads.iter().all(|r| r.error.is_none()),
        error: None,
        searches: (!searches.is_empty()).then_some(searches),
        reads: (!reads.is_empty()).then_some(reads),
        total_chars,
    })
}

fn resolve_path(root: &Path, p: &str) -> PathBuf {
    let p = p.replace('\\', "/");
    if Path::new(&p).is_absolute() {
        PathBuf::from(p)
    } else {
        root.join(p)
    }
}

/// Global output cap applied to the serialized JSON. Returns the truncated string.
pub fn enforce_total_cap(json: &str, cap: usize) -> (String, bool) {
    if json.len() <= cap {
        return (json.to_owned(), false);
    }
    let mut cut = cap;
    while cut > 0 && !json.is_char_boundary(cut) {
        cut -= 1;
    }
    (json[..cut].to_owned(), true)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::write(
            root.join("a.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::write(
            root.join("src/deep/b.py"),
            "def hello():\n    print('hello')\n",
        )
        .unwrap();
        (dir, root)
    }

    #[test]
    fn search_finds_regex_hits() {
        let (_d, root) = fixture();
        let req = BatchRequest {
            searches: vec![SearchQuery {
                pattern: "hello".into(),
                glob: None,
                file_type: None,
                max_results: Some(10),
                context: None,
            }],
            reads: vec![],
            root: Some(root.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let res = rt.block_on(batch(req)).unwrap();
        let s = &res.searches.as_ref().unwrap()[0];
        assert!(s.error.is_none(), "{:?}", s.error);
        assert_eq!(s.hits.len(), 3, "{:?}", s.hits);
        assert_eq!(s.hits[0].path, "a.rs");
        assert_eq!(s.hits[0].line, 2);
        assert!(
            s.hits[1].path.starts_with("src/deep/b.py")
                || s.hits[2].path.starts_with("src/deep/b.py")
        );
        let py_hits: Vec<&Hit> = s.hits.iter().filter(|h| h.path.contains("b.py")).collect();
        assert_eq!(py_hits.len(), 2);
        assert_eq!(py_hits[0].line, 1); // def hello():
        assert_eq!(py_hits[1].line, 2); // print('hello')
        assert_eq!(py_hits[1].col, 12); // col of 'hello' inside print(  -> print('hello' ; h at index 11 (1-based 12)
    }

    #[test]
    fn glob_filters_paths() {
        let (_d, root) = fixture();
        let req = BatchRequest {
            searches: vec![SearchQuery {
                pattern: "hello".into(),
                glob: Some("*.rs".into()),
                file_type: None,
                max_results: None,
                context: None,
            }],
            reads: vec![],
            root: Some(root.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let res = rt.block_on(batch(req)).unwrap();
        let s = &res.searches.as_ref().unwrap()[0];
        // Only a.rs matches *.rs, and it contains "hello" exactly once (line 2).
        assert_eq!(s.hits.len(), 1, "{:?}", s.hits);
        assert_eq!(s.hits[0].path, "a.rs");
        assert_eq!(s.hits[0].line, 2);
    }

    #[tokio::test]
    async fn file_type_filter_works() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("x.md"), "needle\n").unwrap();
        std::fs::write(root.join("x.txt"), "needle\n").unwrap();
        let req = BatchRequest {
            searches: vec![SearchQuery {
                pattern: "needle".into(),
                glob: None,
                file_type: Some("md".into()),
                max_results: None,
                context: None,
            }],
            reads: vec![],
            root: Some(root.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let res = batch(req).await.unwrap();
        let s = &res.searches.as_ref().unwrap()[0];
        assert_eq!(s.hits.len(), 1);
        assert!(s.hits[0].path.ends_with(".md"));
    }

    #[tokio::test]
    async fn reads_dedupe_paths() {
        let (_d, root) = fixture();
        let req = BatchRequest {
            searches: vec![],
            reads: vec![
                ReadRequest {
                    path: "a.rs".into(),
                    offset: None,
                    limit: None,
                },
                ReadRequest {
                    path: "a.rs".into(),
                    offset: Some(1),
                    limit: Some(1),
                },
            ],
            root: Some(root.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let res = batch(req).await.unwrap();
        assert_eq!(res.reads.as_ref().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn offset_limit_are_1_indexed() {
        let (_d, root) = fixture();
        let req = BatchRequest {
            searches: vec![],
            reads: vec![ReadRequest {
                path: "a.rs".into(),
                offset: Some(2),
                limit: Some(1),
            }],
            root: Some(root.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let res = batch(req).await.unwrap();
        let r = &res.reads.as_ref().unwrap()[0];
        assert_eq!(
            r.display.as_ref().unwrap(),
            &vec!["2|    println!(\"hello\");".to_string()]
        );
        assert_eq!(r.line_count, Some(3));
    }

    #[tokio::test]
    async fn binary_files_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("blob.bin"), b"\x00\x01\x02hello\x00").unwrap();
        let req = BatchRequest {
            searches: vec![SearchQuery {
                pattern: "hello".into(),
                glob: None,
                file_type: None,
                max_results: None,
                context: None,
            }],
            reads: vec![ReadRequest {
                path: "blob.bin".into(),
                offset: None,
                limit: None,
            }],
            root: Some(root.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let res = batch(req).await.unwrap();
        assert_eq!(res.searches.as_ref().unwrap()[0].hits.len(), 0);
        let r = &res.reads.as_ref().unwrap()[0];
        assert_eq!(r.error.as_deref(), Some("binary file skipped"));
        assert!(r.display.is_none());
    }

    #[tokio::test]
    async fn auto_read_returns_search_hits_plus_contents() {
        let (_d, root) = fixture();
        let req = BatchRequest {
            searches: vec![SearchQuery {
                pattern: "hello".into(),
                glob: None,
                file_type: None,
                max_results: None,
                context: None,
            }],
            reads: vec![],
            root: Some(root.to_string_lossy().into_owned()),
            auto_read: true,
            auto_read_limit: Some(10),
            ..Default::default()
        };
        let res = batch(req).await.unwrap();
        let reads = res.reads.as_ref().unwrap();
        assert_eq!(reads.len(), 2); // a.rs + src/deep/b.py, deduped
        assert!(reads.iter().all(|r| r.error.is_none()));
        assert!(reads.iter().all(|r| r.display.as_ref().unwrap().len() >= 2));
    }

    #[tokio::test]
    async fn max_results_caps_hits() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let big: String = "needle\n".repeat(500);
        std::fs::write(root.join("big.txt"), &big).unwrap();
        let req = BatchRequest {
            searches: vec![SearchQuery {
                pattern: "needle".into(),
                glob: None,
                file_type: None,
                max_results: Some(50),
                context: None,
            }],
            reads: vec![],
            root: Some(root.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let res = batch(req).await.unwrap();
        let s = &res.searches.as_ref().unwrap()[0];
        assert_eq!(s.hits.len(), 50);
        assert_eq!(s.truncated, Some(50));
    }

    #[tokio::test]
    async fn oversized_files_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("big.txt"), "x".repeat(2048)).unwrap();
        let req = BatchRequest {
            searches: vec![SearchQuery {
                pattern: "x".into(),
                glob: None,
                file_type: None,
                max_results: None,
                context: None,
            }],
            reads: vec![],
            root: Some(root.to_string_lossy().into_owned()),
            max_file_size: Some(1024),
            ..Default::default()
        };
        let res = batch(req).await.unwrap();
        assert_eq!(res.searches.as_ref().unwrap()[0].hits.len(), 0);
    }

    #[tokio::test]
    async fn gitignored_files_excluded_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".gitignore"), "secret*\n").unwrap();
        std::fs::write(root.join("secret.env"), "needle\n").unwrap();
        std::fs::write(root.join("open.txt"), "needle\n").unwrap();
        let mk = |respect: bool| BatchRequest {
            searches: vec![SearchQuery {
                pattern: "needle".into(),
                glob: None,
                file_type: None,
                max_results: None,
                context: None,
            }],
            reads: vec![],
            root: Some(root.to_string_lossy().into_owned()),
            respect_ignore: respect,
            hidden: true,
            ..Default::default()
        };
        let res = batch(mk(true)).await.unwrap();
        let paths: Vec<&str> = res.searches.as_ref().unwrap()[0]
            .hits
            .iter()
            .map(|h| h.path.as_str())
            .collect();
        assert_eq!(paths, vec!["open.txt"]);
        let res2 = batch(mk(false)).await.unwrap();
        assert!(
            res2.searches.as_ref().unwrap()[0]
                .hits
                .iter()
                .any(|h| h.path == "secret.env")
        );
    }

    #[tokio::test]
    async fn symlinks_do_not_loot_the_host() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("f.txt"), "needle\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/", root.join("rootlink")).unwrap();
        let req = BatchRequest {
            searches: vec![SearchQuery {
                pattern: "needle".into(),
                glob: None,
                file_type: None,
                max_results: None,
                context: None,
            }],
            reads: vec![],
            root: Some(root.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let res = batch(req).await.unwrap();
        // Symlink is not followed by default: only the real file is seen.
        let hits = &res.searches.as_ref().unwrap()[0].hits;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "f.txt");
    }

    #[test]
    fn total_cap_truncates_on_char_boundary() {
        let s = "héllo world".repeat(100);
        let (t, truncated) = enforce_total_cap(&s, 50);
        assert!(truncated);
        assert!(std::panic::catch_unwind(|| t.len()).is_ok());
        assert!(t.is_char_boundary(t.len()));
        assert!(t.len() <= 50);
    }

    #[test]
    fn is_binary_detects_nul_and_bad_utf8() {
        assert!(is_binary(b"\x00\x01"));
        assert!(is_binary(&[0xff, 0xfe, 0xfd]));
        assert!(!is_binary(b"plain text"));
        assert!(!is_binary("héllo".as_bytes()));
        // truncated multibyte at the very end of the window is still text
        let mut v = b"abc".to_vec();
        v.extend_from_slice(&[0xe2, 0x82]); // incomplete euro sign at end
        assert!(!is_binary(&v));
    }

    #[tokio::test]
    async fn bad_regex_reports_error_not_panic() {
        let (_d, root) = fixture();
        let req = BatchRequest {
            searches: vec![SearchQuery {
                pattern: "(unclosed".into(),
                glob: None,
                file_type: None,
                max_results: None,
                context: None,
            }],
            reads: vec![],
            root: Some(root.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let res = batch(req).await.unwrap();
        let s = &res.searches.as_ref().unwrap()[0];
        assert!(s.error.is_some());
        assert!(s.hits.is_empty());
    }

    #[tokio::test]
    async fn missing_read_target_is_reported() {
        let (_d, root) = fixture();
        let req = BatchRequest {
            searches: vec![],
            reads: vec![ReadRequest {
                path: "nope.rs".into(),
                offset: None,
                limit: None,
            }],
            root: Some(root.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let res = batch(req).await.unwrap();
        let r = &res.reads.as_ref().unwrap()[0];
        assert!(r.error.is_some());
        assert!(r.error.as_ref().unwrap().starts_with("stat failed"));
    }
}
