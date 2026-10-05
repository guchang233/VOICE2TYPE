//! Native, non-activating status overlay. Timing and drawing are independent of Win32.
mod model;
#[cfg(target_os = "windows")]
mod native;
#[cfg(any(target_os = "windows", test))]
mod render;

use crate::config::{AppConfig, DICTATION_MODE_STREAM};
pub use model::IndicatorState;
use model::Settings;

#[derive(Clone)]
pub struct StatusIndicator {
    #[cfg(target_os = "windows")]
    sender: Option<native::IndicatorSender>,
}

impl StatusIndicator {
    pub fn new(config: &AppConfig) -> Self {
        #[cfg(target_os = "windows")]
        {
            let sender = match native::start(settings(config)) {
                Ok(sender) => Some(sender),
                Err(error) => {
                    log::error!("状态浮层启动失败: {error}");
                    None
                }
            };
            Self { sender }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = config;
            Self {}
        }
    }

    pub fn set_state(&self, state: IndicatorState) {
        #[cfg(target_os = "windows")]
        if let Some(sender) = &self.sender {
            sender.send(native::Command::State(state));
        }
        #[cfg(not(target_os = "windows"))]
        let _ = state;
    }

    /// Apply theme, visibility and timing changes without restarting the application.
    pub fn configure(&self, config: &AppConfig) {
        #[cfg(target_os = "windows")]
        if let Some(sender) = &self.sender {
            sender.send(native::Command::Configure(settings(config)));
        }
        #[cfg(not(target_os = "windows"))]
        let _ = config;
    }
}

fn settings(config: &AppConfig) -> Settings {
    Settings {
        enabled: config.features.enable_indicator
            && (config.basic.dictation_mode != DICTATION_MODE_STREAM
                || config.streaming.enable_indicator),
        theme: config.theme.clone(),
        fade: std::time::Duration::from_millis(config.indicator.fade_duration.min(10_000)),
        success: std::time::Duration::from_millis(config.indicator.success_duration),
        error: std::time::Duration::from_millis(config.indicator.error_duration),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibility_respects_global_and_streaming_switches() {
        let mut config = AppConfig::default();
        config.basic.dictation_mode = DICTATION_MODE_STREAM.into();
        config.streaming.enable_indicator = false;
        assert!(!settings(&config).enabled);
        config.basic.dictation_mode = "batch".into();
        assert!(settings(&config).enabled);
        config.features.enable_indicator = false;
        assert!(!settings(&config).enabled);
    }
}
