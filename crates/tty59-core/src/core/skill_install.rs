//! Writing an agent skill to disk: `tty59 skill install` on the host and
//! `tty59-server skill install` on a remote machine.
//!
//! A skill is a directory of text files (`SKILL.md`, references, a helper
//! script) that a coding agent loads by name. Both binaries carry theirs
//! embedded, so the skill on disk always matches the binary that wrote it.
//! This module is the part they share: where a skill goes, and how to write
//! one without destroying anything the user wrote by hand.
//!
//! The rules:
//!
//! - A directory tty59 wrote carries a [`MARKER`] file listing what it wrote.
//!   Reinstalling over it replaces those files and removes the ones the new
//!   version no longer ships.
//! - A directory without the marker is the user's own skill. It is refused
//!   unless the caller passes `force`, and even then it is moved aside to
//!   `<name>.bak-<unix time>`, never deleted.

use std::io;
use std::path::{Path, PathBuf};

/// Names the files tty59 wrote into a skill directory, one relative path per
/// line after a `tty59-skill <version>` header.
pub const MARKER: &str = ".tty59-skill";

/// One file of a skill, relative to the skill's own directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillFile {
    pub path: &'static str,
    pub contents: String,
    /// Mode 0755 on unix: a helper the skill tells the agent to run.
    pub executable: bool,
}

/// Which agent's skills directory to install into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillTarget {
    /// `$CLAUDE_CONFIG_DIR/skills`, else `~/.claude/skills`.
    Claude,
    /// `~/.agents/skills`, the shared directory that Codex, OpenCode and the
    /// `skills` CLI read.
    Agents,
}

impl SkillTarget {
    pub fn parse(s: &str) -> Option<SkillTarget> {
        match s.to_ascii_lowercase().as_str() {
            "claude" | "claude-code" => Some(SkillTarget::Claude),
            "agents" | "codex" | "opencode" => Some(SkillTarget::Agents),
            _ => None,
        }
    }

    /// The directory skills live in for this agent, or `None` when there is no
    /// home directory to put it under.
    pub fn skills_dir(self) -> Option<PathBuf> {
        match self {
            SkillTarget::Claude => std::env::var_os("CLAUDE_CONFIG_DIR")
                .filter(|d| !d.is_empty())
                .map(PathBuf::from)
                .or_else(|| home().map(|h| h.join(".claude")))
                .map(|d| d.join("skills")),
            SkillTarget::Agents => home().map(|h| h.join(".agents").join("skills")),
        }
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// What an install did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallReport {
    /// The skill's own directory, `<skills dir>/<name>`.
    pub dir: PathBuf,
    /// Files written, relative to `dir`.
    pub written: Vec<String>,
    /// Files from an earlier tty59 install that this version no longer ships.
    pub removed: Vec<String>,
    /// Where a skill tty59 did not write was moved, under `force`.
    pub backup: Option<PathBuf>,
    /// Whether a tty59 install was already there.
    pub updated: bool,
}

/// Write `files` into `<skills_dir>/<name>`.
///
/// Refuses, writing nothing, when that directory holds a skill tty59 did not
/// install, unless `force` is set; `force` moves it aside first.
pub fn install(
    skills_dir: &Path,
    name: &str,
    version: &str,
    files: &[SkillFile],
    force: bool,
) -> io::Result<InstallReport> {
    let dir = skills_dir.join(name);
    let mut backup = None;
    let previous = match std::fs::read_to_string(dir.join(MARKER)) {
        Ok(marker) => Some(listed_files(&marker)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if is_nonempty_dir(&dir)? {
                if !force {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!(
                            "{} holds a skill tty59 did not install; --force moves it aside \
                             to {name}.bak-<time> and installs over it",
                            dir.display()
                        ),
                    ));
                }
                let aside = skills_dir.join(format!("{name}.bak-{}", unix_now()));
                std::fs::rename(&dir, &aside)?;
                backup = Some(aside);
            } else if dir.exists() && !dir.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("{} exists and is not a directory", dir.display()),
                ));
            }
            None
        }
        Err(e) => return Err(e),
    };

    std::fs::create_dir_all(&dir)?;
    let mut written = Vec::new();
    for file in files {
        let path = dir.join(file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        write_atomically(&path, file.contents.as_bytes(), file.executable)?;
        written.push(file.path.to_string());
    }

    let mut removed = Vec::new();
    for old in previous.iter().flatten() {
        if written.iter().any(|w| w == old) || !is_safe_relative(old) {
            continue;
        }
        match std::fs::remove_file(dir.join(old)) {
            Ok(()) => removed.push(old.clone()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }

    let mut marker = format!("tty59-skill {version}\n");
    for w in &written {
        marker.push_str(w);
        marker.push('\n');
    }
    write_atomically(&dir.join(MARKER), marker.as_bytes(), false)?;

    Ok(InstallReport {
        dir,
        written,
        removed,
        backup,
        updated: previous.is_some(),
    })
}

fn listed_files(marker: &str) -> Vec<String> {
    marker
        .lines()
        .skip(1)
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// A marker is a file on disk anyone could edit, so a line naming `../x` or
/// an absolute path must not become a delete outside the skill directory.
fn is_safe_relative(path: &str) -> bool {
    let p = Path::new(path);
    !p.is_absolute()
        && p.components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

fn is_nonempty_dir(dir: &Path) -> io::Result<bool> {
    match std::fs::read_dir(dir) {
        Ok(mut entries) => Ok(entries.next().is_some()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) if e.kind() == io::ErrorKind::NotADirectory => Ok(false),
        Err(e) => Err(e),
    }
}

fn write_atomically(path: &Path, bytes: &[u8], executable: bool) -> io::Result<()> {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{file_name}.tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = if executable { 0o755 } else { 0o644 };
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = executable;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &'static str, contents: &str) -> SkillFile {
        SkillFile {
            path,
            contents: contents.to_string(),
            executable: false,
        }
    }

    #[test]
    fn a_fresh_install_writes_the_files_and_the_marker() {
        let root = tempfile::tempdir().unwrap();
        let report = install(
            root.path(),
            "tty59",
            "1.0",
            &[file("SKILL.md", "a"), file("references/x.md", "b")],
            false,
        )
        .unwrap();
        assert!(!report.updated);
        let dir = root.path().join("tty59");
        assert_eq!(std::fs::read_to_string(dir.join("SKILL.md")).unwrap(), "a");
        assert_eq!(
            std::fs::read_to_string(dir.join("references/x.md")).unwrap(),
            "b"
        );
        let marker = std::fs::read_to_string(dir.join(MARKER)).unwrap();
        assert_eq!(marker, "tty59-skill 1.0\nSKILL.md\nreferences/x.md\n");
    }

    #[test]
    fn reinstalling_replaces_files_and_drops_ones_no_longer_shipped() {
        let root = tempfile::tempdir().unwrap();
        install(
            root.path(),
            "tty59",
            "1.0",
            &[file("SKILL.md", "old"), file("references/gone.md", "x")],
            false,
        )
        .unwrap();
        let report = install(
            root.path(),
            "tty59",
            "2.0",
            &[file("SKILL.md", "new")],
            false,
        )
        .unwrap();
        assert!(report.updated);
        assert_eq!(report.removed, vec!["references/gone.md".to_string()]);
        let dir = root.path().join("tty59");
        assert_eq!(
            std::fs::read_to_string(dir.join("SKILL.md")).unwrap(),
            "new"
        );
        assert!(!dir.join("references/gone.md").exists());
    }

    #[test]
    fn a_skill_the_user_wrote_is_refused_and_untouched() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("tty59");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "mine").unwrap();
        let err =
            install(root.path(), "tty59", "1.0", &[file("SKILL.md", "x")], false).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            std::fs::read_to_string(dir.join("SKILL.md")).unwrap(),
            "mine"
        );
    }

    #[test]
    fn force_moves_the_users_skill_aside_instead_of_deleting_it() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("tty59");
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::write(dir.join("scripts/helper"), "mine").unwrap();
        let report = install(root.path(), "tty59", "1.0", &[file("SKILL.md", "x")], true).unwrap();
        let aside = report.backup.expect("moved aside");
        assert_eq!(
            std::fs::read_to_string(aside.join("scripts/helper")).unwrap(),
            "mine"
        );
        assert!(!dir.join("scripts/helper").exists());
        assert_eq!(std::fs::read_to_string(dir.join("SKILL.md")).unwrap(), "x");
    }

    #[test]
    fn a_marker_line_pointing_outside_the_skill_is_never_deleted() {
        let root = tempfile::tempdir().unwrap();
        let outside = root.path().join("keep.txt");
        std::fs::write(&outside, "keep").unwrap();
        let dir = root.path().join("tty59");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MARKER), "tty59-skill 0\n../keep.txt\n").unwrap();
        install(root.path(), "tty59", "1.0", &[file("SKILL.md", "x")], false).unwrap();
        assert!(outside.exists());
    }

    #[cfg(unix)]
    #[test]
    fn helpers_are_executable() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = tempfile::tempdir().unwrap();
        let mut helper = file("scripts/tty59", "#!/bin/sh\n");
        helper.executable = true;
        install(root.path(), "s", "1.0", &[helper], false).unwrap();
        let mode = std::fs::metadata(root.path().join("s/scripts/tty59"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
    }
}
