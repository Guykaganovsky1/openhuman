//! Wire-format types for `openhuman.skill_registry_*` RPC methods.

use serde::{Deserialize, Serialize};

use crate::core::ControllerSchema;
use crate::openhuman::skills::catalog::types::CatalogEntry;
use crate::openhuman::skills::ops_types::WorkflowScope;

// ── Params ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
pub(super) struct BrowseParams {
    #[serde(default)]
    pub(super) force_refresh: bool,
    /// Case-insensitive substring filter over name/description/tags/category/author.
    #[serde(default)]
    pub(super) query: Option<String>,
    /// Restrict to these upstream sources (case-insensitive). Empty list = no filter.
    #[serde(default)]
    pub(super) sources: Option<Vec<String>>,
    #[serde(default)]
    pub(super) offset: Option<usize>,
    /// Page size, clamped to `ops::MAX_BROWSE_LIMIT`. Absent = the whole
    /// (filtered) catalog, which is the pre-paging behaviour.
    #[serde(default)]
    pub(super) limit: Option<usize>,
}

impl BrowseParams {
    /// True when the caller asked for a page/filter rather than the legacy
    /// "give me everything" browse. Decides whether `total` is emitted.
    pub(super) fn is_paged(&self) -> bool {
        self.query.is_some()
            || self.sources.is_some()
            || self.offset.is_some()
            || self.limit.is_some()
    }
}

#[derive(Debug, Deserialize, Default)]
pub(super) struct SearchParams {
    #[serde(default)]
    pub(super) query: String,
    #[serde(default)]
    pub(super) source: Option<String>,
    #[serde(default)]
    pub(super) category: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct InstallParams {
    pub(super) entry_id: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct UninstallParams {
    pub(super) name: String,
}

// ── Results ─────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub(super) struct BrowseResult {
    pub(super) entries: Vec<CatalogEntry>,
    /// Size of the filtered set this page was cut from. Omitted entirely for a
    /// legacy (no query/sources/offset/limit) browse so that payload stays
    /// byte-identical to what pre-paging callers already parse.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) total: Option<usize>,
}

#[derive(Debug, Serialize)]
pub(super) struct SearchResult {
    pub(super) entries: Vec<CatalogEntry>,
}

#[derive(Debug, Serialize)]
pub(super) struct SourcesResult {
    pub(super) sources: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct CategoriesResult {
    pub(super) categories: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct InstallResult {
    pub(super) url: String,
    pub(super) stdout: String,
    pub(super) stderr: String,
    pub(super) new_skills: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct UninstallResult {
    pub(super) name: String,
    pub(super) removed_path: String,
    pub(super) scope: WorkflowScope,
}

#[derive(Debug, Serialize)]
pub(super) struct SchemasResult {
    pub(super) schemas: Vec<ControllerSchema>,
}
