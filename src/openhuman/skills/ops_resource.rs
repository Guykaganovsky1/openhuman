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
    // the check and the read: these skill roots are user-managed, so a process
    // with write access can replace a component with a symlink after the
    // `starts_with` test passes. So the path checks above are kept for their
    // error messages and early rejection, and the actual open does not trust
    // them: `open_under_root` walks the components one at a time from a held
    // root descriptor, no-follow at every step, and everything below reads the
    // resulting DESCRIPTOR. The bytes returned are then provably the object we
    // checked, not whatever the path resolves to a moment later.
    //
    // The walk starts from `skill_root`, NOT from `canonical_root`: the latter
    // is what `canonicalize` said a moment ago, and canonicalize follows
    // symlinks. A skill root replaced by a link to somewhere else between
    // discovery and here would make `canonical_root` the attacker's directory,
    // and a no-follow walk beneath it would faithfully read the wrong tree.
    // Opening `skill_root` with `O_NOFOLLOW` refuses that replacement instead
    // of resolving it. (Replacing an ANCESTOR of the root is still not chased
    // — see this function's note.)
    let file = open_under_root(&skill_root, relative_path)?;

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

/// Open the resource at `relative` beneath `root` without letting ANY path
/// component be swapped for a symlink after it was checked.
///
/// A no-follow open of the joined path is not enough: it constrains only the
/// final component, so an attacker able to write in the skill tree can replace
/// an *intermediate directory* between the `canonicalize` above and the open,
/// and the kernel resolves the new one. The fix is to never hand the kernel a
/// multi-component path at all — hold a descriptor for the root, then walk one
/// component at a time with `openat(..., O_NOFOLLOW)`, so each step is relative
/// to a directory we already have open. A directory swapped after we opened it
/// no longer participates in the resolution.
///
/// `relative` is already validated to be non-empty and to contain only
/// `Component::Normal` parts, which is what makes the walk below total.
///
/// The root itself is opened no-follow as well, so the skill directory cannot
/// be swapped for a symlink between `canonicalize` and this call. What remains
/// uncovered is an ANCESTOR of the skill root being replaced — and that is not
/// worth further machinery, because an attacker who can write there does not
/// need a race at all: they can drop a `SKILL.md` in the skills tree and be
/// discovered through the ordinary path. The race only buys something to an
/// attacker confined to the tree itself, which is exactly what these opens
/// close.
#[cfg(unix)]
pub(crate) fn open_under_root(root: &Path, relative: &Path) -> Result<std::fs::File, String> {
    use std::ffi::{CString, OsStr};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Component;

    fn openat_no_follow(
        dir: libc::c_int,
        name: &OsStr,
        flags: libc::c_int,
        display: &Path,
    ) -> Result<OwnedFd, String> {
        let c_name = CString::new(name.as_bytes())
            .map_err(|_| "resource path component contains an interior NUL".to_string())?;
        // SAFETY: `dir` is a live descriptor owned by the caller for the whole
        // call, and `c_name` is a NUL-terminated string that outlives it.
        let fd = unsafe { libc::openat(dir, c_name.as_ptr(), flags) };
        if fd < 0 {
            return Err(format!(
                "failed to open resource {}: {}",
                display.display(),
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: `openat` returned a fresh, owned descriptor.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    let components: Vec<&OsStr> = relative
        .components()
        .map(|c| match c {
            Component::Normal(name) => Ok(name),
            _ => Err("relative_path must be a plain relative path".to_string()),
        })
        .collect::<Result<_, _>>()?;
    let Some((last, parents)) = components.split_last() else {
        return Err("relative_path must name a file".to_string());
    };

    let root_c = CString::new(root.as_os_str().as_bytes())
        .map_err(|_| "skill root contains an interior NUL".to_string())?;
    // SAFETY: `root_c` is NUL-terminated and lives across the call.
    let root_fd = unsafe {
        libc::open(
            root_c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if root_fd < 0 {
        return Err(format!(
            "failed to open skill root {}: {}",
            root.display(),
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: `open` returned a fresh, owned descriptor.
    let mut dir = unsafe { OwnedFd::from_raw_fd(root_fd) };

    for name in parents {
        dir = openat_no_follow(
            dir.as_raw_fd(),
            name,
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            relative,
        )?;
    }

    let file = openat_no_follow(
        dir.as_raw_fd(),
        last,
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        relative,
    )?;
    Ok(std::fs::File::from(file))
}

/// Windows: open the leaf with the reparse-point flag, then verify the HANDLE.
///
/// Windows has no `openat`, so the walk the unix path uses is not available
/// and the open still takes a joined path — which on its own would leave the
/// intermediate-directory swap open. What closes it is checking afterwards
/// what the handle actually refers to: `GetFinalPathNameByHandleW` answers for
/// the opened object, not for a path that can be re-resolved, so if a
/// directory component was replaced mid-open the final path lands outside the
/// root and the read is refused. The reparse-point flag still keeps a
/// symlinked leaf from being followed in the first place.
///
/// Both sides of the comparison come from the same API, so both are in the
/// same normalised `\\?\` form and can be compared as prefixes.
#[cfg(windows)]
pub(crate) fn open_under_root(root: &Path, relative: &Path) -> Result<std::fs::File, String> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFinalPathNameByHandleW, FILE_NAME_NORMALIZED,
    };

    // FILE_FLAG_OPEN_REPARSE_POINT / FILE_FLAG_BACKUP_SEMANTICS
    const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const BACKUP_SEMANTICS: u32 = 0x0200_0000;

    fn final_path(handle: &impl AsRawHandle, what: &str) -> Result<Vec<u16>, String> {
        let raw = handle.as_raw_handle();
        // SAFETY: `raw` is a live handle owned by the caller for both calls.
        // The first passes a null buffer with length 0, which the API answers
        // with the size it needs (including the NUL); the second fills it.
        let needed = unsafe {
            GetFinalPathNameByHandleW(raw, std::ptr::null_mut(), 0, FILE_NAME_NORMALIZED)
        };
        if needed == 0 {
            return Err(format!(
                "failed to resolve {what}: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut buf = vec![0u16; needed as usize];
        // SAFETY: `buf` has `needed` elements, which is what the sizing call
        // above asked for.
        let written = unsafe {
            GetFinalPathNameByHandleW(raw, buf.as_mut_ptr(), needed, FILE_NAME_NORMALIZED)
        };
        if written == 0 || written >= needed {
            return Err(format!(
                "failed to resolve {what}: {}",
                std::io::Error::last_os_error()
            ));
        }
        buf.truncate(written as usize);
        Ok(buf)
    }

    let root_handle = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(BACKUP_SEMANTICS | OPEN_REPARSE_POINT)
        .open(root)
        .map_err(|e| format!("failed to open skill root {}: {e}", root.display()))?;
    let root_final = final_path(&root_handle, "skill root")?;

    let path = root.join(relative);
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(OPEN_REPARSE_POINT)
        .open(&path)
        .map_err(|e| format!("failed to open resource {}: {e}", path.display()))?;
    let file_final = final_path(&file, "resource")?;

    // The resource must sit strictly beneath the root: same prefix, and the
    // next character a separator, so `\\?\C:\skills\a` cannot admit
    // `\\?\C:\skills\attack`.
    let separator = u16::from(b'\\');
    let contained = file_final.len() > root_final.len()
        && file_final.starts_with(&root_final)
        && file_final[root_final.len()] == separator;
    if !contained {
        return Err(format!(
            "resource path escapes skill root: {}",
            path.display()
        ));
    }

    Ok(file)
}
