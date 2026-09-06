//! Skill resource reading: resolving a skill id to its on-disk root and
//! serving a bundled file from it, hardened against traversal and symlink
//! escape.

use std::io::Read;
use std::path::Path;

use super::ops_discover::{load_workflow_metadata_for_profile, scan_root};
use super::ops_types::{Workflow, WorkflowScope, MAX_WORKFLOW_RESOURCE_BYTES};

/// Read a bundled skill resource as UTF-8 text, hardened against directory
/// traversal, symlink escape, and oversized payloads.
///
/// `skill_id` identifies the skill by its discovered `name` or its on-disk
/// `dir_name` slug — the same identifiers surfaced in the UI summary. The
/// skill is resolved by running the standard
/// discovery pipeline (`dirs::home_dir()` + `workspace_dir`, honoring the
/// `.openhuman/trust` marker) and locating the matching entry; this keeps the
/// read scoped to legitimately installed skills and reuses all the symlink /
/// traversal hardening already baked into discovery.
///
/// `relative_path` is resolved relative to the skill's on-disk directory
/// (the parent of its `SKILL.md` / `skill.json`). All of the following are
/// rejected with an error:
///
/// * paths that canonicalize outside the skill root (traversal),
/// * paths whose final component or any intermediate component is a symlink
///   (link-follow escape),
/// * non-file targets (directories, sockets, fifos),
/// * files larger than [`MAX_WORKFLOW_RESOURCE_BYTES`],
/// * non-UTF-8 byte contents (binary files must be surfaced some other way —
///   no lossy replacement).
///
/// On success returns the file's contents as an owned `String`.
pub fn read_workflow_resource(
    workspace_dir: &Path,
    skill_id: &str,
    relative_path: &Path,
) -> Result<String, String> {
    read_workflow_resource_with_profile(workspace_dir, skill_id, relative_path, None)
}

/// The dir_name/name set of skills discovered under a profile-local skills root.
///
/// Used by the `describe_workflow` / `read_workflow_resource` / `run_workflow`
/// tools to treat a profile's private skills as implicitly allowed for their
/// owner (they bypass the `allowed_skills` allowlist, mirroring `list_workflows`).
/// Empty when no profile root is active, so the profile-less session and other
/// profiles are unaffected.
pub fn profile_local_skill_ids(
    profile_skills_root: Option<&Path>,
) -> std::collections::HashSet<String> {
    let Some(root) = profile_skills_root else {
        return std::collections::HashSet::new();
    };
    scan_root(root, WorkflowScope::Profile)
        .into_iter()
        .flat_map(|w| {
            let mut ids = vec![w.name];
            if !w.dir_name.is_empty() {
                ids.push(w.dir_name);
            }
            ids
        })
        .collect()
}

/// Like [`read_workflow_resource`], but resolves the skill against the active
/// profile's private skills root too (`<workspace>/personalities/<id>/skills/`)
/// when `profile_skills_root` is supplied. `None` is byte-identical to
/// [`read_workflow_resource`].
pub fn read_workflow_resource_with_profile(
    workspace_dir: &Path,
    skill_id: &str,
    relative_path: &Path,
    profile_skills_root: Option<&Path>,
) -> Result<String, String> {
    tracing::debug!(
        skill_id = %skill_id,
        relative_path = %relative_path.display(),
        workspace = %workspace_dir.display(),
        has_profile_root = profile_skills_root.is_some(),
        "[skills] read_workflow_resource: entry"
    );

    if skill_id.trim().is_empty() {
        return Err("skill_id must not be empty".to_string());
    }

    let relative_str = relative_path.to_string_lossy();
    if relative_str.trim().is_empty() {
        return Err("relative_path must not be empty".to_string());
    }
    if relative_path.is_absolute() {
        return Err("relative_path must be relative, not absolute".to_string());
    }
    // Reject any component that is `..`, is empty, starts with `.`, or is the
    // root. `..` is the obvious traversal vector; the others are defense in
    // depth against unusual path inputs (e.g. `./`, `//foo`, Windows `C:`).
    for component in relative_path.components() {
        use std::path::Component;
        match component {
            Component::Normal(_) => {}
            Component::ParentDir => {
                return Err("relative_path must not contain '..' components".to_string());
            }
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {
                return Err("relative_path must be a plain relative path".to_string());
            }
        }
    }

    // Resolve the skill by running the standard discovery pipeline. We reuse
    // `load_workflow_metadata_for_profile` (which honors both user and workspace
    // roots plus the trust marker, and the active profile's private root when
    // supplied) so the resource read is scoped to the exact same set of skills
    // the owner would already have seen listed.
    let skill = resolve_workflow_for_resource(
        load_workflow_metadata_for_profile(workspace_dir, profile_skills_root),
        skill_id,
    )?;
    let skill_root = skill
        .location
        .as_deref()
        .and_then(|p| p.parent())
        .ok_or_else(|| format!("skill '{skill_id}' has no on-disk location"))?
        .to_path_buf();

    // Canonicalize the root first. The root must itself be a real directory
    // on disk (not a symlink). Reject early if this fails.
    let canonical_root = std::fs::canonicalize(&skill_root).map_err(|e| {
        format!(
            "failed to canonicalize skill root {}: {e}",
            skill_root.display()
        )
    })?;

    let requested = canonical_root.join(relative_path);

    // Pre-check the immediate target with `symlink_metadata` so we catch
    // symlinked leaves before `canonicalize` silently follows them.
    let leaf_meta = std::fs::symlink_metadata(&requested)
        .map_err(|e| format!("failed to stat resource {}: {e}", requested.display()))?;
    if leaf_meta.file_type().is_symlink() {
        return Err("resource path is a symlink".to_string());
    }
    if !leaf_meta.is_file() {
        return Err("resource path is not a regular file".to_string());
    }

    // Canonicalize the full path and verify it stays within the skill root.
    // This catches a symlink reachable via an intermediate path component.
    let canonical_requested = std::fs::canonicalize(&requested).map_err(|e| {
        format!(
            "failed to canonicalize resource {}: {e}",
            requested.display()
        )
    })?;
    if !canonical_requested.starts_with(&canonical_root) {
        return Err(format!(
            "resource path escapes skill root: {}",
            canonical_requested.display()
        ));
    }

    // Everything above validates a PATH, and a path can be swapped between
    // the check and the read: these skill roots are user-managed, so a
    // process with write access can replace a component with a symlink after
    // the `starts_with` test passes. Open the leaf with no-follow semantics
    // and re-validate the OPEN DESCRIPTOR, then read through that same
    // descriptor — the bytes we return are then provably the object we
    // checked, not whatever the path resolves to a moment later.
    let file = open_no_follow(&canonical_requested)?;

    // Type and size come from the descriptor (fstat), not from the path.
    let meta = file
        .metadata()
        .map_err(|e| format!("failed to stat opened resource: {e}"))?;
    if !meta.is_file() {
        return Err("resource path is not a regular file".to_string());
    }
    let size = meta.len();
    if size > MAX_WORKFLOW_RESOURCE_BYTES {
        return Err(format!(
            "resource file is {size} bytes, exceeds limit of {MAX_WORKFLOW_RESOURCE_BYTES}"
        ));
    }

    // Bound the read independently of the reported size — a file that grows
    // between fstat and read must not be able to hand us more than the limit.
    let mut bytes = Vec::with_capacity(size as usize);
    file.take(MAX_WORKFLOW_RESOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| {
            format!(
                "failed to read resource {}: {e}",
                canonical_requested.display()
            )
        })?;
    if bytes.len() as u64 > MAX_WORKFLOW_RESOURCE_BYTES {
        return Err(format!(
            "resource file exceeds limit of {MAX_WORKFLOW_RESOURCE_BYTES} bytes"
        ));
    }

    let content = std::str::from_utf8(&bytes)
        .map_err(|e| format!("resource is not valid UTF-8 text: {e}"))?
        .to_string();

    tracing::debug!(
        skill_id = %skill_id,
        bytes = bytes.len(),
        "[skills] read_workflow_resource: success"
    );

    Ok(content)
}

fn resolve_workflow_for_resource(
    workflows: Vec<Workflow>,
    skill_id: &str,
) -> Result<Workflow, String> {
    let mut dir_match: Option<Workflow> = None;
    let mut name_match: Option<Workflow> = None;

    for workflow in workflows {
        if workflow.dir_name == skill_id {
            if dir_match.is_some() {
                return Err(format!(
                    "skill id '{skill_id}' is ambiguous across multiple skill directories"
                ));
            }
            dir_match = Some(workflow);
            continue;
        }

        if workflow.name == skill_id {
            if name_match.is_some() {
                return Err(format!(
                    "skill name '{skill_id}' is ambiguous; use the directory id"
                ));
            }
            name_match = Some(workflow);
        }
    }

    match (dir_match, name_match) {
        (Some(dir_skill), Some(name_skill)) => {
            if dir_skill.location == name_skill.location {
                Ok(dir_skill)
            } else {
                Err(format!(
                    "skill id '{skill_id}' matches both a directory id and a different skill name"
                ))
            }
        }
        (Some(skill), None) | (None, Some(skill)) => Ok(skill),
        (None, None) => Err(format!("skill '{skill_id}' not found")),
    }
}

/// Open `path` without following a final symlink component.
///
/// `File::open` follows symlinks, which is exactly the window the path checks
/// above cannot close: between `canonicalize` and the open, the leaf can be
/// replaced. `O_NOFOLLOW` (unix) and `FILE_FLAG_OPEN_REPARSE_POINT` (windows)
/// make that replacement an error rather than a silent redirect.
///
/// Windows returns a handle to the reparse point itself rather than failing,
/// so the caller's `metadata().is_file()` check is what rejects it there —
/// which is why that check reads the descriptor and not the path.
fn open_no_follow(path: &Path) -> Result<std::fs::File, String> {
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_OPEN_REPARSE_POINT
        opts.custom_flags(0x0020_0000);
    }

    opts.open(path)
        .map_err(|e| format!("failed to open resource {}: {e}", path.display()))
}
