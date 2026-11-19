//! WASM bindings for gist-search, built with `--features wasm` targeting
//! `wasm32-unknown-unknown`. Exposes metadata search to the browser; the
//! web app handles network I/O (sync + content fetch) in JavaScript.

use wasm_bindgen::prelude::*;

/// Search gist metadata (descriptions + file names). `index_json` accepts a
/// bare array of gists or the CLI index format. Returns a JSON array of
/// `Hit` objects.
#[wasm_bindgen]
pub fn search(index_json: &str, pattern: &str, ignore_case: bool) -> Result<String, JsError> {
    let gists = crate::parse_index(index_json).map_err(|e| JsError::new(&e))?;
    let hits = crate::search_metadata(&gists, pattern, ignore_case).map_err(|e| JsError::new(&e))?;
    serde_json::to_string(&hits).map_err(|e| JsError::new(&e.to_string()))
}
