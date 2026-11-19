# gist-search

Search every gist created by https://gist.github.com/rust-play (or any other GitHub user).

## How it works

1. `sync` paginates through the GitHub API (`GET /users/{user}/gists`, 100 per page)
   and stores a local index (ids, descriptions, file names, raw URLs) in
   `~/.cache/gist-search/{user}/index.json`.
2. `search` runs a regex against the local index — descriptions and file names
   by default, plus full file bodies with `--content` (fetched from
   `gist.githubusercontent.com` over 8 worker threads; raw fetches don't count
   against the GitHub API rate limit).

Set `GITHUB_TOKEN` to raise the API rate limit (60 req/h anonymous → 5,000 req/h).

## Usage

```sh
# build
cargo build --release

# fetch the full index (optionally limit pages while testing)
gist-search sync
gist-search sync --max-pages 2

# summary of the local index
gist-search info

# search descriptions + file names
gist-search search "iterator"
gist-search search -i "lazy"

# also grep inside every file body
gist-search search "unsafe" --content

# other users
gist-search sync --user torvalds
gist-search search "kernel" --user torvalds --content
```

## Notes

- The `rust-play` account has thousands of gists, so a full content search
  downloads many files; only matched lines are printed (truncated at 300 chars).
- Pattern syntax is Rust `regex` crate syntax.

## Web app (WASM)

The search core (`src/lib.rs`) also compiles to WASM (`src/wasm.rs`). A small
web UI in `web/` syncs the index straight from the browser (GitHub's API is
CORS-enabled) and searches metadata in Rust-compiled WASM; `--content`-style
search fetches raw files from `gist.githubusercontent.com` in the browser.

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.130 --locked
cargo build --release --lib --features wasm --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir web/pkg --out-name gist_search \
  target/wasm32-unknown-unknown/release/gist_search.wasm
# serve web/ with any static file server
```

## GitHub Pages

Merging to `master` runs `.github/workflows/pages.yml`, which builds the WASM,
generates the bindings, and deploys `web/` to GitHub Pages. Enable Pages once
in Settings → Pages → Source: GitHub Actions.
