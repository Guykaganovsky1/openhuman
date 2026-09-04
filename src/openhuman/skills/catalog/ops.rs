//! Business logic for the skill registry: fetch, index, search, and install.
//!
//! The catalog is sourced from the HermesHub aggregated JSON API which
//! includes skills from HermesHub (built-in + optional), ClawHub, skills.sh,
//! LobeHub, and browse.sh — all accessible from a single endpoint.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Mutex;

use super::store;
use super::store::CachedCatalog;
use super::types::CatalogEntry;

const CATALOG_URL: &str = "https://hermes-agent.nousresearch.com/docs/api/skills.json";
const CATALOG_URL_ENV: &str = "OPENHUMAN_SKILL_REGISTRY_CATALOG_URL";
const DOWNLOAD_BASE_URL_ENV: &str = "OPENHUMAN_SKILL_REGISTRY_DOWNLOAD_BASE_URL";
const REFRESH_ON_BOOT_ENV: &str = "OPENHUMAN_SKILL_REGISTRY_REFRESH_ON_BOOT";
const FETCH_TIMEOUT_SECS: u64 = 180;

/// Largest page a `browse` caller may ask for. The catalog holds ~90k entries
/// (~39 MB serialized), so an unbounded `limit` would just re-create the whole
/// payload the paging exists to avoid.
pub const MAX_BROWSE_LIMIT: usize = 200;

/// Single-flight gate for catalog fetches. On mount the skills explorer issues
/// several catalog reads that each funnel into [`browse_catalog`] — `sources`
/// and `browse` from two separate effects, plus `search` as the user types —
/// and React StrictMode double-invokes those effects in dev, so a handful of
/// reads land within the same instant. Without this lock each would issue its
/// own ~80s download of the same ~90k-entry catalog. Concurrent cache-miss
/// callers serialize here, and all but the first re-read the just-written cache
/// instead of hitting the network.
static FETCH_LOCK: Mutex<()> = Mutex::const_new(());

/// True while a background (stale-while-revalidate) refresh is scheduled or
/// running, so a burst of stale-cache reads spawns at most one refresh task.
static REFRESHING: AtomicBool = AtomicBool::new(false);

/// Clears [`REFRESHING`] when the background refresh task ends (incl. panic).
struct RefreshGuard;
impl Drop for RefreshGuard {
    fn drop(&mut self) {
        REFRESHING.store(false, Ordering::Release);
    }
}

/// Start a one-shot background refresh of the remote skills catalog.
///
/// This is intended for core startup: it warms the explorer/search cache without
/// making core readiness depend on registry availability. Set
/// `OPENHUMAN_SKILL_REGISTRY_REFRESH_ON_BOOT=0` to disable it in constrained
/// environments.
pub fn start_boot_catalog_refresh() {
    static STARTED: std::sync::Once = std::sync::Once::new();

    STARTED.call_once(|| {
        if !refresh_on_boot_enabled(std::env::var(REFRESH_ON_BOOT_ENV).ok().as_deref()) {
            tracing::info!(
                env = REFRESH_ON_BOOT_ENV,
                "[skill_registry] boot catalog refresh disabled"
            );
            return;
        }

        tracing::info!("[skill_registry] scheduling boot catalog refresh");
        tokio::spawn(async {
            let started = std::time::Instant::now();
            match browse_catalog(true).await {
                Ok(entries) => {
                    tracing::info!(
                        count = entries.len(),
                        elapsed_ms = started.elapsed().as_millis(),
                        "[skill_registry] boot catalog refresh complete"
                    );
                }
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        elapsed_ms = started.elapsed().as_millis(),
                        "[skill_registry] boot catalog refresh failed"
                    );
                }
            }
        });
    });
}

fn refresh_on_boot_enabled(raw: Option<&str>) -> bool {
    let Some(raw) = raw else { return true };
    let value = raw.trim();
    !(value == "0"
        || value.eq_ignore_ascii_case("false")
        || value.eq_ignore_ascii_case("no")
        || value.eq_ignore_ascii_case("off"))
}

/// Whether a past-TTL (stale) cache may be served without a network round-trip.
#[derive(Clone, Copy, PartialEq)]
enum StaleMode {
    /// Serve stale immediately + revalidate in the background — for the
    /// unfiltered browse, where a slightly-old catalog is fine and speed wins.
    Allow,
    /// Treat stale as a miss and fetch fresh under the single-flight lock — for
    /// search / filter reads, which must reflect the current catalog.
    Reject,
}

/// Fetch the full catalog for the **unfiltered browse** view, accepting a stale
/// cache (stale-while-revalidate):
/// - **Fresh cache** → returned immediately.
/// - **Stale cache** (past TTL) → returned immediately *and* a single background
///   refresh is kicked off, so the explorer renders from the last-known catalog
///   instead of blocking on the ~80s download.
/// - **No cache** → fetch under the single-flight lock; concurrent callers
///   coalesce onto that one request.
///
/// `force_refresh == true` (boot warm-up / explicit refresh) always re-fetches.
/// Search / filter reads use [`browse_catalog_fresh`], which never serves stale.
pub async fn browse_catalog(force_refresh: bool) -> Result<Vec<CatalogEntry>, String> {
    browse_catalog_with(force_refresh, StaleMode::Allow, fetch_catalog_uncached).await
}

/// Fetch the full catalog for **search / filter** reads. Never serves a stale
/// cache: a fresh cache is used as-is, but a stale-or-absent cache falls through
/// to a (single-flight) fresh fetch so results aren't computed over an outdated
/// catalog. Thanks to single-flight, a search issued while a background
/// revalidation is already running simply awaits that in-flight fetch rather
/// than starting a new one.
pub async fn browse_catalog_fresh() -> Result<Vec<CatalogEntry>, String> {
    browse_catalog_with(false, StaleMode::Reject, fetch_catalog_uncached).await
}

/// Core of [`browse_catalog`] / [`browse_catalog_fresh`], parameterised over the
/// fetcher so the cache / single-flight orchestration can be unit-tested without
/// real network I/O.
async fn browse_catalog_with<F, Fut>(
    force_refresh: bool,
    stale_mode: StaleMode,
    fetch: F,
) -> Result<Vec<CatalogEntry>, String>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Vec<CatalogEntry>, String>>,
{
    if !force_refresh {
        match store::load_cached_catalog_state() {
            Some(CachedCatalog::Fresh(entries)) => {
                tracing::debug!(
                    count = entries.len(),
                    "[skill_registry] serving fresh cache"
                );
                return Ok(entries);
            }
            // Browse: serve stale now, revalidate in background.
            Some(CachedCatalog::Stale(entries)) if stale_mode == StaleMode::Allow => {
                tracing::info!(
                    count = entries.len(),
                    "[skill_registry] serving stale cache; revalidating in background"
                );
                spawn_background_refresh();
                return Ok(entries);
            }
            // Search / filter: stale is not good enough — fall through to fetch.
            Some(CachedCatalog::Stale(_)) => {
                tracing::debug!("[skill_registry] stale cache rejected for fresh read; fetching");
            }
            None => {}
        }
    }

    // Single-flight: only one fetch runs at a time. Callers that queued behind
    // the lock re-check the cache below and reuse the just-fetched result.
    let _guard = FETCH_LOCK.lock().await;
    if !force_refresh {
        if let Some(CachedCatalog::Fresh(entries)) = store::load_cached_catalog_state() {
            tracing::debug!(
                count = entries.len(),
                "[skill_registry] cache populated by concurrent fetch; reusing"
            );
            return Ok(entries);
        }
    }

    fetch().await
}

/// Spawn at most one background catalog refresh (stale-while-revalidate). Extra
/// calls while a refresh is in flight no-op via [`REFRESHING`]. The refresh runs
/// under [`FETCH_LOCK`] so it never races a foreground fetch.
fn spawn_background_refresh() {
    if REFRESHING.swap(true, Ordering::AcqRel) {
        return;
    }
    tokio::spawn(async {
        let _reset = RefreshGuard;
        let _guard = FETCH_LOCK.lock().await;
        match fetch_catalog_uncached().await {
            Ok(entries) => tracing::info!(
                count = entries.len(),
                "[skill_registry] background catalog refresh complete"
            ),
            Err(error) => {
                tracing::warn!(error = %error, "[skill_registry] background catalog refresh failed")
            }
        }
    });
}

/// Download, parse, index, and cache the catalog — the network path, unguarded.
/// Callers must go through [`browse_catalog_with`] / [`spawn_background_refresh`]
/// so this runs under the single-flight lock.
async fn fetch_catalog_uncached() -> Result<Vec<CatalogEntry>, String> {
    let catalog_url = catalog_url();
    tracing::info!(
        catalog_url = %redact_url_for_log(&catalog_url),
        "[skill_registry] fetching catalog"
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("failed to build http client: {e}"))?;

    let response = client
        .get(&catalog_url)
        .header("User-Agent", "openhuman-core")
        .send()
        .await
        .map_err(|e| format!("catalog fetch failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "catalog returned status {}",
            response.status().as_u16()
        ));
    }

    let body = response
        .text()
        .await
        .map_err(|e| format!("failed to read response: {e}"))?;

    let raw_items: Vec<serde_json::Value> = parse_catalog_json(&body)?;

    tracing::info!(
        total_raw = raw_items.len(),
        "[skill_registry] parsing catalog"
    );

    let entries: Vec<CatalogEntry> = raw_items.iter().filter_map(parse_hermes_entry).collect();

    tracing::info!(count = entries.len(), "[skill_registry] catalog indexed");

    store::save_catalog_cache(&entries);
    Ok(entries)
}

fn catalog_url() -> String {
    std::env::var(CATALOG_URL_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| CATALOG_URL.to_string())
}

fn redact_url_for_log(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(parsed) => {
            let scheme = parsed.scheme();
            let host = parsed.host_str().unwrap_or("");
            let path = parsed.path();
            format!("{scheme}://{host}{path}")
        }
        Err(_) => "<unparseable>".to_string(),
    }
}

pub(crate) fn parse_catalog_json(body: &str) -> Result<Vec<serde_json::Value>, String> {
    serde_json::from_str(body).map_err(|e| format!("invalid catalog json: {e}"))
}

/// One window of the catalog, plus the size of the filtered set it was cut out
/// of so a caller can render "60 / 90,696" and decide whether more pages exist.
#[derive(Debug, Clone)]
pub struct CatalogPage {
    pub entries: Vec<CatalogEntry>,
    pub total: usize,
}

/// Browse the catalog server-side: filter by `query` / `sources`, then return
/// only the `offset .. offset + limit` window of the result.
///
/// `limit == None` returns everything from `offset` on — with all four
/// arguments at their defaults this is exactly [`browse_catalog`], which is what
/// keeps a legacy (`force_refresh`-only) call byte-identical.
pub async fn browse_catalog_page(
    force_refresh: bool,
    query: Option<&str>,
    sources: Option<&[String]>,
    offset: usize,
    limit: Option<usize>,
) -> Result<CatalogPage, String> {
    browse_catalog_page_with(
        force_refresh,
        query,
        sources,
        offset,
        limit,
        fetch_catalog_uncached,
    )
    .await
}

/// `browse_catalog_page` with the fetcher injected, so the stale-cache
/// decision below is testable at the function the RPC actually calls rather
/// than only at its predicate.
async fn browse_catalog_page_with<F, Fut>(
    force_refresh: bool,
    query: Option<&str>,
    sources: Option<&[String]>,
    offset: usize,
    limit: Option<usize>,
    fetch: F,
) -> Result<CatalogPage, String>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Vec<CatalogEntry>, String>>,
{
    tracing::debug!(
        force_refresh,
        query = ?query,
        sources = ?sources,
        offset,
        limit = ?limit,
        "[skill_registry] browse_catalog_page"
    );
    // A filtered read may not be computed over a stale catalog. The unfiltered
    // read serves a stale cache while it revalidates, which is right for "show
    // me the catalog" — the user sees rows immediately and they refresh
    // underneath. It is wrong for "show me the rows matching X": a skill that
    // has just appeared is simply absent, and the caller cannot tell that from
    // "no match". So a filtered read rejects a stale cache and fetches.
    let stale_mode = if is_filtered_read(query, sources) {
        StaleMode::Reject
    } else {
        StaleMode::Allow
    };
    let catalog = browse_catalog_with(force_refresh, stale_mode, fetch).await?;
    let page = page_catalog(catalog, query, sources, offset, limit);
    tracing::debug!(
        returned = page.entries.len(),
        total = page.total,
        "[skill_registry] browse page cut"
    );
    Ok(page)
}

/// Whether this browse is a *filtered* read — one whose answer is a subset the
/// caller cannot audit, so a skill missing from it is indistinguishable from a
/// skill that does not match.
///
/// An empty or whitespace-only `query` and an empty `sources` list are the
/// unfiltered defaults, not filters; treating them as filters would push every
/// plain catalog open onto the no-stale path and lose the instant first paint
/// the stale cache exists to give.
fn is_filtered_read(query: Option<&str>, sources: Option<&[String]>) -> bool {
    query.is_some_and(|q| !q.trim().is_empty()) || sources.is_some_and(|s| !s.is_empty())
}

/// Pure filter-then-slice half of [`browse_catalog_page`], split out so the
/// paging can be unit-tested without touching the cache or the network.
fn page_catalog(
    catalog: Vec<CatalogEntry>,
    query: Option<&str>,
    sources: Option<&[String]>,
    offset: usize,
    limit: Option<usize>,
) -> CatalogPage {
    let query = query
        .map(|q| q.trim().to_lowercase())
        .filter(|q| !q.is_empty());

    let filtered: Vec<CatalogEntry> = catalog
        .into_iter()
        .filter(|entry| {
            if let Some(allowed) = sources {
                if !allowed.is_empty()
                    && !allowed.iter().any(|s| entry.source.eq_ignore_ascii_case(s))
                {
                    return false;
                }
            }
            match query.as_deref() {
                Some(q) => entry_matches_query(entry, q),
                None => true,
            }
        })
        .collect();

    let total = filtered.len();
    // Clamp rather than reject: a caller asking for more than the cap gets the
    // cap, never the 39 MB full catalog.
    let entries = match limit.map(|l| l.min(MAX_BROWSE_LIMIT)) {
        Some(limit) => filtered.into_iter().skip(offset).take(limit).collect(),
        None => filtered.into_iter().skip(offset).collect(),
    };
    CatalogPage { entries, total }
}

/// Case-insensitive substring match over the fields the registry has always
/// searched: name, description, tags, category, author. `q` must already be
/// lowercased. Shared by [`search_catalog`] and [`page_catalog`] so the paged
/// browse and the legacy search cannot drift apart.
fn entry_matches_query(entry: &CatalogEntry, q: &str) -> bool {
    entry.name.to_lowercase().contains(q)
        || entry.description.to_lowercase().contains(q)
        || entry.tags.iter().any(|t| t.to_lowercase().contains(q))
        || entry.category.to_lowercase().contains(q)
        || entry
            .author
            .as_deref()
            .map(|a| a.to_lowercase().contains(q))
            .unwrap_or(false)
}

/// Search the catalog by query string.
pub async fn search_catalog(
    query: &str,
    source_filter: Option<&str>,
    category_filter: Option<&str>,
) -> Result<Vec<CatalogEntry>, String> {
    tracing::debug!(
        query = %query,
        source_filter = ?source_filter,
        category_filter = ?category_filter,
        "[skill_registry] search_catalog"
    );
    // Search/filter must reflect the current catalog — never serve stale.
    let catalog = browse_catalog_fresh().await?;
    let q = query.to_lowercase();

    let filtered: Vec<CatalogEntry> = catalog
        .into_iter()
        .filter(|entry| {
            if let Some(src) = source_filter {
                if !entry.source.eq_ignore_ascii_case(src) {
                    return false;
                }
            }
            if let Some(cat) = category_filter {
                if !entry.category.eq_ignore_ascii_case(cat) {
                    return false;
                }
            }
            if q.is_empty() {
                return true;
            }
            entry_matches_query(entry, &q)
        })
        .collect();

    tracing::debug!(
        result_count = filtered.len(),
        "[skill_registry] search complete"
    );
    Ok(filtered)
}

/// Return the distinct set of upstream sources present in the catalog.
pub async fn list_sources() -> Result<Vec<String>, String> {
    let catalog = browse_catalog(false).await?;
    let mut sources: Vec<String> = catalog
        .iter()
        .map(|e| e.source.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    sources.sort();
    Ok(sources)
}

/// Return the distinct set of categories present in the catalog.
pub async fn list_categories() -> Result<Vec<String>, String> {
    let catalog = browse_catalog(false).await?;
    let mut categories: Vec<String> = catalog
        .iter()
        .map(|e| e.category.clone())
        .filter(|c| !c.is_empty())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    categories.sort();
    Ok(categories)
}

/// Install a skill from the catalog by its entry id.
pub async fn install_from_catalog(
    workspace_dir: &std::path::Path,
    entry: &CatalogEntry,
) -> Result<crate::openhuman::skills::ops_install::InstallWorkflowFromUrlOutcome, String> {
    tracing::info!(
        entry_id = %entry.id,
        source = %entry.source,
        download_url = %entry.download_url,
        "[skill_registry] installing from catalog"
    );

    if entry.download_url.trim().is_empty() {
        let where_to_find = entry
            .source_url
            .as_deref()
            .map(|u| format!(" View it at {u}."))
            .unwrap_or_default();
        return Err(format!(
            "'{}' is hosted on {} and has no direct SKILL.md download, so it can't be installed automatically yet.{}",
            entry.name, entry.source, where_to_find
        ));
    }

    let params = crate::openhuman::skills::ops_install::InstallWorkflowFromUrlParams {
        url: entry.download_url.clone(),
        timeout_secs: Some(60),
    };

    crate::openhuman::skills::ops_install::install_workflow_from_url(workspace_dir, params).await
}

pub(crate) fn parse_hermes_entry(item: &serde_json::Value) -> Option<CatalogEntry> {
    let name = item.get("name").and_then(|v| v.as_str())?.to_string();

    let description = item
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let source = item
        .get("source")
        .and_then(|v| v.as_str())
        .unwrap_or("hermes")
        .to_string();

    let category = item
        .get("category")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let author = item
        .get("author")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let version = item
        .get("version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let license = item
        .get("license")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let tags = item
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let platforms = item
        .get("platforms")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let commands = item
        .get("commands")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let env_vars = item
        .get("envVars")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let docs_path = item
        .get("docsPath")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let source_url = item
        .get("sourceUrl")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let download_url = derive_download_url(
        &source,
        &category,
        &name,
        docs_path.as_deref(),
        source_url.as_deref(),
    );

    Some(CatalogEntry {
        id: name.clone(),
        name,
        description,
        source,
        category,
        author,
        version,
        tags,
        platforms,
        download_url,
        source_url,
        docs_path,
        commands,
        env_vars,
        license,
    })
}

/// Resolve a fetchable `SKILL.md` URL for a catalog entry.
///
/// Precedence:
/// 1. `OPENHUMAN_SKILL_REGISTRY_DOWNLOAD_BASE_URL` test override.
/// 2. `docsPath` — Hermes' own bundled / optional skills, which live in the
///    `NousResearch/hermes-agent` repo under `skills/` / `optional-skills/`.
/// 3. `sourceUrl` — community skills (ClawHub / LobeHub / skills.sh / browse.sh
///    / NVIDIA). When it points at a GitHub blob/tree it is rewritten to the
///    `raw.githubusercontent.com` `SKILL.md`; non-GitHub portals have no raw
///    download.
///
/// Returns an empty string when no direct download can be derived (portal-only
/// community skills). [`install_from_catalog`] turns that into an actionable
/// error rather than fetching a guaranteed-404 URL. Previously every community
/// skill was force-templated onto a `NousResearch/hermes-agent` path it never
/// lived at, so virtually all community installs 404'd — issue #3741.
fn derive_download_url(
    _source: &str,
    _category: &str,
    name: &str,
    docs_path: Option<&str>,
    source_url: Option<&str>,
) -> String {
    if let Ok(base) = std::env::var(DOWNLOAD_BASE_URL_ENV) {
        let base = base.trim().trim_end_matches('/');
        if !base.is_empty() {
            return format!("{base}/{name}/SKILL.md");
        }
    }
    if let Some(url) = docs_path.and_then(download_url_from_docs_path) {
        return url;
    }
    if let Some(url) = source_url.and_then(download_url_from_source_url) {
        return url;
    }
    // No resolvable direct download (portal-only community skill).
    String::new()
}

/// Rewrite a GitHub `sourceUrl` (blob or tree view) into the raw
/// `SKILL.md` download URL. Returns `None` for non-GitHub hosts (portal pages
/// that serve HTML, not raw markdown).
///
/// - blob: `…/github.com/{owner}/{repo}/blob/{branch}/{path}` →
///   `…/raw.githubusercontent.com/{owner}/{repo}/{branch}/{path}`
/// - tree (directory): same rewrite, then append `/SKILL.md`.
fn download_url_from_source_url(source_url: &str) -> Option<String> {
    let rest = source_url
        .strip_prefix("https://github.com/")
        .or_else(|| source_url.strip_prefix("http://github.com/"))?;

    // {owner}/{repo}/{blob|tree}/{branch}/{path...}
    let parts: Vec<&str> = rest.splitn(5, '/').collect();
    if parts.len() < 5 {
        return None;
    }
    let (owner, repo, kind, branch, path) = (parts[0], parts[1], parts[2], parts[3], parts[4]);
    if owner.is_empty() || repo.is_empty() || branch.is_empty() || path.is_empty() {
        return None;
    }

    let path = path.trim_end_matches('/');
    let raw = format!("https://raw.githubusercontent.com/{owner}/{repo}/{branch}/{path}");
    match kind {
        // blob points directly at a file; only append SKILL.md if it isn't one.
        "blob" => {
            if raw.ends_with("/SKILL.md") || raw.ends_with(".md") {
                Some(raw)
            } else {
                Some(format!("{raw}/SKILL.md"))
            }
        }
        // tree points at a directory — the skill's SKILL.md lives inside it.
        "tree" => Some(format!("{raw}/SKILL.md")),
        _ => None,
    }
}

fn download_url_from_docs_path(docs_path: &str) -> Option<String> {
    let parts: Vec<&str> = docs_path.split('/').collect();
    if parts.len() != 3 {
        return None;
    }
    let root = match parts[0] {
        "bundled" => "skills",
        "optional" => "optional-skills",
        _ => return None,
    };
    let category = parts[1];
    let prefixed_slug = parts[2];
    let skill = prefixed_slug
        .strip_prefix(&format!("{category}-"))
        .unwrap_or(prefixed_slug);
    Some(format!(
        "https://raw.githubusercontent.com/NousResearch/hermes-agent/main/{root}/{category}/{skill}/SKILL.md"
    ))
}

#[cfg(test)]
#[path = "ops_tests.rs"]
mod tests;
