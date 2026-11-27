//! `tty59 skill`: the agent skill in `skills/tty59/`, embedded in this binary.
//!
//! Embedding it means the skill an agent reads always describes the CLI it
//! will run: `tty59 skill install` after an upgrade brings the two back in
//! step, with no network and no `npx`.

use anyhow::{Context as _, Result, bail};
use serde_json::json;
use tty59_core::core::skill_install::{self, SkillFile, SkillTarget};

use crate::commands::{Outcome, Report};

/// The skill's directory name, which is also the name agents load it by.
pub const NAME: &str = "tty59";

/// Every file of the skill, by its path inside the skill directory.
const FILES: &[(&str, &str)] = &[
    ("SKILL.md", include_str!("../../../skills/tty59/SKILL.md")),
    (
        "references/commands.md",
        include_str!("../../../skills/tty59/references/commands.md"),
    ),
    (
        "references/delegation.md",
        include_str!("../../../skills/tty59/references/delegation.md"),
    ),
];

pub fn files() -> Vec<SkillFile> {
    FILES
        .iter()
        .map(|(path, contents)| SkillFile {
            path,
            contents: contents.to_string(),
            executable: false,
        })
        .collect()
}

pub fn install(
    agent: &str,
    dir: Option<std::path::PathBuf>,
    force: bool,
    machine: Option<&str>,
) -> Result<Outcome> {
    if machine.is_some() {
        bail!(
            "`skill install` writes to this machine's disk and cannot be routed with -m; \
             on a remote machine run `tty59-server skill install` there"
        );
    }
    let root = match dir {
        Some(dir) => dir,
        None => {
            let target = SkillTarget::parse(agent).with_context(|| {
                format!("unknown --agent {agent:?}: expected `claude` or `agents`")
            })?;
            target
                .skills_dir()
                .context("no home directory to install the skill under; pass --dir")?
        }
    };
    let report = skill_install::install(&root, NAME, env!("CARGO_PKG_VERSION"), &files(), force)
        .with_context(|| format!("installing the {NAME} skill into {}", root.display()))?;
    let mut human = format!(
        "{} the {NAME} skill at {}\n",
        if report.updated {
            "updated"
        } else {
            "installed"
        },
        report.dir.display()
    );
    if let Some(aside) = &report.backup {
        human.push_str(&format!(
            "the skill that was there is now at {}\n",
            aside.display()
        ));
    }
    Ok(Outcome::Report(Report {
        human,
        json: json!({
            "dir": report.dir,
            "written": report.written,
            "removed": report.removed,
            "backup": report.backup,
            "updated": report.updated,
        }),
    }))
}

pub fn show(file: Option<&str>) -> Result<Outcome> {
    let wanted = file.unwrap_or("SKILL.md");
    let Some((path, contents)) = FILES.iter().find(|(path, _)| *path == wanted) else {
        bail!(
            "the skill has no file {wanted:?}; it has: {}",
            FILES.iter().map(|(p, _)| *p).collect::<Vec<_>>().join(", ")
        );
    };
    Ok(Outcome::Report(Report {
        human: contents.to_string(),
        json: json!({ "path": path, "contents": contents }),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_skill_is_the_one_in_the_repo() {
        let skill = files();
        assert_eq!(skill[0].path, "SKILL.md");
        assert!(skill[0].contents.starts_with("---\nname: tty59\n"));
        // Every reference SKILL.md points at ships with it.
        for (path, _) in FILES.iter().skip(1) {
            assert!(
                skill[0].contents.contains(path),
                "SKILL.md never mentions {path}"
            );
        }
    }

    #[test]
    fn install_writes_every_file() {
        let root = tempfile::tempdir().unwrap();
        install("claude", Some(root.path().to_path_buf()), false, None).unwrap();
        for (path, contents) in FILES {
            let on_disk = std::fs::read_to_string(root.path().join(NAME).join(path)).unwrap();
            assert_eq!(&on_disk, contents);
        }
    }

    #[test]
    fn install_is_not_routable() {
        let err = install("claude", None, false, Some("devbox")).unwrap_err();
        assert!(err.to_string().contains("tty59-server skill install"));
    }
}
