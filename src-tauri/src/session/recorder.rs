//! 录音状态机。
//!
//! 替代现有的 `bool is_recording` 标志，提供更精细的状态管理。
//! 第七阶段将把 `app_state.rs` 中的状态切换迁移到该状态机。

use std::fmt;

/// 录音状态机。
///
/// 状态转移规则：
/// ```text
/// Idle ──start──▶ Listening ──stop──▶ Processing ──done──▶ Idle
///   │                │                   │
///   │                └──cancel──▶ Idle   └──error──▶ Error ──reset──▶ Idle
///   │
///   └──(自动监听模式)──▶ Listening
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecorderState {
    /// 空闲，未在录音。
    Idle,
    /// 正在监听麦克风（录音中）。
    Listening,
    /// 录音已停止，正在识别处理。
    Processing,
    /// 发生错误。
    Error(String),
}

impl RecorderState {
    /// 是否允许从当前状态转移到目标状态。
    pub fn can_transition_to(&self, target: &RecorderState) -> bool {
        use RecorderState::*;
        match (self, target) {
            // Idle 可触发录音或保持
            (Idle, Listening) => true,
            (Idle, Idle) => true,
            // Listening 可停止进入处理，或取消回 Idle
            (Listening, Processing) => true,
            (Listening, Idle) => true,
            // Processing 完成回 Idle，或出错
            (Processing, Idle) => true,
            (Processing, Error(_)) => true,
            // Error 可重置回 Idle
            (Error(_), Idle) => true,
            _ => false,
        }
    }

    /// 是否处于活动状态（正在录音或处理中）。
    pub fn is_active(&self) -> bool {
        matches!(self, RecorderState::Listening | RecorderState::Processing)
    }

    /// 是否正在录音。
    pub fn is_listening(&self) -> bool {
        matches!(self, RecorderState::Listening)
    }
}

impl fmt::Display for RecorderState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecorderState::Idle => write!(f, "idle"),
            RecorderState::Listening => write!(f, "listening"),
            RecorderState::Processing => write!(f, "processing"),
            RecorderState::Error(e) => write!(f, "error:{}", e),
        }
    }
}

impl Default for RecorderState {
    fn default() -> Self {
        RecorderState::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_can_start_listening() {
        assert!(RecorderState::Idle.can_transition_to(&RecorderState::Listening));
    }

    #[test]
    fn listening_can_stop_to_processing() {
        assert!(RecorderState::Listening.can_transition_to(&RecorderState::Processing));
    }

    #[test]
    fn listening_can_cancel_to_idle() {
        assert!(RecorderState::Listening.can_transition_to(&RecorderState::Idle));
    }

    #[test]
    fn idle_cannot_go_to_processing_directly() {
        assert!(!RecorderState::Idle.can_transition_to(&RecorderState::Processing));
    }

    #[test]
    fn is_active_for_listening_and_processing() {
        assert!(RecorderState::Listening.is_active());
        assert!(RecorderState::Processing.is_active());
        assert!(!RecorderState::Idle.is_active());
    }
}
