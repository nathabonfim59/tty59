//! `tty59-server skill install`: the agent skill for a machine that has only
//! this binary — a remote host, a VM, a container.
//!
//! The skill teaches an agent in one of this machine's panes to use the pane
//! verbs in [`crate::verbs`]. It ships with a small `scripts/tty59` shim, because
//! the server is installed as `tty59-server-c<control>p<protocol>` under
//! `~/.local/share/tty59/bin` and that name changes whenever the dialect does:
//! the shim finds whichever one is there now.

use std::process::ExitCode;

use tty59_core::core::skill_install::{self, SkillFile, SkillTarget};

/// The directory name, and the name the agent loads the skill by. Distinct
/// from the host's `tty59` skill, so a machine that has both keeps both.
pub const NAME: &str = "tty59-remote";

const SKILL_MD: &str = include_str!("../skill/SKILL.md");
const SHIM: &str = include_str!("../skill/tty59.sh");

/// Placeholders the install fills in.
const SHIM_PATH: &str = "{{SHIM}}";
const INSTALLED_SERVER: &str = "{{SERVER}}";

pub const USAGE: &str = "\
SKILL:
    tty59-server skill install [--agent claude|agents] [--dir DIR] [--force]
                                   write the tty59-remote agent skill on this machine
    tty59-server skill show [SKILL.md|scripts/tty59]
";

pub fn run(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("install") => install(&args[1..]),
        Some("show") => show(args.get(1).map(String::as_str)),
        _ => {
            eprint!("tty59-server: `skill` takes `install` or `show`\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn install(args: &[String]) -> ExitCode {
    let mut agent = "claude".to_string();
    let mut dir = None;
    let mut force = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--agent" | "--dir" => {
                let Some(v) = it.next() else {
                    eprintln!("tty59-server: {a} needs a value");
                    return ExitCode::from(2);
                };
                if a == "--agent" {
                    agent = v.clone();
                } else {
                    dir = Some(std::path::PathBuf::from(v));
                }
            }
            "--force" => force = true,
            other => {
                eprint!("tty59-server: `skill install` does not take {other:?}\n\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let root = match dir {
        Some(d) => d,
        None => match SkillTarget::parse(&agent).and_then(SkillTarget::skills_dir) {
            Some(d) => d,
            None => {
                eprintln!(
                    "tty59-server: no skills directory for --agent {agent:?} (expected claude \
                     or agents, and a home directory); pass --dir"
                );
                return ExitCode::from(2);
            }
        },
    };
    let shim = root.join(NAME).join("scripts").join("tty59");
    let server = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let files = files(&shim.display().to_string(), &server);
    match skill_install::install(&root, NAME, env!("CARGO_PKG_VERSION"), &files, force) {
        Ok(report) => {
            println!(
                "{} the {NAME} skill at {}",
                if report.updated {
                    "updated"
                } else {
                    "installed"
                },
                report.dir.display()
            );
            if let Some(aside) = report.backup {
                println!("the skill that was there is now at {}", aside.display());
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("tty59-server: installing the {NAME} skill: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The skill with its placeholders filled: the shim's own absolute path in
/// `SKILL.md`, and the server that wrote it as the shim's first guess.
pub fn files(shim_path: &str, server: &str) -> Vec<SkillFile> {
    vec![
        SkillFile {
            path: "SKILL.md",
            contents: SKILL_MD.replace(SHIM_PATH, shim_path),
            executable: false,
        },
        SkillFile {
            path: "scripts/tty59",
            contents: SHIM.replace(INSTALLED_SERVER, &shell_single_quoted(server)),
            executable: true,
        },
    ]
}

fn shell_single_quoted(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn show(file: Option<&str>) -> ExitCode {
    let files = files("<skills dir>/tty59-remote/scripts/tty59", "<this binary>");
    let wanted = file.unwrap_or("SKILL.md");
    match files.iter().find(|f| f.path == wanted) {
        Some(f) => {
            print!("{}", f.contents);
            ExitCode::SUCCESS
        }
        None => {
            eprintln!(
                "tty59-server: the skill has no file {wanted:?}; it has SKILL.md, scripts/tty59"
            );
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_filled() {
        let files = files(
            "/home/me/.claude/skills/tty59-remote/scripts/tty59",
            "/x/it's",
        );
        assert!(!files[0].contents.contains("{{"), "{}", files[0].contents);
        assert!(!files[1].contents.contains("{{"), "{}", files[1].contents);
        assert!(
            files[0]
                .contents
                .contains("/home/me/.claude/skills/tty59-remote/scripts/tty59")
        );
        assert!(files[1].contents.contains(r"'/x/it'\''s'"));
        assert!(files[0].contents.starts_with("---\nname: tty59-remote\n"));
    }

    #[test]
    fn every_verb_the_skill_names_exists() {
        for verb in crate::verbs::VERBS {
            assert!(
                SKILL_MD.contains(&format!("$T {verb}")),
                "SKILL.md never shows {verb}"
            );
        }
    }
}
