import init, { search as wasmSearch } from "./pkg/gist_search.js";

const $ = (id) => document.getElementById(id);
const els = {
  user: $("user"), token: $("token"), sync: $("sync"),
  indexinfo: $("indexinfo"), pattern: $("pattern"),
  ignorecase: $("ignorecase"), content: $("content"),
  search: $("search"), status: $("status"), results: $("results"),
};

// ---- theme: system / light / dark ----
const THEME_KEY = "gist-search:theme";
function applyTheme(theme) {
  document.documentElement.dataset.theme = theme;
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

function status(msg, isError = false) {
  els.status.textContent = msg;
  els.status.className = isError ? "error" : "";
}

function storeKey(user) { return `gist-search:index:${user}`; }

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

async function sync() {
  const user = els.user.value.trim();
  if (!user) return;
  els.sync.disabled = true;
  try {
    const headers = { Accept: "application/vnd.github+json" };
    if (els.token.value.trim()) headers.Authorization = `Bearer ${els.token.value.trim()}`;
    const gists = [];
    let page = 1;
    for (;;) {
      status(`syncing ${user}: page ${page} (${gists.length} gists)…`);
      const resp = await fetch(
        `https://api.github.com/users/${encodeURIComponent(user)}/gists?per_page=100&page=${page}`,
        { headers },
      );
      if (resp.status === 403 || resp.status === 429) {
        throw new Error(`GitHub rate limit hit (use a token, or wait for the hourly reset)`);
      }
      if (!resp.ok) throw new Error(`GitHub API returned HTTP ${resp.status}`);
      const batch = await resp.json();
      gists.push(...batch);
      if (batch.length < 100) break;
      page += 1;
    }
    state.gists = gists;
    state.user = user;
    try {
      localStorage.setItem(storeKey(user), JSON.stringify(gists));
      status(`synced ${gists.length} gists for '${user}' (cached in localStorage)`);
    } catch {
      status(`synced ${gists.length} gists for '${user}' (too big for localStorage; kept in memory only)`);
    }
  } catch (e) {
    status(String(e.message || e), true);
  } finally {
    els.sync.disabled = false;
    updateIndexInfo();
  }
}

function escapeHtml(s) {
  return s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
}

function renderHits(hits, limit = 200) {
  const parts = [];
  parts.push(`<p class="sub">${hits.length} gist(s) matched</p>`);
  for (const hit of hits.slice(0, limit)) {
    parts.push(`<div class="hit"><a href="${hit.gist_url}">${hit.gist_url}</a>`);
    if (hit.description) parts.push(`<p class="desc">${escapeHtml(hit.description)}</p>`);
    for (const m of hit.matches) {
      const where = m.line === 0
        ? `<span class="meta">[meta] ${escapeHtml(m.file)}</span>`
        : `<span class="lineno">${escapeHtml(m.file)}:${m.line}:</span>`;
      parts.push(`<p class="match">${where} ${escapeHtml(m.text)}</p>`);
    }
    parts.push("</div>");
  }
  if (hits.length > limit) parts.push(`<p class="sub">… and ${hits.length - limit} more</p>`);
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

await init();
if (await loadCached(els.user.value.trim())) {
  status(`loaded cached index for '${els.user.value.trim()}'`);
}
updateIndexInfo();
