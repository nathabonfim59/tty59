use std::time::{Duration, Instant};

use tty59_agent_detect::{Detection, State};

use crate::core::cli_agent::{AgentSessionState, AgentStatus, AgentStatusSource};

const STARTUP_GRACE: Duration = Duration::from_secs(3);
const HOOK_GRACE: Duration = Duration::from_secs(1);
const SCREEN_LIFETIME: Duration = Duration::from_secs(2);
const PLAIN_IDLE_CAP: Duration = Duration::from_millis(700);

#[derive(Default)]
pub(super) struct Detector {
    pub started: Option<Instant>,
    pub title: String,
    pub progress: String,
    screen: Option<ScreenSample>,
    pending_idle: Option<(Instant, u8)>,
    pub pending_osc_idle: Option<(Detection<'static>, Instant)>,
    osc_working: bool,
    pub last_working: Option<Instant>,
}

struct ScreenSample {
    writer: Writer,
    revision: u64,
    observed: Instant,
    received: Instant,
    detection: Detection<'static>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Writer {
    Controller(u64),
    Observer(u64),
}

impl Detector {
    pub fn start(&mut self, now: Instant) {
        *self = Self {
            started: Some(now),
            ..Self::default()
        };
    }

    pub fn screen_recent(&self, now: Instant) -> bool {
        self.screen
            .as_ref()
            .is_some_and(|screen| now.saturating_duration_since(screen.received) < SCREEN_LIFETIME)
    }

    pub fn accept_screen(
        &mut self,
        writer: Writer,
        revision: u64,
        detection: Detection<'static>,
        now: Instant,
    ) -> Option<Instant> {
        if self
            .started
            .is_some_and(|started| now.saturating_duration_since(started) < STARTUP_GRACE)
        {
            return None;
        }
        let mut observed = now;
        if let Some(previous) = &self.screen {
            if previous.writer == writer {
                if revision < previous.revision
                    || (revision == previous.revision && detection != previous.detection)
                {
                    return None;
                }
                if revision == previous.revision {
                    observed = previous.observed;
                }
            } else if matches!(previous.writer, Writer::Controller(_))
                && matches!(writer, Writer::Observer(_))
                && self.screen_recent(now)
            {
                return None;
            }
        }
        self.screen = Some(ScreenSample {
            writer,
            revision,
            observed,
            received: now,
            detection,
        });
        Some(observed)
    }

    pub fn screen_overrides_osc(&self, now: Instant) -> bool {
        self.screen_recent(now)
            && self.screen.as_ref().is_some_and(|screen| {
                screen.detection.skip_state_update || screen.detection.visible_blocker
            })
    }

    pub fn note_osc(&mut self, evidence: Option<Detection<'static>>, now: Instant) {
        self.osc_working = evidence.is_some_and(|evidence| evidence.state == State::Working);
        self.pending_osc_idle = evidence
            .filter(|evidence| evidence.visible_idle)
            .map(|evidence| (evidence, now));
    }

    /// Apply state evidence in precedence order:
    ///
    /// | Evidence | Effect |
    /// | --- | --- |
    /// | Visible blocker | Set Waiting even if a hook reported Working. |
    /// | Fresh hook | Do not complete the turn from idle evidence. |
    /// | Visible idle prompt | Finish a mid-turn session if the prompt is at least as recent as its last hook. |
    /// | Plain idle | Debounce before completing a hookless turn or a screen-derived wait. |
    ///
    /// Transcript overlays keep the previous state. Working evidence starts a
    /// hookless turn or releases a screen-derived wait when activity is visible.
    pub fn apply(
        &mut self,
        session: &mut AgentSessionState,
        evidence: Detection<'static>,
        source: AgentStatusSource,
        observed: Instant,
        now: Instant,
        last_hook: Option<Instant>,
    ) {
        if evidence.skip_state_update || evidence.state == State::Unknown {
            self.pending_idle = None;
            return;
        }
        if evidence.state == State::Working {
            self.last_working = Some(now);
        }
        let hook_is_fresh =
            last_hook.is_some_and(|hook| now.saturating_duration_since(hook) < HOOK_GRACE);
        let screen_waiting = session.status == AgentStatus::Waiting && session.observed();
        let status = match evidence.state {
            State::Blocked if evidence.visible_blocker || !session.rich => {
                Some(AgentStatus::Waiting)
            }
            State::Working if !session.rich || (screen_waiting && evidence.visible_working) => {
                Some(AgentStatus::Working)
            }
            State::Idle
                if session.mid_turn()
                    && evidence.visible_idle
                    && !hook_is_fresh
                    && last_hook.is_none_or(|hook| observed >= hook) =>
            {
                Some(AgentStatus::Done)
            }
            State::Idle
                if session.mid_turn()
                    && source == AgentStatusSource::Screen
                    && !self.osc_working
                    && (!session.rich || screen_waiting && !hook_is_fresh) =>
            {
                let (first, samples) = self.pending_idle.get_or_insert((now, 0));
                *samples = samples.saturating_add(1);
                if *samples < 3 && now.saturating_duration_since(*first) < PLAIN_IDLE_CAP {
                    return;
                }
                Some(AgentStatus::Done)
            }
            _ => None,
        };
        self.pending_idle = None;
        if let Some(status) = status
            && session.status != status
        {
            if status == AgentStatus::Done {
                session.turns = session.turns.wrapping_add(1);
            }
            session.status = status;
            session.message = None;
            session.inferred = false;
            if let Some(detection) = &mut session.detection {
                detection.source = Some(source);
                detection.rule = evidence.rule_id.map(str::to_string);
            }
        }
    }
}
