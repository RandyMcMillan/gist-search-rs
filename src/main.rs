use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use gist_search::{GistFile, GistMeta, Hit, HitMatch};

const PER_PAGE: usize = 100;
const CONTENT_WORKERS: usize = 8;
const MAX_LINE_LEN: usize = 300;

#[derive(Parser)]
#[command(
    name = "gist-search",
    about = "Search all gists created by a GitHub user (default: rust-play)",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Fetch the full gist index for a user and store it locally.
    Sync {
        /// GitHub user whose gists should be indexed.
        #[arg(long, default_value = "rust-play")]
        user: String,

        /// Stop after N pages (1 page = 100 gists). Useful for testing.
        #[arg(long)]
        max_pages: Option<usize>,
    },
    /// Search the local gist index (descriptions + file names; add --content to grep file bodies).
    Search {
        /// Regex pattern to search for.
        pattern: String,

        /// GitHub user whose index should be searched.
        #[arg(long, default_value = "rust-play")]
        user: String,

        /// Also download and search inside file contents.
        #[arg(long)]
        content: bool,

        /// Case-insensitive search.
        #[arg(short = 'i', long)]
        ignore_case: bool,

        /// Maximum number of gists to print.
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Show a summary of the local index.
    Info {
        #[arg(long, default_value = "rust-play")]
        user: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct Index {
    user: String,
    fetched_at: String,
    gists: Vec<GistMeta>,
}

fn cache_dir(user: &str) -> Result<PathBuf> {
    let dir = dirs::cache_dir()
        .context("cannot determine cache directory")?
        .join("gist-search")
        .join(user);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn index_path(user: &str) -> Result<PathBuf> {
    Ok(cache_dir(user)?.join("index.json"))
}

fn http_client() -> Result<reqwest::blocking::Client> {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::ACCEPT,
        reqwest::header::HeaderValue::from_static("application/vnd.github+json"),
    );
    headers.insert(
        reqwest::header::USER_AGENT,
        reqwest::header::HeaderValue::from_static("gist-search/0.1"),
    );
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        let value = format!("Bearer {token}");
        headers.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(&value)?,
        );
    }
    reqwest::blocking::Client::builder()
        .default_headers(headers)
        .build()
        .context("failed to build HTTP client")
}

fn sync(user: &str, max_pages: Option<usize>) -> Result<()> {
    let client = http_client()?;
    let mut gists: Vec<GistMeta> = Vec::new();
    let mut page = 1usize;

    loop {
        if let Some(max) = max_pages {
            if page > max {
                break;
            }
        }
        let url = format!(
            "https://api.github.com/users/{user}/gists?per_page={PER_PAGE}&page={page}"
        );
        let resp = client.get(&url).send().context("request failed")?;
        let status = resp.status();
        if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        {
            let remaining = resp
                .headers()
                .get("x-ratelimit-remaining")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("?");
            anyhow::bail!(
                "GitHub rate limit hit (remaining: {remaining}). Set a GITHUB_TOKEN env var to raise the limit."
            );
        }
        if !status.is_success() {
            anyhow::bail!("GitHub API returned {status} for {url}");
        }
        let batch: Vec<GistMeta> = resp
            .json()
            .context("failed to parse gist list response")?;
        let n = batch.len();
        gists.extend(batch);
        eprint!("\rpage {page}: {n} gists (total {total})", total = gists.len());
        std::io::stderr().flush().ok();

        if n < PER_PAGE {
            break;
        }
        page += 1;
    }
    eprintln!();

    let index = Index {
        user: user.to_string(),
        fetched_at: chrono_now(),
        gists,
    };
    let path = index_path(user)?;
    std::fs::write(&path, serde_json::to_string_pretty(&index)?)?;
    println!(
        "indexed {} gists for '{user}' -> {}",
        index.gists.len(),
        path.display()
    );
    Ok(())
}

fn chrono_now() -> String {
    // Minimal RFC 3339 timestamp without pulling in chrono.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    format_unix(secs)
}

fn format_unix(t: i64) -> String {
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    // civil-from-days (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

fn load_index(user: &str) -> Result<Index> {
    let path = index_path(user)?;
    let data = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "no local index for '{user}' ({}). Run `gist-search sync --user {user}` first.",
            path.display()
        )
    })?;
    Ok(serde_json::from_str(&data)?)
}

fn info(user: &str) -> Result<()> {
    let index = load_index(user)?;
    let files: usize = index.gists.iter().map(|g| g.files.len()).sum();
    let bytes: u64 = index
        .gists
        .iter()
        .flat_map(|g| g.files.values())
        .map(|f| f.size)
        .sum();
    println!("user:        {}", index.user);
    println!("fetched at:  {}", index.fetched_at);
    println!("gists:       {}", index.gists.len());
    println!("files:       {files}");
    println!("total bytes: {bytes}");
    Ok(())
}

fn search(user: &str, pattern: &str, content: bool, ignore_case: bool, limit: usize) -> Result<()> {
    let index = load_index(user)?;

    // 1) metadata pass: description + file names (shared with the WASM build).
    let mut hits: Vec<Hit> =
        gist_search::search_metadata(&index.gists, pattern, ignore_case).map_err(anyhow::Error::msg)?;

    // Shared with the content pass below.
    let re = RegexBuilder::new(pattern)
        .case_insensitive(ignore_case)
        .build()
        .with_context(|| format!("invalid regex: {pattern}"))?;

    // 2) content pass: grep every file body.
    if content {
        let work: Vec<(GistMeta, GistFile)> = index
            .gists
            .iter()
            .cloned()
            .flat_map(|g| {
                let gg = g.clone();
                g.files
                    .into_values()
                    .map(move |f| (gg.clone(), f))
            })
            .collect();
        let total_files = work.len();
        let done = Arc::new(AtomicUsize::new(0));
        let queue: Arc<Mutex<std::collections::VecDeque<(GistMeta, GistFile)>>> =
            Arc::new(Mutex::new(work.into()));
        let found: Arc<Mutex<Vec<Hit>>> = Arc::new(Mutex::new(Vec::new()));
        let re = Arc::new(re);

        std::thread::scope(|s| {
            for _ in 0..CONTENT_WORKERS {
                let queue = Arc::clone(&queue);
                let found = Arc::clone(&found);
                let done = Arc::clone(&done);
                let re = Arc::clone(&re);
                s.spawn(move || loop {
                    let item = queue.lock().unwrap().pop_front();
                    let (gist, file) = match item {
                        Some(it) => it,
                        None => break,
                    };
                    if let Ok(body) = fetch_text(&file.raw_url) {
                        let mut matches = Vec::new();
                        for (i, line) in body.lines().enumerate() {
                            if re.is_match(line) {
                                let line = if line.chars().count() > MAX_LINE_LEN {
                                    line.chars().take(MAX_LINE_LEN).collect::<String>() + " …"
                                } else {
                                    line.to_string()
                                };
                                matches.push(HitMatch {
                                    file: file.filename.clone(),
                                    line: i + 1,
                                    text: line,
                                });
                            }
                        }
                        if !matches.is_empty() {
                            found.lock().unwrap().push(Hit {
                                gist_id: gist.id.clone(),
                                gist_url: gist.html_url.clone(),
                                description: gist.description.clone().unwrap_or_default(),
                                matches,
                            });
                        }
                    }
                    let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if n % 50 == 0 || n == total_files {
                        eprint!("\rcontent: {n}/{total_files} files");
                        std::io::stderr().flush().ok();
                    }
                });
            }
        });
        eprintln!();
        hits.extend(Arc::into_inner(found).unwrap().into_inner().unwrap());
    }

    // Deduplicate: merge hits for the same gist.
    let mut by_id: BTreeMap<String, Hit> = BTreeMap::new();
    for mut hit in hits {
        match by_id.get_mut(&hit.gist_id) {
            Some(existing) => existing.matches.append(&mut hit.matches),
            None => {
                by_id.insert(hit.gist_id.clone(), hit);
            }
        }
    }
    let mut hits: Vec<Hit> = by_id.into_values().collect();
    hits.sort_by(|a, b| b.gist_id.cmp(&a.gist_id)); // newest-ish first (ids are roughly time ordered)

    if hits.is_empty() {
        println!("no matches for /{pattern}/");
        return Ok(());
    }
    println!("{} gist(s) matched /{pattern}/\n", hits.len());
    for hit in hits.iter().take(limit) {
        println!("== {} {}", hit.gist_id, hit.gist_url);
        if !hit.description.is_empty() {
            let desc: String = hit
                .description
                .chars()
                .take(120)
                .collect();
            println!("   {desc}");
        }
        for m in &hit.matches {
            if m.line == 0 {
                println!("   [meta] {}: {}", m.file, m.text);
            } else {
                println!("   {}:{}: {}", m.file, m.line, m.text);
            }
        }
        println!();
    }
    if hits.len() > limit {
        println!("… and {} more (raise --limit)", hits.len() - limit);
    }
    Ok(())
}

fn fetch_text(url: &str) -> Result<String> {
    let client = http_client()?;
    let resp = client.get(url).send().context("request failed")?;
    if !resp.status().is_success() {
        anyhow::bail!("HTTP {}", resp.status());
    }
    let mut buf = String::new();
    resp.take(2_000_000).read_to_string(&mut buf).ok();
    Ok(buf)
}

fn main() -> Result<()> {
    // Die silently on SIGPIPE like a normal Unix CLI (e.g. when piping to `head`).
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    let cli = Cli::parse();
    match cli.command {
        Command::Sync { user, max_pages } => sync(&user, max_pages),
        Command::Search {
            pattern,
            user,
            content,
            ignore_case,
            limit,
        } => search(&user, &pattern, content, ignore_case, limit),
        Command::Info { user } => info(&user),
    }
}
