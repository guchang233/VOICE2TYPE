/// 应用状态
#[derive(Debug, Clone, PartialEq)]
pub enum AppState {
    Idle,
    Recording,
    Processing,
    Cancelled, // 取消状态
}