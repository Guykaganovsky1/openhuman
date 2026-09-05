//! Workflow discovery: scanning root directories, scope resolution, and
//! collision handling.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::ops_parse::{load_from_legacy_manifest, load_from_workflow_md};
use super::ops_types::{Workflow, WorkflowScope, SKILL_JSON, SKILL_MD, TRUST_MARKER, WORKFLOW_MD};

const EXCLUDED_SKILL_DIRS: &[&str] = &[
    ".git",
    ".github",
    ".hub",
    ".archive",
    ".venv",
    "venv",
    "node_modules",
    "site-packages",
    "__pycache__",
    ".tox",
    ".nox",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
];

/// Initialize the legacy skills directory in the specified workspace.
///
/// Creates `<workspace>/skills/` and a placeholder `README.md` so the folder
/// is visible to the user. New-style skills should live under
/// `<workspace>/.openhuman/skills/` instead, but this directory is kept for
/// backward compatibility.
pub fn init_workflows_dir(workspace_dir: &Path) -> Result<(), String> {
    let skills_dir = workspace_dir.join("skills");
    std::fs::create_dir_all(&skills_dir).map_err(|e| {
        format!(
            "failed to create skills directory {}: {e}",
            skills_dir.display()
        )
    })?;

    let readme_path = skills_dir.join("README.md");
    if !readme_path.exists() {
        let content = "# Skills\n\nPut one skill per directory under this folder.\n";
        std::fs::write(&readme_path, content)
            .map_err(|e| format!("failed to write {}: {e}", readme_path.display()))?;
    }

    Ok(())
}

/// Backwards-compatible shim for callers that only have a workspace path.
///
/// Delegates to [`discover_workflows`] with the current user's home directory
/// so user-scope skills (`~/.openhuman/skills/`, `~/.agents/skills/`) are
/// surfaced for existing production callers (`agent::harness::session::builder`,
/// `channels::runtime::startup`). Previously this shim passed `None` for the
/// home directory, which silently dropped user-installed skills from the
/// main runtime path.
///
/// Project-scope (workspace) skills still take precedence over user-scope
/// on name collisions.
pub fn load_workflow_metadata(workspace_dir: &Path) -> Vec<Workflow> {
    let trusted = is_workspace_trusted(workspace_dir);
    let home = dirs::home_dir();
    discover_workflows_inner(home.as_deref(), Some(workspace_dir), None, trusted)
}

/// Like [`load_workflow_metadata`], but additionally scans a profile-local
/// skills root (`<workspace>/personalities/<id>/skills/`) when one is supplied.
///
/// Callers pass the active profile's root (resolved via
/// `profiles::profile_skills_root`) so the returned catalog carries that
/// profile's private skills. `None` reproduces [`load_workflow_metadata`]
/// byte-for-byte, so the profile-less session and every other profile are
/// unaffected. Profile-local skills win same-name collisions against global
/// scopes (see [`WorkflowScope::Profile`]).
pub fn load_workflow_metadata_for_profile(
    workspace_dir: &Path,
    profile_skills_root: Option<&Path>,
) -> Vec<Workflow> {
    let trusted = is_workspace_trusted(workspace_dir);
    let home = dirs::home_dir();
    discover_workflows_inner(
        home.as_deref(),
        Some(workspace_dir),
        profile_skills_root,
        trusted,
    )
}

/// Discover skills from every supported location.
///
/// * `home_dir` — user home (typically `dirs::home_dir()`), scanned for
///   `~/.openhuman/skills/` and `~/.agents/skills/`.
/// * `workspace_dir` — current workspace, scanned for project-scope paths.
/// * `trusted` — whether the caller has verified the project trust marker.
///   Project-scope skills are silently skipped when `false`.
///
/// On name collisions, project-scope wins over user-scope and a warning is
/// attached to the retained skill.
pub fn discover_workflows(
    home_dir: Option<&Path>,
    workspace_dir: Option<&Path>,
    trusted: bool,
) -> Vec<Workflow> {
    discover_workflows_inner(home_dir, workspace_dir, None, trusted)
}

/// Discover skills including a profile-local root, for a turn running under a
/// specific agent profile.
///
/// `profile_skills_root` is `<workspace>/personalities/<id>/skills/` (resolved
/// via `profiles::profile_skills_root`, which validates the id). It is scanned
/// unconditionally — no trust marker is required, since the directory is
/// core-managed under `workspace_dir` — and its bundles win same-name collisions
/// against every global scope for this profile. `None` is identical to
/// [`discover_workflows`], so other profiles and the default session never see
/// these skills.
pub fn discover_workflows_with_profile(
    home_dir: Option<&Path>,
    workspace_dir: Option<&Path>,
    profile_skills_root: Option<&Path>,
    trusted: bool,
) -> Vec<Workflow> {
    discover_workflows_inner(home_dir, workspace_dir, profile_skills_root, trusted)
}

/// Whether the workspace has opted into loading project-scope skills.
///
/// Looks for `<workspace>/.openhuman/trust`. The marker file's contents are
/// ignored — presence is sufficient.
pub fn is_workspace_trusted(workspace_dir: &Path) -> bool {
    workspace_dir.join(".openhuman").join(TRUST_MARKER).exists()
}

/// Which on-disk root category a bundle was discovered under.
///
/// `Workflow` roots (`.openhuman/workflows/`) hold task *automations* authored
/// via "New workflow". `Skill` roots (`.openhuman/skills/`, `.agents/skills/`,
/// and the legacy `<workspace>/skills/`) hold capability *skills*. Both are the
/// same on-disk primitive (SKILL.md / WORKFLOW.md bundles) and the agent
/// harness loads both — but the Automations UI lists only `Workflow`-root
/// bundles (see [`discover_automations`]) so capability skills don't masquerade
/// as task templates.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RootKind {
    Skill,
    Workflow,
}

const ALL_ROOT_KINDS: &[RootKind] = &[RootKind::Skill, RootKind::Workflow];
const WORKFLOW_ROOT_KINDS: &[RootKind] = &[RootKind::Workflow];

pub(crate) fn discover_workflows_inner(
    home_dir: Option<&Path>,
    workspace_dir: Option<&Path>,
    profile_skills_root: Option<&Path>,
    trusted: bool,
) -> Vec<Workflow> {
    discover_filtered(
        home_dir,
        workspace_dir,
        profile_skills_root,
        trusted,
        ALL_ROOT_KINDS,
    )
}

/// Discover only *automation* bundles — those under the `workflows/` roots —
/// for the Automations UI list (`openhuman.skills_list`).
///
/// Capability skills (under the `skills/` / `.agents/skills/` / legacy
/// `<workspace>/skills/` roots) are deliberately excluded so they don't show up
/// as task templates. They remain fully available to the agent harness and the
/// run/describe paths via [`discover_workflows`] / [`load_workflow_metadata`].
///
/// Note: bundles authored *before* the skills→workflows rename live under the
/// `skills/` roots and will therefore not appear in this automations-only view;
/// new automations created via "New workflow" land in `~/.openhuman/workflows/`.
pub fn discover_automations(
    home_dir: Option<&Path>,
    workspace_dir: Option<&Path>,
    trusted: bool,
) -> Vec<Workflow> {
    tracing::debug!(
        trusted,
        has_home = home_dir.is_some(),
        has_workspace = workspace_dir.is_some(),
        "[workflows] discover:automations:enter"
    );
    discover_filtered(home_dir, workspace_dir, None, trusted, WORKFLOW_ROOT_KINDS)
}

/// Shared discovery core. `kinds` selects which root categories to scan,
/// letting the full surface ([`discover_workflows_inner`]) and the
/// automations-only list ([`discover_automations`]) share collision handling.
fn discover_filtered(
    home_dir: Option<&Path>,
    workspace_dir: Option<&Path>,
    profile_skills_root: Option<&Path>,
    trusted: bool,
    kinds: &[RootKind],
) -> Vec<Workflow> {
    tracing::debug!(
        trusted,
        has_home = home_dir.is_some(),
        has_workspace = workspace_dir.is_some(),
        has_profile_root = profile_skills_root.is_some(),
        include_skills = kinds.contains(&RootKind::Skill),
        include_workflows = kinds.contains(&RootKind::Workflow),
        "[workflows] discover:enter"
    );
    // Scan order matters for collision resolution: the last scope to register
    // a name wins, so we scan user first, then project, then legacy.
    let mut by_name: HashMap<String, Workflow> = HashMap::new();

    if let Some(home) = home_dir {
        for (root, kind) in user_roots(home) {
            if kinds.contains(&kind) {
                tracing::trace!(
                    root = %root.display(),
                    ?kind,
                    scope = ?WorkflowScope::User,
                    "[workflows] discover:branch:user"
                );
                absorb(&mut by_name, scan_root(&root, WorkflowScope::User));
            }
        }
    }

    if let Some(ws) = workspace_dir {
        if trusted {
            for (root, kind) in project_roots(ws) {
                if kinds.contains(&kind) {
                    tracing::trace!(
                        root = %root.display(),
                        ?kind,
                        scope = ?WorkflowScope::Project,
                        "[workflows] discover:branch:project"
                    );
                    absorb(&mut by_name, scan_root(&root, WorkflowScope::Project));
                }
            }
        }
        // Legacy `<workspace>/skills/` is a skill root: scanned for the full
        // surface (back-compat, no trust marker required) but excluded from the
        // automations-only view. Flagged with `legacy = true` so the UI can
        // nudge migration.
        if kinds.contains(&RootKind::Skill) {
            let legacy_root = ws.join("skills");
            tracing::trace!(
                root = %legacy_root.display(),
                scope = ?WorkflowScope::Legacy,
                "[workflows] discover:branch:legacy"
            );
            absorb(&mut by_name, scan_root(&legacy_root, WorkflowScope::Legacy));
        }
    }

    // Profile-local skills (`<workspace>/personalities/<id>/skills/`) are a skill
    // root scoped to the *active* profile: scanned last and at the highest
    // precedence so a profile-local bundle wins any same-name collision against
    // the global scopes for its owner (see [`precedence`]). Excluded from the
    // automations-only view for the same reason as the legacy skill root. No
    // trust marker is consulted — the directory is core-managed under
    // `workspace_dir`, seeded by `ensure_profile_home`.
    if let Some(profile_root) = profile_skills_root {
        if kinds.contains(&RootKind::Skill) {
            tracing::debug!(
                root = %profile_root.display(),
                scope = ?WorkflowScope::Profile,
                "[profiles] discover:branch:profile-local skills"
            );
            let before = by_name.len();
            absorb(
                &mut by_name,
                scan_root(profile_root, WorkflowScope::Profile),
            );
            tracing::debug!(
                names_before = before,
                names_after = by_name.len(),
                "[profiles] profile-local skills absorbed (profile scope wins same-name collisions)"
            );
        }
    }

    let mut out: Vec<Workflow> = by_name.into_values().collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    tracing::debug!(discovered_count = out.len(), "[workflows] discover:exit");
    out
}

/// Env var naming extra user-scope skill roots, `:`-separated (`;` on Windows),
/// each an absolute directory holding one bundle per subdirectory.
///
/// It exists because a user's real skill library is often neither of the two
/// built-in roots: an agent host (Claude Code, OpenClaw, Hermes) keeps the
/// bundles in its own tree and links them into `~/.claude/skills`, and
/// discovery deliberately refuses symlinked bundle directories
/// ([`scan_root_inner`]), so pointing at the link farm loads nothing. Point
/// this at the directory holding the *real* folders instead.
///
/// Roots are scanned in order, before the built-in ones, at `User` scope — so
/// a same-named bundle in `~/.openhuman/skills` still wins, and a bundle
/// present in two extra roots is de-duplicated by [`absorb`] like any other
/// collision rather than listed twice.
pub(crate) const EXTRA_SKILL_ROOTS_ENV: &str = "OPENHUMAN_SKILL_ROOTS";

/// Parse [`EXTRA_SKILL_ROOTS_ENV`] into scan roots.
///
/// Relative and empty entries are dropped: this list is consulted for every
/// session, and a relative root would resolve against whatever cwd the process
/// happens to have, which is not a property a skill catalog should depend on.
fn extra_user_roots() -> Vec<(PathBuf, RootKind)> {
    let Ok(raw) = std::env::var(EXTRA_SKILL_ROOTS_ENV) else {
        return Vec::new();
    };
    parse_extra_roots(&raw)
}

/// Pure half of [`extra_user_roots`], so the parsing rules are testable without
/// mutating process-global environment state from a parallel test run.
pub(crate) fn parse_extra_roots(raw: &str) -> Vec<(PathBuf, RootKind)> {
    let separator = if cfg!(windows) { ';' } else { ':' };
    let roots: Vec<(PathBuf, RootKind)> = raw
        .split(separator)
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(PathBuf::from)
        .filter(|path| {
            if path.is_absolute() {
                true
            } else {
                tracing::warn!(
                    root = %path.display(),
                    env = EXTRA_SKILL_ROOTS_ENV,
                    "[workflows] ignoring relative extra skill root"
                );
                false
            }
        })
        .map(|path| (path, RootKind::Skill))
        .collect();
    tracing::debug!(
        count = roots.len(),
        env = EXTRA_SKILL_ROOTS_ENV,
        "[workflows] extra user skill roots configured"
    );
    roots
}

fn user_roots(home: &Path) -> Vec<(PathBuf, RootKind)> {
    // `workflows/` is the current layout (create writes here); the `skills/`
    // roots are still scanned for back-compat with installs created before the
    // skills→workflows rename. Order matters: `workflows/` is scanned last so a
    // same-named entry there wins over a legacy `skills/` one, and the
    // env-configured roots are scanned first so a bundle the user also keeps
    // under `~/.openhuman/skills` wins over the external copy.
    let mut roots = extra_user_roots();
    roots.extend([
        (home.join(".claude").join("skills"), RootKind::Skill),
        (home.join(".openhuman").join("skills"), RootKind::Skill),
        (home.join(".agents").join("skills"), RootKind::Skill),
        (
            home.join(".openhuman").join("workflows"),
            RootKind::Workflow,
        ),
    ]);
    roots
}

fn project_roots(workspace: &Path) -> Vec<(PathBuf, RootKind)> {
    vec![
        (workspace.join(".openhuman").join("skills"), RootKind::Skill),
        (workspace.join(".agents").join("skills"), RootKind::Skill),
        (
            workspace.join(".openhuman").join("workflows"),
            RootKind::Workflow,
        ),
    ]
}

fn absorb(by_name: &mut HashMap<String, Workflow>, incoming: Vec<Workflow>) {
    for mut skill in incoming {
        let key = skill.name.clone();
        // A workflow's runnable identity is `dir_name`, while `name` is only
        // display metadata. Collapse on either so a profile-local `foo/` also
        // shadows a global `foo/` whose frontmatter happens to use a different
        // display name. Otherwise registry lookup by slug could nondeterministically
        // select the global copy.
        let collision_keys: Vec<String> = by_name
            .iter()
            .filter(|(existing_name, existing)| {
                existing_name.as_str() == key || existing.dir_name == skill.dir_name
            })
            .map(|(existing_name, _)| existing_name.clone())
            .collect();

        if let Some((_, highest_name, highest_scope)) = collision_keys
            .iter()
            .filter_map(|collision_key| by_name.get(collision_key))
            .map(|existing| {
                (
                    precedence(existing.scope),
                    existing.name.clone(),
                    existing.scope,
                )
            })
            .max_by_key(|(rank, _, _)| *rank)
        {
            if precedence(skill.scope) < precedence(highest_scope) {
                if let Some(kept) = by_name.get_mut(&highest_name) {
                    kept.warnings.push(format!(
                        "workflow id '{}' or name '{}' also declared in {:?} scope at {} (ignored)",
                        skill.dir_name,
                        skill.name,
                        skill.scope,
                        skill
                            .location
                            .as_deref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "<unknown>".to_string())
                    ));
                }
                continue;
            }
        }

        for collision_key in collision_keys {
            if let Some(loser) = by_name.remove(&collision_key) {
                skill.warnings.push(format!(
                    "shadowed {:?}-scope skill '{}' (workflow id '{}') at {}",
                    loser.scope,
                    loser.name,
                    loser.dir_name,
                    loser
                        .location
                        .as_deref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "<unknown>".to_string())
                ));
            }
        }
        by_name.insert(key, skill);
    }
}

fn precedence(scope: WorkflowScope) -> u8 {
    match scope {
        WorkflowScope::Legacy => 0,
        WorkflowScope::User => 1,
        WorkflowScope::Project => 2,
        // Profile-local skills win against every global scope for their owner.
        WorkflowScope::Profile => 3,
    }
}

pub(super) fn scan_root(root: &Path, scope: WorkflowScope) -> Vec<Workflow> {
    let mut out = Vec::new();
    scan_root_inner(root, scope, &mut out);
    out.sort_by(|a, b| a.dir_name.cmp(&b.dir_name));
    out
}

fn scan_root_inner(root: &Path, scope: WorkflowScope, out: &mut Vec<Workflow>) {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    // `read_dir` order is unspecified. When two sibling directories declare
    // the same logical `frontmatter.name` (which can differ from the folder
    // name), cross-scope/same-scope deduplication downstream would otherwise
    // pick a non-deterministic winner across runs. Sort by on-disk directory
    // name for a stable, reproducible order.
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        // Use `file_type()` rather than `path.is_dir()` so a symlinked
        // child cannot be loaded as a skill. `is_dir()` dereferences
        // symlinks, which would re-open out-of-tree loading even though
        // `walk_files` already rejects symlinks deeper in the resource
        // walker. Skip both symlinks and non-directory entries here; if
        // the `file_type()` call itself fails (rare — transient I/O),
        // treat it as "not safe to traverse" and skip.
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() || !file_type.is_dir() {
            continue;
        }
        let path = entry.path();
        let dir_name = entry.file_name().to_string_lossy().to_string();
        if dir_name.starts_with('.') || EXCLUDED_SKILL_DIRS.contains(&dir_name.as_str()) {
            continue;
        }
        if let Some(skill) = load_skill_dir(&path, &dir_name, scope) {
            out.push(skill);
            continue;
        }
        scan_root_inner(&path, scope, out);
    }
}

fn load_skill_dir(dir: &Path, dir_name: &str, scope: WorkflowScope) -> Option<Workflow> {
    // WORKFLOW.md is the current filename; SKILL.md is read for back-compat
    // with workflows authored before the rename.
    let workflow_md = dir.join(WORKFLOW_MD);
    let legacy_md = dir.join(SKILL_MD);
    let legacy_manifest = dir.join(SKILL_JSON);

    // `exists()` follows symlinks, so a manifest could point at an arbitrary
    // file outside the bundle and discovery would ingest its contents into the
    // catalog/prompt flow. Since the legacy `skills/` roots are scanned without
    // a trust marker, require a real (non-symlink) regular file before loading.
    let is_safe_manifest = |path: &Path| {
        matches!(
            std::fs::symlink_metadata(path),
            Ok(meta) if meta.is_file() && !meta.file_type().is_symlink()
        )
    };

    if is_safe_manifest(&workflow_md) {
        return Some(load_from_workflow_md(&workflow_md, dir, dir_name, scope));
    }
    if is_safe_manifest(&legacy_md) {
        return Some(load_from_workflow_md(&legacy_md, dir, dir_name, scope));
    }
    if is_safe_manifest(&legacy_manifest) {
        return Some(load_from_legacy_manifest(
            &legacy_manifest,
            dir,
            dir_name,
            scope,
        ));
    }
    None
}

#[cfg(test)]
#[path = "ops_discover_include_skills_tests_tests.rs"]
mod include_skills_tests;

#[cfg(test)]
#[path = "ops_discover_profile_scope_tests_tests.rs"]
mod profile_scope_tests;
