//! Which machine the Open Workspace picker is looking at.
//!
//! The left column is one flat, most-recently-used list across every machine.
//! The machine rail above it narrows that list to one machine: pick a chip,
//! step with Alt+←/→, or type `@name`. The scope is the group's `key` — empty
//! for this computer, the target's spelling for a remote — so it survives the
//! groups being rebuilt every frame without holding on to any of them.
//!
//! Everything here is pure over `&[Group]` so it can be tested without a
//! window. Rendering lives in `render_rail` in the parent module.

// Wired up piece by piece by the rail, key and `@`-commit packages; the
// last of them removes this.
#![allow(dead_code)]

use super::{Group, Link};

/// What the picker's left column is narrowed to. Reset to `All` every time the
/// picker opens (not remembered): Ctrl+Tab would otherwise quietly hide every
/// other machine's workspaces.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum MachineScope {
    #[default]
    All,
    /// One machine, by its group key ("" is this computer).
    Machine(String),
}

impl MachineScope {
    pub(crate) fn local() -> Self {
        MachineScope::Machine(String::new())
    }

    pub(super) fn admits(&self, group: &Group) -> bool {
        match self {
            MachineScope::All => true,
            MachineScope::Machine(key) => &group.key == key,
        }
    }

    /// The group this scope names, when it names one that exists this frame.
    pub(super) fn group<'a>(&self, groups: &'a [Group]) -> Option<&'a Group> {
        match self {
            MachineScope::All => None,
            MachineScope::Machine(key) => groups.iter().find(|g| &g.key == key),
        }
    }
}

/// One chip on the machine rail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RailEntry {
    pub(super) scope: MachineScope,
    pub(super) label: String,
    /// `None` for the All chip, which has no link of its own.
    pub(super) link: Option<Link>,
    /// Workspaces listed under this chip, before any search.
    pub(super) count: usize,
}

/// The rail, left to right: All, this computer, then remote machines — the
/// ones with workspaces first, then the configured-but-empty ones — each run
/// alphabetical. `all_label` is the localized "All".
pub(super) fn rail_entries(groups: &[Group], all_label: &str) -> Vec<RailEntry> {
    let mut out = vec![RailEntry {
        scope: MachineScope::All,
        label: all_label.to_string(),
        link: None,
        count: groups.iter().map(|g| g.rows.len()).sum(),
    }];
    let entry = |g: &Group| RailEntry {
        scope: MachineScope::Machine(g.key.clone()),
        label: g.label.clone(),
        link: Some(g.link),
        count: g.rows.len(),
    };
    out.extend(groups.iter().filter(|g| g.key.is_empty()).map(entry));
    let mut remotes: Vec<&Group> = groups.iter().filter(|g| !g.key.is_empty()).collect();
    remotes.sort_by(|a, b| {
        a.rows
            .is_empty()
            .cmp(&b.rows.is_empty())
            .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
            .then_with(|| a.key.cmp(&b.key))
    });
    out.extend(remotes.into_iter().map(entry));
    out
}

/// The scope `delta` chips away from `current`, wrapping at both ends. A
/// scope that is no longer on the rail (its machine was removed while the
/// picker was up) steps from All.
pub(super) fn step(entries: &[RailEntry], current: &MachineScope, delta: isize) -> MachineScope {
    if entries.is_empty() {
        return MachineScope::All;
    }
    let at = entries
        .iter()
        .position(|e| &e.scope == current)
        .unwrap_or(0) as isize;
    let len = entries.len() as isize;
    entries[(at + delta).rem_euclid(len) as usize].scope.clone()
}

/// A leading `@token` in the search box: the machine needle and what is left of
/// the query. `"@dev api"` → `("dev", "api")`; `"@dev"` → `("dev", "")`;
/// `"@"` → `("", "")`. `None` when the query does not start with `@`.
/// `committed` is whether whitespace followed the token — the point at which
/// the picker turns the token into a scope and drops it from the box.
pub(super) struct AtToken<'a> {
    pub(super) needle: &'a str,
    pub(super) rest: &'a str,
    pub(super) committed: bool,
}

pub(super) fn parse_at_token(query: &str) -> Option<AtToken<'_>> {
    let body = query.trim_start().strip_prefix('@')?;
    match body.find(char::is_whitespace) {
        Some(at) => Some(AtToken {
            needle: &body[..at],
            rest: body[at..].trim_start(),
            committed: true,
        }),
        None => Some(AtToken {
            needle: body,
            rest: "",
            committed: false,
        }),
    }
}

/// The machine an `@needle` names: the best fuzzy hit on a rail label. An
/// empty needle names nothing, so a bare `@` leaves the list alone.
pub(super) fn resolve_needle(entries: &[RailEntry], needle: &str) -> Option<MachineScope> {
    if needle.is_empty() {
        return None;
    }
    entries
        .iter()
        .filter(|e| e.scope != MachineScope::All)
        .filter_map(|e| crate::ui::search::fuzzy_score(needle, &e.label).map(|score| (score, e)))
        // Highest score wins; ties go to the earlier chip, which is the
        // order the user sees.
        .fold(None::<(i32, &RailEntry)>, |best, (score, e)| match best {
            Some((b, _)) if b >= score => best,
            _ => Some((score, e)),
        })
        .map(|(_, e)| e.scope.clone())
}

/// What the list is narrowed to this frame and the query left to search with:
/// a leading `@token` that resolves overrides the stored scope while it is
/// being typed, so the list previews the machine before the space commits it.
pub(super) fn effective<'a>(
    stored: &MachineScope,
    query: &'a str,
    entries: &[RailEntry],
) -> (MachineScope, &'a str) {
    match parse_at_token(query) {
        Some(token) => match resolve_needle(entries, token.needle) {
            Some(scope) => (scope, token.rest),
            None => (stored.clone(), token.rest),
        },
        None => (stored.clone(), query),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{group, named_group, row};
    use super::*;

    fn rail() -> (Vec<Group>, Vec<RailEntry>) {
        let groups = vec![
            named_group("zeta", vec![]),
            group(vec![row("notes", vec![])]),
            named_group("devbox", vec![]),
            named_group("blackbox", vec![row("api", vec![]), row("web", vec![])]),
        ];
        let entries = rail_entries(&groups, "All");
        (groups, entries)
    }

    #[test]
    fn the_rail_is_all_then_local_then_busy_remotes_then_empty_ones() {
        let (_, entries) = rail();
        let labels: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["All", "This Computer", "blackbox", "devbox", "zeta"]
        );
        assert_eq!(entries[0].count, 3);
        assert_eq!(entries[0].link, None);
        assert_eq!(entries[1].scope, MachineScope::local());
        assert_eq!(entries[2].count, 2);
    }

    #[test]
    fn stepping_wraps_both_ways_and_recovers_from_a_vanished_scope() {
        let (_, entries) = rail();
        assert_eq!(step(&entries, &MachineScope::All, 1), MachineScope::local());
        assert_eq!(
            step(&entries, &MachineScope::All, -1),
            MachineScope::Machine("zeta".into())
        );
        let gone = MachineScope::Machine("removed".into());
        assert_eq!(step(&entries, &gone, 1), MachineScope::local());
    }

    #[test]
    fn a_scope_admits_only_its_own_group() {
        let (groups, _) = rail();
        let scope = MachineScope::Machine("blackbox".into());
        let admitted: Vec<&str> = groups
            .iter()
            .filter(|g| scope.admits(g))
            .map(|g| g.label.as_str())
            .collect();
        assert_eq!(admitted, vec!["blackbox"]);
        assert!(groups.iter().all(|g| MachineScope::All.admits(g)));
        assert_eq!(scope.group(&groups).unwrap().rows.len(), 2);
    }

    #[test]
    fn at_tokens_parse_while_typed_and_commit_on_whitespace() {
        let t = parse_at_token("@dev").unwrap();
        assert_eq!((t.needle, t.rest, t.committed), ("dev", "", false));
        let t = parse_at_token("@dev  api x").unwrap();
        assert_eq!((t.needle, t.rest, t.committed), ("dev", "api x", true));
        let t = parse_at_token("@").unwrap();
        assert_eq!((t.needle, t.committed), ("", false));
        assert!(parse_at_token("dev @box").is_none());
        assert!(parse_at_token("").is_none());
    }

    #[test]
    fn a_needle_resolves_to_the_best_machine_and_never_to_all() {
        let (_, entries) = rail();
        assert_eq!(
            resolve_needle(&entries, "dev"),
            Some(MachineScope::Machine("devbox".into()))
        );
        assert_eq!(
            resolve_needle(&entries, "black"),
            Some(MachineScope::Machine("blackbox".into()))
        );
        assert_eq!(resolve_needle(&entries, ""), None);
        assert_eq!(resolve_needle(&entries, "all"), None);
        assert_eq!(resolve_needle(&entries, "qqqq"), None);
    }

    #[test]
    fn an_at_token_previews_its_machine_and_hands_back_the_rest() {
        let (_, entries) = rail();
        let stored = MachineScope::All;
        let (scope, rest) = effective(&stored, "@black api", &entries);
        assert_eq!(scope, MachineScope::Machine("blackbox".into()));
        assert_eq!(rest, "api");
        assert_eq!(
            effective(&stored, "api", &entries),
            (MachineScope::All, "api")
        );
        // An unresolvable needle keeps the stored scope and searches nothing
        // rather than searching for the literal "@qqq".
        let local = MachineScope::local();
        assert_eq!(effective(&local, "@qqq", &entries), (local.clone(), ""));
    }
}
