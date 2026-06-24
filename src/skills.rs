//! Bundled Claude Code / agent skills and their installation.
//!
//! Marathon ships a `marathon` skill (authoring guidance for runbooks) embedded in
//! the binary at build time. `marathon skills install` writes it into a skills
//! directory. Two independent axes:
//!
//! - **Scope** — a project's working tree, or the user's `$HOME`. The caller passes
//!   the resolved base directory; this module just joins beneath it.
//! - **Target** — `.claude/skills`, `.agents/skills`, or [`Both`](Target::Both). For
//!   `Both` the skill is written once under `.agents/skills` and `.claude/skills`
//!   becomes a relative symlink to it, so the two agent ecosystems share one copy.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Directory name created for the installed skill.
pub const SKILL_NAME: &str = "marathon";

/// The bundled `SKILL.md`, embedded from `assets/` at build time.
pub const SKILL_MD: &str = include_str!("../assets/skills/marathon/SKILL.md");

/// Agent root directory names (each holds a `skills/` subtree).
const CLAUDE_DIR: &str = ".claude";
const AGENTS_DIR: &str = ".agents";

/// Which agent skills directory(ies) to install into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Target {
    /// `.claude/skills` only.
    Claude,
    /// `.agents/skills` only.
    Agents,
    /// Write to `.agents/skills`; symlink `.claude/skills` to it (default).
    #[default]
    Both,
}

/// What an install actually did, for reporting back to the user.
#[derive(Debug)]
pub struct InstallReport {
    /// The `SKILL.md` file written on disk.
    pub written: PathBuf,
    /// The symlink created (`.claude/skills/marathon` → the agents copy), if any.
    pub linked: Option<PathBuf>,
}

/// Install the bundled skill beneath `base` (a project root or `$HOME`) for `target`.
/// Refuses to overwrite anything that already exists unless `force` is set.
pub fn install(base: &Path, target: Target, force: bool) -> Result<InstallReport> {
    match target {
        Target::Claude => Ok(InstallReport {
            written: write_skill(&skill_dir(base, CLAUDE_DIR), force)?,
            linked: None,
        }),
        Target::Agents => Ok(InstallReport {
            written: write_skill(&skill_dir(base, AGENTS_DIR), force)?,
            linked: None,
        }),
        Target::Both => {
            // Canonical copy under .agents; .claude points at it via a relative link.
            let written = write_skill(&skill_dir(base, AGENTS_DIR), force)?;
            let link = skill_dir(base, CLAUDE_DIR);
            link_to_agents(&link, force)?;
            Ok(InstallReport {
                written,
                linked: Some(link),
            })
        }
    }
}

/// `<base>/<agent>/skills/marathon`.
fn skill_dir(base: &Path, agent: &str) -> PathBuf {
    base.join(agent).join("skills").join(SKILL_NAME)
}

/// Write `SKILL.md` into `dir`, creating parents. Refuses to clobber unless `force`.
fn write_skill(dir: &Path, force: bool) -> Result<PathBuf> {
    let file = dir.join("SKILL.md");
    remove_existing(&file, force)?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    std::fs::write(&file, SKILL_MD).with_context(|| format!("writing {}", file.display()))?;
    Ok(file)
}

/// Create a relative symlink at `link` (`.claude/skills/marathon`) pointing at the
/// sibling `.agents/skills/marathon`. Both live at the same depth under `base`, so
/// the target is always `../../.agents/skills/marathon`.
fn link_to_agents(link: &Path, force: bool) -> Result<()> {
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    remove_existing(link, force)?;
    let target = Path::new("..")
        .join("..")
        .join(AGENTS_DIR)
        .join("skills")
        .join(SKILL_NAME);
    symlink_dir(&target, link)
        .with_context(|| format!("linking {} -> {}", link.display(), target.display()))
}

/// Remove whatever currently lives at `path` so we can write fresh. A no-op if it's
/// absent. If it exists and `force` is unset, error instead of clobbering. Handles
/// symlinks (don't follow), files, and directories.
fn remove_existing(path: &Path, force: bool) -> Result<()> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("checking {}", path.display())),
    };
    if !force {
        anyhow::bail!(
            "{} already exists — pass --force to overwrite",
            path.display()
        );
    }
    if meta.file_type().is_symlink() || meta.is_file() {
        std::fs::remove_file(path).with_context(|| format!("removing {}", path.display()))
    } else {
        std::fs::remove_dir_all(path).with_context(|| format!("removing {}", path.display()))
    }
}

#[cfg(unix)]
fn symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_skill_has_valid_frontmatter() {
        assert!(SKILL_MD.starts_with("---\n"), "missing frontmatter open");
        assert!(
            SKILL_MD.contains("name: marathon"),
            "skill name should be `marathon`"
        );
        assert!(
            !SKILL_MD.contains('\u{200b}'),
            "skill contains a zero-width space"
        );
    }

    #[test]
    fn install_claude_only() {
        let tmp = tempfile::TempDir::new().unwrap();
        let report = install(tmp.path(), Target::Claude, false).unwrap();

        assert_eq!(
            report.written,
            tmp.path().join(".claude/skills/marathon/SKILL.md")
        );
        assert!(report.linked.is_none());
        assert!(!tmp.path().join(".agents").exists());
        assert_eq!(std::fs::read_to_string(&report.written).unwrap(), SKILL_MD);
    }

    #[test]
    fn install_agents_only() {
        let tmp = tempfile::TempDir::new().unwrap();
        let report = install(tmp.path(), Target::Agents, false).unwrap();

        assert_eq!(
            report.written,
            tmp.path().join(".agents/skills/marathon/SKILL.md")
        );
        assert!(report.linked.is_none());
        assert!(!tmp.path().join(".claude").exists());
    }

    #[test]
    fn install_both_writes_agents_and_links_claude() {
        let tmp = tempfile::TempDir::new().unwrap();
        let report = install(tmp.path(), Target::Both, false).unwrap();

        // Real file under .agents.
        assert_eq!(
            report.written,
            tmp.path().join(".agents/skills/marathon/SKILL.md")
        );
        // .claude/skills/marathon is a symlink to it.
        let link = tmp.path().join(".claude/skills/marathon");
        assert_eq!(report.linked.as_deref(), Some(link.as_path()));
        assert!(
            std::fs::symlink_metadata(&link).unwrap().is_symlink(),
            "claude path should be a symlink"
        );
        // The link resolves and serves the same content.
        let via_link = std::fs::read_to_string(link.join("SKILL.md")).unwrap();
        assert_eq!(via_link, SKILL_MD);
        // It's a *relative* link (portable if the tree moves).
        let raw = std::fs::read_link(&link).unwrap();
        assert!(raw.is_relative(), "symlink should be relative: {raw:?}");
    }

    #[test]
    fn refuses_to_clobber_without_force() {
        let tmp = tempfile::TempDir::new().unwrap();
        install(tmp.path(), Target::Claude, false).unwrap();
        assert!(install(tmp.path(), Target::Claude, false).is_err());
        assert!(install(tmp.path(), Target::Claude, true).is_ok());
    }

    #[test]
    fn force_replaces_an_existing_both_install() {
        let tmp = tempfile::TempDir::new().unwrap();
        install(tmp.path(), Target::Both, false).unwrap();
        // Re-running with force replaces the file and the symlink cleanly.
        let report = install(tmp.path(), Target::Both, true).unwrap();
        let link = report.linked.unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(std::fs::read_to_string(&report.written).unwrap(), SKILL_MD);
    }
}
