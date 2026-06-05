/// 流式语音识别运行态配置，在每次会话启动时从 ConfigManager 一次性读取快照。
pub struct StreamingConfig {
    pub app_key: String,
    pub access_key: String,
    pub resource_id: String,
    pub max_secs: u32,
    pub output_mode: String,
}

impl StreamingConfig {
    pub fn from_manager(mgr: &crate::config::ConfigManager) -> Self {
        Self {
            app_key: mgr.get_doubao_app_key(),
            access_key: mgr.get_doubao_api_key(),
            resource_id: mgr.get_doubao_resource_id(),
            max_secs: mgr.streaming_max_secs(),
            output_mode: mgr.streaming_output_mode(),
        }
    }
}