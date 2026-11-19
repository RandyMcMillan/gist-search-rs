//! Core search logic for gist-search, shared by the CLI (`src/main.rs`)
//! and the WASM build (`src/wasm.rs`).

use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[cfg(feature = "wasm")]
mod wasm;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GistFile {
    pub filename: String,
    #[serde(default)]
    pub language: Option<String>,
    pub raw_url: String,
    pub size: u64,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GistMeta {
    pub id: String,
    pub html_url: String,
    pub description: Option<String>,
    /// Owner login. The GitHub API returns `owner` as an object; also
    /// accepts a plain string for hand-built indexes.
    #[serde(default, deserialize_with = "deserialize_owner")]
    pub owner: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub public: bool,
    pub files: BTreeMap<String, GistFile>,
}

fn deserialize_owner<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Owner {
        S(String),
        O { login: String },
    }
    Ok(Option::<Owner>::deserialize(d)?.map(|o| match o {
        Owner::S(s) => s,
        Owner::O { login } => login,
    }))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HitMatch {
    /// File name, or "(description)" for a description hit.
    pub file: String,
    /// 0 = metadata hit, 1-based line number for content hits.
    pub line: usize,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hit {
    pub gist_id: String,
    pub gist_url: String,
    pub description: String,
    pub matches: Vec<HitMatch>,
}

/// Accepts either a bare JSON array of gists or the CLI index format
/// (`{"user": ..., "gists": [...]}`).
pub fn parse_index(json: &str) -> Result<Vec<GistMeta>, String> {
    let trimmed = json.trim_start();
    if trimmed.starts_with('[') {
        serde_json::from_str(trimmed).map_err(|e| e.to_string())
    } else {
        #[derive(Deserialize)]
        struct Wrapper {
            gists: Vec<GistMeta>,
        }
        serde_json::from_str::<Wrapper>(trimmed)
            .map(|w| w.gists)
            .map_err(|e| e.to_string())
    }
}

/// A parsed search query. Supports GitHub-style qualifiers:
/// `user:login`, `language:rust`; everything else is the regex pattern.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    /// Free-text regex (empty = match everything when qualifiers are present).
    pub pattern: String,
    /// `user:` qualifiers, lowercased.
    pub users: Vec<String>,
    /// `language:` qualifiers, lowercased.
    pub languages: Vec<String>,
}

/// Split a query into qualifiers and pattern, e.g.
/// `user:rust-play language:rust "fn main"` →
/// `{ users: ["rust-play"], languages: ["rust"], pattern: "fn main" }`.
pub fn parse_query(input: &str) -> Query {
    let mut query = Query::default();
    let mut pattern = Vec::new();
    for tok in input.split_whitespace() {
        if let Some(v) = tok.strip_prefix("user:") {
            if !v.is_empty() {
                query.users.push(v.to_lowercase());
            }
        } else if let Some(v) = tok.strip_prefix("language:") {
            if !v.is_empty() {
                query.languages.push(v.to_lowercase());
            }
        } else {
            pattern.push(tok);
        }
    }
    query.pattern = pattern.join(" ");
    query
}

fn user_matches(gist: &GistMeta, users: &[String]) -> bool {
    if users.is_empty() {
        return true;
    }
    let owner = gist.owner.as_deref().unwrap_or("").to_lowercase();
    users.iter().any(|u| owner == *u)
}

fn language_matches(file: &GistFile, languages: &[String]) -> bool {
    if languages.is_empty() {
        return true;
    }
    let lang = file.language.as_deref().unwrap_or("").to_lowercase();
    languages.iter().any(|l| lang == *l)
}

/// Regex search over descriptions and file names, honoring `user:` and
/// `language:` qualifiers. Content search lives in the CLI and the web app
/// because it needs network access per file.
pub fn search_metadata(
    gists: &[GistMeta],
    pattern: &str,
    ignore_case: bool,
) -> Result<Vec<Hit>, String> {
    search(gists, &parse_query(pattern), ignore_case)
}

/// Same as `search_metadata` but with a pre-parsed query.
pub fn search(gists: &[GistMeta], query: &Query, ignore_case: bool) -> Result<Vec<Hit>, String> {
    let re = RegexBuilder::new(if query.pattern.is_empty() { ".*" } else { &query.pattern })
        .case_insensitive(ignore_case)
        .build()
        .map_err(|e| format!("invalid regex: {e}"))?;

    let mut hits = Vec::new();
    for gist in gists {
        if !user_matches(gist, &query.users) {
            continue;
        }
        let mut matches = Vec::new();
        // Descriptions are not tied to a file, so skip them when a
        // language filter is active (mirrors GitHub code search behavior).
        if query.languages.is_empty() {
            if let Some(desc) = &gist.description {
                if re.is_match(desc) {
                matches.push(HitMatch {
                    file: "(description)".to_string(),
                    line: 0,
                    text: desc.clone(),
                });
            }
            }
        }
        for (name, file) in &gist.files {
            if !language_matches(file, &query.languages) {
                continue;
            }
            if re.is_match(name) {
                matches.push(HitMatch {
                    file: name.clone(),
                    line: 0,
                    text: name.clone(),
                });
            }
        }
        // Qualifier-only query (no pattern): list the files that pass the filter.
        if query.pattern.is_empty() && matches.is_empty() {
            for (name, file) in &gist.files {
                if language_matches(file, &query.languages) {
                    matches.push(HitMatch {
                        file: name.clone(),
                        line: 0,
                        text: name.clone(),
                    });
                }
            }
        }
        if !matches.is_empty() {
            hits.push(Hit {
                gist_id: gist.id.clone(),
                gist_url: gist.html_url.clone(),
                description: gist.description.clone().unwrap_or_default(),
                matches,
            });
        }
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<GistMeta> {
        serde_json::from_str(
            r#"[
            {
                "id": "abc123",
                "html_url": "https://gist.github.com/rust-play/abc123",
                "description": "Code shared from the Rust Playground",
                "owner": { "login": "rust-play" },
                "created_at": "2026-01-01T00:00:00Z",
                "updated_at": "2026-01-01T00:00:00Z",
                "public": true,
                "files": {
                    "playground.rs": {
                        "filename": "playground.rs",
                        "language": "Rust",
                        "raw_url": "https://gist.githubusercontent.com/rust-play/abc123/raw/playground.rs",
                        "size": 42,
                        "truncated": false
                    }
                }
            },
            {
                "id": "def456",
                "html_url": "https://gist.github.com/rust-play/def456",
                "description": null,
                "owner": { "login": "rust-play" },
                "created_at": "2026-01-02T00:00:00Z",
                "updated_at": "2026-01-02T00:00:00Z",
                "public": true,
                "files": {
                    "notes.md": {
                        "filename": "notes.md",
                        "language": "Markdown",
                        "raw_url": "https://gist.githubusercontent.com/rust-play/def456/raw/notes.md",
                        "size": 10,
                        "truncated": false
                    }
                }
            }
        ]"#,
        )
        .unwrap()
    }

    #[test]
    fn metadata_search_matches_description_and_filename() {
        let gists = sample();
        let hits = search_metadata(&gists, "playground", false).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].gist_id, "abc123");
        assert_eq!(hits[0].matches.len(), 1); // playground.rs (description has capital P)

        let hits = search_metadata(&gists, "playground", true).unwrap();
        assert_eq!(hits[0].matches.len(), 2); // description + playground.rs

        let hits = search_metadata(&gists, "NOTES\\.MD", true).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].gist_id, "def456");
    }

    #[test]
    fn parse_index_accepts_array_and_wrapper() {
        let gists = sample();
        let bare = serde_json::to_string(&gists).unwrap();
        assert_eq!(parse_index(&bare).unwrap().len(), 2);

        let wrapped = format!("{{\"user\":\"rust-play\",\"fetched_at\":\"now\",\"gists\":{bare}}}");
        assert_eq!(parse_index(&wrapped).unwrap().len(), 2);
    }

    #[test]
    fn invalid_regex_is_an_error() {
        assert!(search_metadata(&sample(), "(", false).is_err());
    }

    #[test]
    fn parse_query_splits_qualifiers() {
        let q = parse_query("user:rust-play language:rust fn main");
        assert_eq!(q.users, ["rust-play"]);
        assert_eq!(q.languages, ["rust"]);
        assert_eq!(q.pattern, "fn main");

        let q = parse_query("playground");
        assert!(q.users.is_empty() && q.languages.is_empty());
        assert_eq!(q.pattern, "playground");

        let q = parse_query("language:RUST");
        assert_eq!(q.languages, ["rust"]);
        assert!(q.pattern.is_empty());
    }

    #[test]
    fn language_qualifier_filters_files() {
        let gists = sample();
        let hits = search_metadata(&gists, "language:rust .", false).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].gist_id, "abc123");

        let hits = search_metadata(&gists, "language:markdown .", false).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].gist_id, "def456");

        // Qualifier only, no pattern: lists files passing the filter.
        let hits = search_metadata(&gists, "language:rust", false).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].matches.len(), 1); // playground.rs
    }

    #[test]
    fn user_qualifier_filters_owner() {
        let gists = sample();
        let hits = search_metadata(&gists, "user:rust-play .", false).unwrap();
        assert_eq!(hits.len(), 2);

        let hits = search_metadata(&gists, "user:nobody .", false).unwrap();
        assert!(hits.is_empty());
    }
}
