import init, { search as wasmSearch } from "./pkg/gist_search.js";

const $ = (id) => document.getElementById(id);
const els = {
  user: $("user"), token: $("token"), sync: $("sync"),
  indexinfo: $("indexinfo"), pattern: $("pattern"),
  ignorecase: $("ignorecase"), content: $("content"),
  search: $("search"), status: $("status"), results: $("results"),
  ratelimit: $("ratelimit"),
};

// ---- theme: system / light / dark (Primer data-color-mode) ----
const THEME_KEY = "gist-search:theme";
function applyTheme(theme) {
  // Primer: data-color-mode="auto" follows prefers-color-scheme;
  // data-light-theme/data-dark-theme select which palette "auto" resolves to.
  document.documentElement.setAttribute("data-color-mode", theme === "system" ? "auto" : theme);
  for (const t of ["system", "light", "dark"]) {
    $(`theme-${t}`).setAttribute("aria-pressed", String(t === theme));
  }
  try { localStorage.setItem(THEME_KEY, theme); } catch {}
}
for (const t of ["system", "light", "dark"]) {
  $(`theme-${t}`).addEventListener("click", () => applyTheme(t));
}
{
  let theme = "system";
  try { theme = localStorage.getItem(THEME_KEY) || "system"; } catch {}
  applyTheme(theme);
}

const state = { gists: [], user: null };
const starred = new Set(); // gist ids the user has starred this session

function status(msg, isError = false) {
  els.status.textContent = msg;
  els.status.className = `f6 mb-2 ${isError ? "color-fg-danger" : "color-fg-muted"}`;
}

function storeKey(user) { return `gist-search:index:${user}`; }
function sinceKey(user) { return `gist-search:since:${user}`; }

// ---- rate-limit meter ----
function updateRateMeter(headers) {
  const remaining = headers.get("x-ratelimit-remaining");
  const limit = headers.get("x-ratelimit-limit");
  const reset = headers.get("x-ratelimit-reset");
  if (remaining == null || limit == null) return;
  let resetTxt = "";
  if (reset) resetTxt = ` · resets ${new Date(Number(reset) * 1000).toLocaleTimeString()}`;
  els.ratelimit.textContent = `API ${remaining}/${limit}${resetTxt}`;
}

async function refreshRateMeter() {
  try {
    const headers = { Accept: "application/vnd.github+json" };
    if (els.token.value.trim()) headers.Authorization = `Bearer ${els.token.value.trim()}`;
    const resp = await fetch("https://api.github.com/rate_limit", { headers });
    const data = await resp.json();
    const core = data.resources && data.resources.core;
    if (core) {
      els.ratelimit.textContent = `API ${core.remaining}/${core.limit} · resets ${new Date(core.reset * 1000).toLocaleTimeString()}`;
    }
  } catch { /* leave the meter blank */ }
}

function updateIndexInfo() {
  if (!state.gists.length) {
    els.indexinfo.textContent = "";
    els.search.disabled = true;
    return;
  }
  const files = state.gists.reduce((n, g) => n + Object.keys(g.files || {}).length, 0);
  els.indexinfo.textContent = `${state.user}: ${state.gists.length} gists, ${files} files`;
  els.search.disabled = false;
}

async function loadCached(user) {
  try {
    const raw = localStorage.getItem(storeKey(user));
    if (!raw) return false;
    const parsed = JSON.parse(raw);
    state.gists = Array.isArray(parsed) ? parsed : parsed.gists || [];
    state.user = user;
    return state.gists.length > 0;
  } catch { return false; }
}

function escapeHtml(s) {
  return s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
}

// ---- gist star toggle ----
function starButton(gistId) {
  const on = starred.has(gistId);
  return `<button class="star${on ? " starred" : ""}" data-star="${gistId}" title="${on ? "Unstar" : "Star"} this gist (needs a token with gist scope)">${on ? "★" : "☆"}</button>`;
}

function paintStars() {
  for (const btn of els.results.querySelectorAll("button.star")) {
    const on = starred.has(btn.dataset.star);
    btn.classList.toggle("starred", on);
    btn.textContent = on ? "★" : "☆";
  }
}

async function toggleStar(gistId, btn) {
  const token = els.token.value.trim();
  if (!token) {
    status("starring needs a token with the gist scope — paste one in the token field", true);
    return;
  }
  const want = !starred.has(gistId);
  btn.disabled = true;
  try {
    const headers = { Accept: "application/vnd.github+json", Authorization: `Bearer ${token}` };
    const resp = await fetch(`https://api.github.com/gists/${gistId}/star`, {
      method: want ? "PUT" : "DELETE",
      headers,
    });
    updateRateMeter(resp.headers);
    if (resp.status === 204 || resp.status === 304) {
      if (want) starred.add(gistId); else starred.delete(gistId);
      paintStars();
      status(want ? `starred ${gistId}` : `unstarred ${gistId}`);
    } else {
      throw new Error(`HTTP ${resp.status}`);
    }
  } catch (e) {
    status(`star failed: ${e.message || e}`, true);
  } finally {
    btn.disabled = false;
  }
}

// One delegated handler covers hits rendered at any time.
els.results.addEventListener("click", (e) => {
  const btn = e.target.closest("button.star");
  if (btn) toggleStar(btn.dataset.star, btn);
});

// Render a live preview of the most recent chunk so the user sees
// results immediately instead of waiting for the full sync.
function renderSyncPreview(chunk, page, incremental) {
  const MAX_ROWS = 8;
  const parts = [];
  parts.push(`<p class="f6 color-fg-muted">${incremental ? "updating" : "syncing"} ${state.user} — page ${page}: ${state.gists.length} gists indexed so far (already searchable)</p>`);
  for (const gist of chunk.slice(0, MAX_ROWS)) {
    const desc = gist.description ? escapeHtml(gist.description.slice(0, 100)) : "(no description)";
    parts.push(`<div class="Box p-3 mb-3">${starButton(gist.id)} <a class="text-bold" href="${gist.html_url}">${gist.html_url}</a><p class="f6 color-fg-muted mt-1 mb-0">${desc}</p></div>`);
  }
  if (chunk.length > MAX_ROWS) parts.push(`<p class="f6 color-fg-muted">… and ${chunk.length - MAX_ROWS} more this page</p>`);
  els.results.innerHTML = parts.join("");
}

// Merge a fetched batch into the current index by gist id (updates win).
function mergeBatch(batch) {
  if (!batch.length) return;
  const byId = new Map(state.gists.map((g) => [g.id, g]));
  for (const g of batch) byId.set(g.id, g);
  state.gists = [...byId.values()];
}

async function sync() {
  const user = els.user.value.trim();
  if (!user) return;
  els.sync.disabled = true;
  state.user = user;
  // Start from the cached index if present — re-syncs are then incremental:
  // the `since` watermark (max updated_at from the last sync) means GitHub
  // only returns gists created/updated since then.
  const hadCache = await loadCached(user);
  if (!hadCache) state.gists = [];
  let since = null;
  try { since = localStorage.getItem(sinceKey(user)); } catch {}
  const incremental = Boolean(hadCache && since);
  let page = 1;
  try {
    const headers = { Accept: "application/vnd.github+json" };
    if (els.token.value.trim()) headers.Authorization = `Bearer ${els.token.value.trim()}`;
    for (;;) {
      status(`${incremental ? "updating" : "syncing"} ${user}: page ${page} (${state.gists.length} gists)…`);
      let url = `https://api.github.com/users/${encodeURIComponent(user)}/gists?per_page=100&page=${page}`;
      if (incremental) url += `&since=${encodeURIComponent(since)}`;
      const resp = await fetch(url, { headers });
      updateRateMeter(resp.headers);
      if (resp.status === 403 || resp.status === 429) {
        throw new Error(`GitHub rate limit hit (use a token, or wait for the hourly reset)`);
      }
      if (!resp.ok) throw new Error(`GitHub API returned HTTP ${resp.status}`);
      const batch = await resp.json();
      mergeBatch(batch); // each chunk is searchable the moment it lands
      updateIndexInfo();
      renderSyncPreview(batch, page, incremental);
      if (batch.length < 100) break;
      page += 1;
    }
    // Persist the new watermark = newest updated_at across the whole index.
    let newest = "";
    for (const g of state.gists) if (g.updated_at > newest) newest = g.updated_at;
    try { if (newest) localStorage.setItem(sinceKey(user), newest); } catch {}
    try {
      localStorage.setItem(storeKey(user), JSON.stringify(state.gists));
      status(`${incremental ? "updated" : "synced"} ${state.gists.length} gists for '${user}' (cached in localStorage)`);
    } catch {
      status(`${incremental ? "updated" : "synced"} ${state.gists.length} gists for '${user}' (too big for localStorage; kept in memory only)`);
    }
  } catch (e) {
    status(String(e.message || e), true);
  } finally {
    els.sync.disabled = false;
    updateIndexInfo();
  }
}

function renderHits(hits, limit = 200) {
  const parts = [];
  parts.push(`<p class="f6 color-fg-muted">${hits.length} gist(s) matched</p>`);
  for (const hit of hits.slice(0, limit)) {
    parts.push(`<div class="Box p-3 mb-3">${starButton(hit.gist_id)} <a href="${hit.gist_url}">${hit.gist_url}</a>`);
    if (hit.description) parts.push(`<p class="f6 color-fg-muted mt-1 mb-2">${escapeHtml(hit.description)}</p>`);
    for (const m of hit.matches) {
      const where = m.line === 0
        ? `<span class="color-fg-attention">[meta] ${escapeHtml(m.file)}</span>`
        : `<span class="color-fg-muted">${escapeHtml(m.file)}:${m.line}:</span>`;
      parts.push(`<p class="mono f6 m-0" style="white-space:pre-wrap;word-break:break-all">${where} ${escapeHtml(m.text)}</p>`);
    }
    parts.push("</div>");
  }
  if (hits.length > limit) parts.push(`<p class="f6 color-fg-muted">… and ${hits.length - limit} more</p>`);
  els.results.innerHTML = parts.join("");
}

function mergeHits(hits) {
  const byId = new Map();
  for (const h of hits) {
    const existing = byId.get(h.gist_id);
    if (existing) existing.matches.push(...h.matches);
    else byId.set(h.gist_id, h);
  }
  return [...byId.values()].sort((a, b) => b.gist_id.localeCompare(a.gist_id));
}

// Mirror of the Rust parse_query: `user:` / `language:` qualifiers, rest is pattern.
function parseQualifiers(query) {
  const users = [], languages = [];
  const rest = [];
  for (const tok of query.split(/\s+/)) {
    if (tok.startsWith("user:") && tok.length > 5) users.push(tok.slice(5).toLowerCase());
    else if (tok.startsWith("language:") && tok.length > 9) languages.push(tok.slice(9).toLowerCase());
    else rest.push(tok);
  }
  return { users, languages, pattern: rest.join(" ") };
}

// Content search: fetch raw files (concurrency pool of 8) and grep in JS.
// JS RegExp syntax differs slightly from Rust's regex crate; that's acceptable here.
async function contentSearch(query, ignoreCase) {
  const { users, languages, pattern } = parseQualifiers(query);
  const re = new RegExp(pattern || ".*", ignoreCase ? "i" : "");
  const work = [];
  for (const gist of state.gists) {
    if (users.length) {
      const owner = (gist.owner?.login || gist.owner || "").toLowerCase();
      if (!users.includes(owner)) continue;
    }
    for (const file of Object.values(gist.files || {})) {
      if (languages.length) {
        const lang = (file.language || "").toLowerCase();
        if (!languages.includes(lang)) continue;
      }
      work.push([gist, file]);
    }
  }
  const hits = [];
  let done = 0, next = 0;
  const MAX_LINE = 300;

  async function worker() {
    while (next < work.length) {
      const [gist, file] = work[next++];
      try {
        const resp = await fetch(file.raw_url);
        if (!resp.ok) throw new Error(String(resp.status));
        const body = await resp.text();
        const matches = [];
        body.split("\n").forEach((line, i) => {
          if (re.test(line)) {
            matches.push({
              file: file.filename,
              line: i + 1,
              text: line.length > MAX_LINE ? line.slice(0, MAX_LINE) + " …" : line,
            });
          }
        });
        if (matches.length) {
          hits.push({
            gist_id: gist.id,
            gist_url: gist.html_url,
            description: gist.description || "",
            matches,
          });
        }
      } catch { /* skip unreadable files */ }
      done += 1;
      if (done % 50 === 0 || done === work.length) status(`content: ${done}/${work.length} files`);
    }
  }

  await Promise.all(Array.from({ length: 8 }, worker));
  return hits;
}

async function runSearch() {
  const pattern = els.pattern.value;
  if (!pattern || !state.gists.length) return;
  els.search.disabled = true;
  try {
    status("searching…");
    let hits;
    try {
      hits = JSON.parse(wasmSearch(JSON.stringify(state.gists), pattern, els.ignorecase.checked));
    } catch (e) {
      throw new Error(`invalid regex: ${String(e.message || e).split("\n")[0]}`);
    }
    if (els.content.checked) {
      hits = mergeHits([...hits, ...(await contentSearch(pattern, els.ignorecase.checked))]);
    }
    renderHits(hits);
    status("");
  } catch (e) {
    status(String(e.message || e), true);
  } finally {
    els.search.disabled = false;
  }
}

els.sync.addEventListener("click", sync);
els.search.addEventListener("click", runSearch);
els.pattern.addEventListener("keydown", (e) => { if (e.key === "Enter") runSearch(); });
els.user.addEventListener("change", async () => {
  const user = els.user.value.trim();
  state.gists = []; state.user = null;
  if (user && await loadCached(user)) status(`loaded cached index for '${user}'`);
  updateIndexInfo();
});
els.token.addEventListener("change", refreshRateMeter);

await init();
refreshRateMeter();
if (await loadCached(els.user.value.trim())) {
  status(`loaded cached index for '${els.user.value.trim()}'`);
}
updateIndexIndexInfoGuard();
function noop() {}
