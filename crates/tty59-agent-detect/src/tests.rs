// Derived from herdr (https://github.com/herdrdev/herdr) at ee5bfb50, Apache-2.0.
// Modified for tty59: synthetic screen cases and bundled-only detection.

use super::*;

#[test]
fn every_bundled_manifest_compiles() {
    for item in &BUNDLED_MANIFESTS {
        let manifest = load(item.id, item.toml).unwrap_or_else(|err| panic!("{}: {err}", item.id));
        assert_eq!(manifest.rules.len(), manifest.manifest.rules.len());
    }
}

#[test]
fn each_agent_matches_live_chrome() {
    let cases = [
        (
            "agy",
            "↑/↓ Navigate · tab Amend\nesc to cancel",
            State::Blocked,
            "permission_prompt",
        ),
        ("amp", "esc to cancel", State::Working, "esc_cancel_working"),
        (
            "claude",
            "──────\n❯\n──────",
            State::Idle,
            "live_prompt_box",
        ),
        (
            "cline",
            "Let Cline use this tool",
            State::Blocked,
            "tool_permission",
        ),
        (
            "codex",
            "Allow command?\nPress enter to confirm or esc to cancel",
            State::Blocked,
            "live_strong_blocker",
        ),
        (
            "copilot",
            "enter to select · esc to cancel",
            State::Blocked,
            "selection_blocker",
        ),
        (
            "cursor",
            "ctrl+c to stop",
            State::Working,
            "stop_hint_working",
        ),
        (
            "devin",
            "Approve once · select · confirm · esc cancel",
            State::Blocked,
            "permission_prompt",
        ),
        (
            "droid",
            "↑/↓ navigate · enter select · esc cancel",
            State::Blocked,
            "selection_menu_blocker",
        ),
        (
            "gemini",
            "│ Allow execution",
            State::Blocked,
            "apply_or_allow_change",
        ),
        (
            "grok",
            "tab:scrollback · shift+x:dismiss",
            State::Blocked,
            "question_dialog_hints_blocked",
        ),
        (
            "hermes",
            "Hermes needs your answer\nenter send",
            State::Blocked,
            "clarification_prompt",
        ),
        (
            "kilo",
            "△ Permission required",
            State::Blocked,
            "opencode_permission",
        ),
        (
            "kimi",
            "question\n? Which option?\n↑↓ select · esc cancel · ↵ choose",
            State::Blocked,
            "question_panel",
        ),
        (
            "kiro",
            "> Ask a question or describe a task",
            State::Idle,
            "live_prompt_idle",
        ),
        (
            "letta",
            "Run this command?\nEnter to select · Esc to cancel",
            State::Blocked,
            "command_approval",
        ),
        ("maki", " [BUILD]", State::Idle, "status_bar_idle"),
        ("muse", "⟩", State::Idle, "idle_prompt"),
        (
            "opencode",
            "△ Permission required",
            State::Blocked,
            "permission_required",
        ),
        ("pi", "Working...", State::Working, "working_literal"),
        (
            "qodercli",
            "Enter your response",
            State::Blocked,
            "confirmation_or_input_blocker",
        ),
        (
            "qwen",
            "Do you trust this folder?\nTrust folder (Enter)\nDon't trust (Esc)",
            State::Blocked,
            "folder_trust_dialog",
        ),
    ];
    assert_eq!(cases.len(), BUNDLED_MANIFESTS.len());
    let mut failures = Vec::new();
    for (id, screen, expected, rule) in cases {
        let detection =
            detect_screen(id, screen).unwrap_or_else(|| panic!("{id}: missing manifest"));
        if (detection.state, detection.rule_id) != (expected, Some(rule)) {
            failures.push(format!(
                "{id}: expected {expected:?}/{rule}, got {detection:?}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn screen_and_osc_are_independent_and_keep_rule_priority() {
    let title = "⠋ Neutral session";
    let screen = "Allow command?\nPress enter to confirm or esc to cancel";
    assert_eq!(
        detect_screen("codex", screen).unwrap().state,
        State::Blocked
    );
    assert_eq!(
        detect_osc("codex", title, "").unwrap().state,
        State::Working
    );
    assert_eq!(
        detect_osc("codex", "⠋ Action Required", "").unwrap().state,
        State::Blocked
    );
    assert_eq!(
        detect_osc("kiro", "", "4;3;").unwrap().state,
        State::Working
    );
    assert_eq!(detect_osc("claude", "", ""), None);
}

#[test]
fn transcript_and_fallback_are_distinct() {
    let transcript = "Showing detailed transcript\nctrl+o to toggle";
    let detection = detect_screen("claude", transcript).unwrap();
    assert!(detection.skip_state_update);
    assert_eq!(detection.state, State::Unknown);
    assert!(!detection.visible_idle);
    assert_eq!(
        detect_screen("opencode", "Ordinary text"),
        Some(Detection::FALLBACK_IDLE)
    );
    assert_eq!(detect_screen("unknown", "Ordinary text"), None);
}

#[test]
fn validates_region_gates_and_regex_before_evaluating() {
    for extra in [
        "region = \"nonexistent\"",
        "regex = [\"[\"]",
        "skip_state_update = true",
    ] {
        let source = format!(
            "id = \"test\"\n[[rules]]\nid = \"test\"\nstate = \"working\"\ncontains = [\"marker\"]\n{extra}"
        );
        assert!(load("test", &source).is_err(), "{extra}");
    }
}

#[test]
fn launcher_aliases_do_not_require_matching_free_text() {
    assert_eq!(manifest_id_for_launcher("Devin-cli"), Some("devin"));
    assert_eq!(manifest_id_for_launcher("kilo-code"), Some("kilo"));
    assert_eq!(manifest_id_for_launcher(".cline"), Some("cline"));
    assert_eq!(manifest_id_for_launcher("some codex text"), None);
}
