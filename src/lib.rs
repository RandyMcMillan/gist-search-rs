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
    pub created_at: String,
    pub updated_at: String,
    pub public: bool,
    pub files: BTreeMap<String, GistFile>,
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

/// Regex search over descriptions and file names. Content search lives in
/// the CLI and the web app because it needs network access per file.
pub fn search_metadata(
    gists: &[GistMeta],
    pattern: &str,
    ignore_case: bool,
) -> Result<Vec<Hit>, String> {
    let re = RegexBuilder::new(pattern)
        .case_insensitive(ignore_case)
        .build()
        .map_err(|e| format!("invalid regex: {e}"))?;

    let mut hits = Vec::new();
    for gist in gists {
        let mut matches = Vec::new();
        if let Some(desc) = &gist.description {
            if re.is_match(desc) {
                matches.push(HitMatch {
                    file: "(description)".to_string(),
                    line: 0,
                    text: desc.clone(),
                });
            }
        }
        for name in gist.files.keys() {
            if re.is_match(name) {
                matches.push(HitMatch {
                    file: name.clone(),
                    line: 0,
                    text: name.clone(),
                });
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
}
