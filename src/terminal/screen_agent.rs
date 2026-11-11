use std::time::{Duration, Instant};

use crate::core::cli_agent::{
    AgentScreenState, AgentSessionState, agent_manifest_id, detect_screen,
};
use crate::daemon::protocol::AgentScreenHint;

use super::RemoteTerminal;

const MAX_ROWS: usize = 96;
const MAX_COLUMNS: usize = 320;
const REFRESH: Duration = Duration::from_millis(850);
const CONFIRM_IDLE: Duration = Duration::from_millis(280);

#[derive(Default)]
pub(super) struct ScreenAgent {
    sample: Option<Sample>,
    preview: std::cell::RefCell<Option<Preview>>,
}

struct Preview {
    revision: u64,
    sampled: Instant,
    line: Option<String>,
}

struct Sample {
    hint: AgentScreenHint,
    grid_revision: u64,
    text: String,
    sent: Instant,
    actionable: bool,
    idle: bool,
}

impl Sample {
    fn refresh(&mut self, terminal: &RemoteTerminal, mid_turn: bool, now: Instant) {
        let interval = if self.idle { CONFIRM_IDLE } else { REFRESH };
        if (self.actionable || mid_turn) && now.saturating_duration_since(self.sent) >= interval {
            terminal.report_agent_screen(self.hint.clone());
            self.sent = now;
        }
    }
}

fn manifest_epoch(session: &AgentSessionState) -> Option<(&'static str, u64)> {
    let detection = session
        .detection
        .as_ref()
        .filter(|detection| !detection.exited)?;
    Some((agent_manifest_id(&detection.kind)?, detection.epoch))
}

impl ScreenAgent {
    pub(super) fn refresh_preview(&self, terminal: &RemoteTerminal) -> bool {
        let previous = self
            .preview
            .borrow()
            .as_ref()
            .and_then(|preview| preview.line.clone());
        previous != self.preview(terminal)
    }

    pub(super) fn preview(&self, terminal: &RemoteTerminal) -> Option<String> {
        let term = terminal.term.lock();
        let revision = term.content_revision();
        let now = Instant::now();
        if let Some(cached) = self.preview.borrow().as_ref()
            && (cached.revision == revision
                || now.duration_since(cached.sampled) < Duration::from_millis(300)
                || term.sync_timeout().is_some())
        {
            return cached.line.clone();
        }
        let grid = term.view();
        let rows = super::composer::screen_rows_bounded(
            &grid,
            grid.screen_lines().saturating_sub(MAX_ROWS),
            grid.columns().min(180),
        );
        drop(term);
        let line: Option<String> = rows
            .iter()
            .rev()
            .map(|line| line.trim())
            .find(|line| line.chars().filter(|c| c.is_alphanumeric()).take(4).count() == 4)
            .map(|line| line.chars().take(180).collect());
        *self.preview.borrow_mut() = Some(Preview {
            revision,
            sampled: now,
            line: line.clone(),
        });
        line
    }

    pub(super) fn poll(&mut self, terminal: &RemoteTerminal, now: Instant) {
        let Some(session) = terminal.agent_session() else {
            self.sample = None;
            return;
        };
        let Some((manifest, epoch)) = manifest_epoch(&session) else {
            self.sample = None;
            return;
        };
        let term = terminal.term.lock();
        if term.sync_timeout().is_some() {
            return;
        }
        let revision = term.content_revision();
        if let Some(sample) = self
            .sample
            .as_mut()
            .filter(|sample| sample.hint.epoch == epoch && sample.grid_revision == revision)
        {
            drop(term);
            sample.refresh(terminal, session.mid_turn(), now);
            return;
        }
        let grid = term.view();
        let rows = super::composer::screen_rows_bounded(
            &grid,
            grid.screen_lines().saturating_sub(MAX_ROWS),
            grid.columns().min(MAX_COLUMNS),
        );
        drop(term);
        let screen = rows
            .iter()
            .map(|row| row.trim_end())
            .collect::<Vec<_>>()
            .join("\n");
        if let Some(sample) = self
            .sample
            .as_mut()
            .filter(|sample| sample.hint.epoch == epoch && sample.text == screen)
        {
            sample.grid_revision = revision;
            sample.refresh(terminal, session.mid_turn(), now);
            return;
        }
        let Some(evidence) = detect_screen(manifest, &screen) else {
            return;
        };
        let hint = AgentScreenHint {
            epoch,
            revision,
            rule: evidence.rule_id.map(str::to_string),
        };
        terminal.report_agent_screen(hint.clone());
        self.sample = Some(Sample {
            hint,
            grid_revision: revision,
            text: screen,
            sent: now,
            actionable: matches!(
                evidence.state,
                AgentScreenState::Blocked | AgentScreenState::Working
            ),
            idle: evidence.state == AgentScreenState::Idle,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_daemon_status_does_not_enable_screen_reports() {
        let old: AgentSessionState =
            serde_json::from_str(r#"{"status":"working","rich":true}"#).unwrap();
        assert_eq!(manifest_epoch(&old), None);
    }
}
