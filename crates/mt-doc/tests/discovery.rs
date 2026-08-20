//! Skill discovery against real directory trees.
//!
//! These build their own trees in a temp dir rather than using the checked-in
//! fixtures, because what is under test is the *walking* — symlinks, duplicate
//! roots, skipped directories — which cannot be expressed as committed files
//! (a repo cannot portably carry a symlink or a `node_modules`).

use std::fs;
use std::path::Path;

use mt_doc::skill::{self, Discovery};

/// Write a minimal conformant skill at `dir`.
fn write_skill(dir: &Path, name: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: A test skill.\n---\n\n# {name}\n"),
    )
    .unwrap();
}

/// Create a directory symlink, or report that this platform/session cannot.
///
/// Windows needs Developer Mode or elevation for `symlink_dir`, so a test that
/// depends on one must skip rather than fail on an unprivileged machine.
fn try_symlink_dir(target: &Path, link: &Path) -> bool {
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(target, link).is_ok()
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (target, link);
        false
    }
}

#[test]
fn finds_skills_across_the_harness_conventions() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_skill(&root.join("skills/alpha"), "alpha");
    write_skill(&root.join(".claude/skills/beta"), "beta");
    write_skill(&root.join(".codebuddy/skills/gamma"), "gamma");
    // A harness whose directory is not `<dot-dir>/skills`.
    write_skill(&root.join(".posit/assistant/skills/delta"), "delta");

    let names: Vec<String> = skill::discover(root).into_iter().map(|s| s.name).collect();
    for expected in ["alpha", "beta", "gamma", "delta"] {
        assert!(
            names.contains(&expected.to_string()),
            "missing {expected} in {names:?}"
        );
    }
}

#[test]
fn category_folders_are_searched_but_deeper_ones_are_not() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // The documented three levels below a container.
    write_skill(&root.join("skills/category/sub/deep"), "deep");
    write_skill(&root.join("skills/way/too/deep/buried"), "buried");

    let names: Vec<String> = skill::discover(root).into_iter().map(|s| s.name).collect();
    assert!(
        names.contains(&"deep".to_string()),
        "depth 3 must be reached"
    );
    assert!(
        !names.contains(&"buried".to_string()),
        "depth 4 must not be, or a large tree becomes a full scan"
    );
}

#[test]
fn skipped_directories_are_never_descended() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_skill(&root.join("skills/real"), "real");
    write_skill(&root.join("skills/node_modules/vendored"), "vendored");
    write_skill(&root.join("skills/.git/hooky"), "hooky");
    write_skill(&root.join("skills/dist/built"), "built");

    let names: Vec<String> = skill::discover(root).into_iter().map(|s| s.name).collect();
    assert!(names.contains(&"real".to_string()));
    for skipped in ["vendored", "hooky", "built"] {
        assert!(
            !names.contains(&skipped.to_string()),
            "{skipped} must be skipped"
        );
    }
}

#[test]
fn a_skill_reachable_by_two_roots_is_listed_once_with_an_alias() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_skill(&root.join(".agents/skills/shared"), "shared");

    // `.claude/skills` is a link to `.agents/skills` — exactly the layout the
    // reference installer produces, and what makes naive discovery double-count.
    let link = root.join(".claude/skills");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    if !try_symlink_dir(&root.join(".agents/skills"), &link) {
        eprintln!("skipping: this session cannot create directory symlinks");
        return;
    }

    let skills = skill::discover(root);
    let shared: Vec<_> = skills.iter().filter(|s| s.name == "shared").collect();
    assert_eq!(shared.len(), 1, "the link and its target are one skill");
    assert_eq!(
        shared[0].aliases.len(),
        1,
        "the other path must be recorded, not discarded: {:?}",
        shared[0]
    );
    // `.agents/skills` outranks `.claude/skills`, so it is the one shown.
    assert!(shared[0].dir.starts_with(root.join(".agents")));
}

#[test]
fn a_symlink_cycle_terminates() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_skill(&root.join("skills/alpha"), "alpha");

    // A container that contains itself. Without a visited-set this recurses on
    // every branch until the depth cap, repeatedly.
    if !try_symlink_dir(&root.join("skills"), &root.join("skills/loop")) {
        eprintln!("skipping: this session cannot create directory symlinks");
        return;
    }

    let skills = skill::discover(root);
    assert_eq!(
        skills.iter().filter(|s| s.name == "alpha").count(),
        1,
        "a cycle must not multiply results"
    );
}

#[test]
fn internal_skills_are_hidden_unless_asked_for() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_skill(&root.join("skills/public"), "public");
    fs::create_dir_all(root.join("skills/private")).unwrap();
    fs::write(
        root.join("skills/private/SKILL.md"),
        "---\nname: private\ndescription: Hidden.\nmetadata:\n  internal: true\n---\n",
    )
    .unwrap();

    let default: Vec<String> = skill::discover(root).into_iter().map(|s| s.name).collect();
    assert!(default.contains(&"public".to_string()));
    assert!(
        !default.contains(&"private".to_string()),
        "internal skills are hidden by default"
    );

    let opted_in: Vec<String> = skill::discover_with(
        root,
        Discovery {
            global: false,
            include_internal: true,
        },
    )
    .into_iter()
    .map(|s| s.name)
    .collect();
    assert!(opted_in.contains(&"private".to_string()));
}

#[test]
fn workspace_discovery_never_reaches_global_directories() {
    // The hermetic property `discover` exists to guarantee: a test, a CI run,
    // or a headless tool must not silently pick up whatever the developer has
    // installed on their machine.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_skill(&root.join("skills/local"), "local");

    for skill in skill::discover(root) {
        assert_eq!(skill.origin, mt_doc::Origin::Workspace);
        assert!(
            skill.dir.starts_with(root),
            "{} escaped the workspace",
            skill.dir.display()
        );
    }
}

#[test]
fn a_skill_directory_is_a_leaf() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_skill(&root.join("skills/outer"), "outer");
    // Shallower shadows nested, per the reference walker.
    write_skill(&root.join("skills/outer/inner"), "inner");

    let names: Vec<String> = skill::discover(root).into_iter().map(|s| s.name).collect();
    assert!(names.contains(&"outer".to_string()));
    assert!(
        !names.contains(&"inner".to_string()),
        "nested skill must be shadowed"
    );
}

#[test]
fn a_missing_workspace_is_empty_not_a_panic() {
    let skills = skill::discover(Path::new("Q:/definitely/not/here/xyz"));
    assert!(skills.is_empty());
}

#[test]
fn global_discovery_tags_its_results() {
    // Cannot assert on *what* is installed — that is machine dependent — only
    // that anything from outside the workspace is labeled as such, which is
    // what the UI groups on.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_skill(&root.join("skills/local"), "local");

    let skills = skill::discover_with(root, Discovery::everything());
    for skill in &skills {
        match skill.origin {
            mt_doc::Origin::Workspace => assert!(skill.dir.starts_with(root)),
            mt_doc::Origin::Global => assert!(!skill.dir.starts_with(root)),
        }
    }
    assert!(
        skills.iter().any(|s| s.name == "local"),
        "the workspace's own skills must still be found"
    );
}
