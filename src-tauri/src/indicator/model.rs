use std::time::{Duration, Instant};

pub const FRAME: Duration = Duration::from_millis(16);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndicatorState {
    Hidden,
    Recording,
    Processing,
    Success,
    Error,
    Cancelled,
}

impl IndicatorState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Hidden => "",
            Self::Recording => "聆听中",
            Self::Processing => "处理中",
            Self::Success => "已输出",
            Self::Error => "识别失败",
            Self::Cancelled => "已取消",
        }
    }
    pub fn active(self) -> bool {
        matches!(self, Self::Recording | Self::Processing)
    }
}

#[derive(Clone)]
pub struct Settings {
    pub enabled: bool,
    pub theme: String,
    pub fade: Duration,
    pub success: Duration,
    pub error: Duration,
}

/// Absolute-time animation avoids changing speed when commands arrive between frames.
pub struct Model {
    pub settings: Settings,
    pub state: IndicatorState,
    pub displayed: IndicatorState,
    pub alpha: f32,
    entered: Instant,
    fade_started: Instant,
    fade_from: f32,
    fade_to: f32,
}

impl Model {
    pub fn new(settings: Settings, now: Instant) -> Self {
        Self {
            settings,
            state: IndicatorState::Hidden,
            displayed: IndicatorState::Hidden,
            alpha: 0.0,
            entered: now,
            fade_started: now,
            fade_from: 0.0,
            fade_to: 0.0,
        }
    }

    pub fn set_state(&mut self, state: IndicatorState, now: Instant) {
        // A repeated result renews its lifetime; repeated active states keep their animation.
        if self.state == state && state.active() {
            return;
        }
        self.advance_fade(now);
        self.state = state;
        self.entered = now;
        if state != IndicatorState::Hidden {
            self.displayed = state;
        }
        self.fade_to(
            if self.settings.enabled && state != IndicatorState::Hidden {
                1.0
            } else {
                0.0
            },
            now,
        );
    }

    pub fn configure(&mut self, settings: Settings, now: Instant) {
        self.advance_fade(now);
        self.settings = settings;
        self.fade_from = self.alpha;
        self.fade_started = now;
        if !self.settings.enabled {
            // Disabling takes effect immediately, including during recording.
            self.alpha = 0.0;
            self.fade_from = 0.0;
            self.fade_to = 0.0;
        } else {
            self.fade_to(
                if self.state == IndicatorState::Hidden {
                    0.0
                } else {
                    1.0
                },
                now,
            );
        }
        self.tick(now);
    }

    pub fn tick(&mut self, now: Instant) -> bool {
        let previous = self.alpha;
        let expired = self
            .lifetime()
            .is_some_and(|duration| self.elapsed(now) >= duration);
        if expired {
            self.set_state(IndicatorState::Hidden, now);
        }
        self.advance_fade(now);
        previous != self.alpha || expired
    }

    pub fn elapsed(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.entered)
    }

    /// None means wait for a command or native message, with no idle polling.
    pub fn wait(&self, now: Instant) -> Option<Duration> {
        if self.alpha != self.fade_to || (self.settings.enabled && self.state.active()) {
            return Some(FRAME);
        }
        self.lifetime()
            .map(|duration| duration.saturating_sub(self.elapsed(now)))
    }

    fn lifetime(&self) -> Option<Duration> {
        match self.state {
            IndicatorState::Success => Some(self.settings.success),
            IndicatorState::Error => Some(self.settings.error),
            IndicatorState::Cancelled => Some(Duration::from_millis(1200)),
            _ => None,
        }
    }

    fn fade_to(&mut self, target: f32, now: Instant) {
        if self.fade_to != target {
            self.fade_from = self.alpha;
            self.fade_to = target;
            self.fade_started = now;
        }
        self.advance_fade(now);
    }

    fn advance_fade(&mut self, now: Instant) {
        let progress = if self.settings.fade.is_zero() {
            1.0
        } else {
            (now.saturating_duration_since(self.fade_started)
                .as_secs_f32()
                / self.settings.fade.as_secs_f32())
            .clamp(0.0, 1.0)
        };
        let eased = 1.0 - (1.0 - progress).powi(3);
        self.alpha = if progress >= 1.0 {
            self.fade_to
        } else {
            self.fade_from + (self.fade_to - self.fade_from) * eased
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn settings() -> Settings {
        Settings {
            enabled: true,
            theme: "dark".into(),
            fade: Duration::from_millis(300),
            success: Duration::from_millis(1500),
            error: Duration::from_millis(3000),
        }
    }

    #[test]
    fn cancellation_expires_and_final_fade_frame_is_drawn() {
        let now = Instant::now();
        let mut model = Model::new(settings(), now);
        model.set_state(IndicatorState::Cancelled, now);
        model.tick(now + Duration::from_millis(300));
        assert_eq!(model.alpha, 1.0);
        assert!(model.tick(now + Duration::from_millis(1200)));
        assert_eq!(model.displayed, IndicatorState::Cancelled);
        assert!(model.tick(now + Duration::from_millis(1500)));
        assert_eq!(model.alpha, 0.0);
        assert_eq!(model.wait(now + Duration::from_millis(1500)), None);
    }

    #[test]
    fn repeated_success_renews_deadline() {
        let now = Instant::now();
        let mut model = Model::new(settings(), now);
        model.set_state(IndicatorState::Success, now);
        model.set_state(IndicatorState::Success, now + Duration::from_secs(1));
        model.tick(now + Duration::from_secs(2));
        assert_eq!(model.state, IndicatorState::Success);
        model.tick(now + Duration::from_millis(2500));
        assert_eq!(model.state, IndicatorState::Hidden);
    }

    #[test]
    fn fade_duration_is_independent_of_frame_rate() {
        let now = Instant::now();
        let mut fast = Model::new(settings(), now);
        let mut slow = Model::new(settings(), now);
        fast.set_state(IndicatorState::Recording, now);
        slow.set_state(IndicatorState::Recording, now);
        for ms in 1..=150 {
            fast.tick(now + Duration::from_millis(ms));
        }
        slow.tick(now + Duration::from_millis(150));
        assert_eq!(fast.alpha, slow.alpha);
        assert_eq!(fast.alpha, 0.875);
    }

    #[test]
    fn disabled_indicator_stays_hidden_and_can_resume_recording() {
        let now = Instant::now();
        let mut disabled = settings();
        disabled.enabled = false;
        let mut model = Model::new(disabled.clone(), now);
        model.set_state(IndicatorState::Recording, now);
        model.tick(now + Duration::from_secs(1));
        assert_eq!(model.alpha, 0.0);
        assert_eq!(model.wait(now), None);
        model.configure(settings(), now + Duration::from_secs(1));
        model.tick(now + Duration::from_millis(1300));
        assert_eq!(model.alpha, 1.0);
        model.configure(disabled, now + Duration::from_millis(1300));
        assert_eq!(model.alpha, 0.0);
    }

    #[test]
    fn zero_durations_and_extreme_deadlines_are_safe() {
        let now = Instant::now();
        let mut config = settings();
        config.fade = Duration::ZERO;
        config.success = Duration::ZERO;
        config.error = Duration::from_millis(u64::MAX);
        let mut model = Model::new(config, now);
        model.set_state(IndicatorState::Success, now);
        model.tick(now);
        assert_eq!(model.alpha, 0.0);
        model.set_state(IndicatorState::Error, now);
        model.tick(now);
        assert_eq!(model.alpha, 1.0);
        assert!(model.wait(now).unwrap() > Duration::from_secs(1));
    }

    #[test]
    fn new_recording_interrupts_fade_without_stale_auto_hide() {
        let now = Instant::now();
        let mut model = Model::new(settings(), now);
        model.set_state(IndicatorState::Error, now);
        model.tick(now + Duration::from_secs(3));
        model.set_state(IndicatorState::Recording, now + Duration::from_millis(3100));
        model.tick(now + Duration::from_secs(10));
        assert_eq!(model.state, IndicatorState::Recording);
        assert_eq!(model.alpha, 1.0);
    }

    #[test]
    fn changing_theme_or_duration_preserves_in_flight_opacity() {
        let now = Instant::now();
        let mut model = Model::new(settings(), now);
        model.set_state(IndicatorState::Recording, now);
        model.tick(now + Duration::from_millis(150));
        let previous = model.alpha;
        let mut updated = settings();
        updated.theme = "light".into();
        updated.fade = Duration::from_millis(500);
        model.configure(updated, now + Duration::from_millis(150));
        assert_eq!(model.alpha, previous);
        model.tick(now + Duration::from_millis(650));
        assert_eq!(model.alpha, 1.0);
    }
}
