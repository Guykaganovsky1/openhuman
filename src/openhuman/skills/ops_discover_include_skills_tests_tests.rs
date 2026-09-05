use super::*;

/// Write a minimal `<file>`-named bundle under `root/slug/`.
fn seed_bundle(root: &Path, slug: &str, file: &str) {
    let dir = root.join(slug);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(file),
        format!("---\nname: {slug}\ndescription: {slug} desc\n---\n\n{slug} body\n"),
    )
    .unwrap();
}

/// `discover_automations` lists only `workflows/`-root automations, while
/// `discover_workflows` additionally surfaces `skills/`-root installs. This
/// is exactly the branch `handle_skills_list` selects on `include_skills`
/// so the Skills Explorer's Installed tab can show registry installs (#3954).
#[test]
fn automations_excludes_skill_roots_but_full_discover_includes_them() {
    let home = tempfile::TempDir::new().unwrap();
    let home_path = home.path();
    // A registry-style install lands under `~/.openhuman/skills/`.
    seed_bundle(
        &home_path.join(".openhuman").join("skills"),
        "installed-skill",
        "SKILL.md",
    );
    // A "New workflow" automation lands under `~/.openhuman/workflows/`.
    seed_bundle(
        &home_path.join(".openhuman").join("workflows"),
        "my-automation",
        "WORKFLOW.md",
    );

    // Automations-only view (the default `skills_list` path) hides the skill.
    let automations = discover_automations(Some(home_path), None, false);
    let auto_names: Vec<&str> = automations.iter().map(|w| w.name.as_str()).collect();
    assert_eq!(
        auto_names,
        vec!["my-automation"],
        "discover_automations must exclude `skills/`-root installs"
    );

    // Full view (`include_skills=true`) surfaces both.
    let full = discover_workflows(Some(home_path), None, false);
    let mut full_names: Vec<&str> = full.iter().map(|w| w.name.as_str()).collect();
    full_names.sort_unstable();
    assert_eq!(
        full_names,
        vec!["installed-skill", "my-automation"],
        "discover_workflows must include `skills/`-root installs"
    );
}

/// `~/.claude/skills` is a first-class user-scope root: an agent host that keeps
/// its bundles there should not need a copy under `~/.openhuman/skills`.
#[test]
fn claude_skills_root_is_discovered() {
    let home = tempfile::TempDir::new().unwrap();
    seed_bundle(
        &home.path().join(".claude").join("skills"),
        "claude-only-skill",
        "SKILL.md",
    );

    let found = discover_workflows(Some(home.path()), None, false);
    let names: Vec<&str> = found.iter().map(|w| w.name.as_str()).collect();
    assert_eq!(names, vec!["claude-only-skill"]);
}

/// A bundle present in two user-scope roots is listed once, and the built-in
/// `~/.openhuman/skills` copy is the one kept — the extra roots are scanned
/// first precisely so the user's own install wins the collision.
#[test]
fn a_bundle_in_two_user_roots_is_listed_once() {
    let home = tempfile::TempDir::new().unwrap();
    let claude_root = home.path().join(".claude").join("skills");
    let openhuman_root = home.path().join(".openhuman").join("skills");
    seed_bundle(&claude_root, "shared-skill", "SKILL.md");
    seed_bundle(&openhuman_root, "shared-skill", "SKILL.md");

    let found = discover_workflows(Some(home.path()), None, false);
    assert_eq!(
        found.len(),
        1,
        "the same bundle in two user roots must not be listed twice: {found:#?}"
    );
    assert_eq!(
        found[0].location.as_deref().and_then(|p| p.parent()),
        Some(openhuman_root.join("shared-skill").as_path()),
        "the built-in root must win the same-scope collision"
    );
}

/// The env-configured roots exist for hosts (Claude Code, OpenClaw, Hermes)
/// that link their bundles into `~/.claude/skills` rather than storing them
/// there — discovery refuses symlinked bundle dirs, so the real directory has
/// to be nameable. Relative and empty entries are dropped rather than resolved
/// against the process cwd.
#[test]
fn extra_roots_are_parsed_as_absolute_skill_roots() {
    let separator = if cfg!(windows) { ";" } else { ":" };
    let absolute = if cfg!(windows) {
        r"C:\skills\real"
    } else {
        "/skills/real"
    };
    let raw = format!("{absolute}{separator} {separator}relative/skills{separator}");

    let roots = parse_extra_roots(&raw);
    assert_eq!(
        roots,
        vec![(PathBuf::from(absolute), RootKind::Skill)],
        "only the absolute entry survives"
    );
    assert!(parse_extra_roots("").is_empty());
}
