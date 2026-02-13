use anyhow::Result;
use std::sync::Arc;

use crate::config::ConfigManager;

/// 语音识别服务接口
pub trait SpeechService {
    /// 识别音频数据
    async fn recognize(&self, audio_data: &[u8], config: &Arc<ConfigManager>) -> Result<String>;
    
    /// 获取服务名称
    fn name(&self) -> &str;
    
    /// 检查服务是否可用
    async fn is_available(&self, config: &Arc<ConfigManager>) -> bool;
}

/// 语音识别服务类型
pub enum SpeechServiceType {
    /// SiliconFlow
    SiliconFlow,
    /// OpenAI
    OpenAI,
    /// Google Cloud Speech-to-Text
    GoogleCloud,
    /// Azure Speech Services
    Azure,
    /// 百度语音识别
    Baidu,
    /// 阿里云语音识别
    Alibaba,
    /// 腾讯云语音识别
    Tencent,
}

impl SpeechServiceType {
    /// 从字符串创建服务类型
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "siliconflow" => Some(Self::SiliconFlow),
            "openai" => Some(Self::OpenAI),
            "google" => Some(Self::GoogleCloud),
            "azure" => Some(Self::Azure),
            "baidu" => Some(Self::Baidu),
            "alibaba" => Some(Self::Alibaba),
            "tencent" => Some(Self::Tencent),
            _ => None,
        }
    }
    
    /// 转换为字符串
    pub fn to_str(&self) -> &str {
        match self {
            Self::SiliconFlow => "siliconflow",
            Self::OpenAI => "openai",
            Self::GoogleCloud => "google",
            Self::Azure => "azure",
            Self::Baidu => "baidu",
            Self::Alibaba => "alibaba",
            Self::Tencent => "tencent",
        }
    }
}

/// 语音识别服务工厂
pub struct SpeechServiceFactory;

impl SpeechServiceFactory {
    /// 创建语音识别服务
    pub fn create(service_type: SpeechServiceType) -> Box<dyn SpeechService> {
        match service_type {
            SpeechServiceType::SiliconFlow => Box::new(crate::speech::services::siliconflow::SiliconFlowService::new()),
            SpeechServiceType::OpenAI => Box::new(crate::speech::services::openai::OpenAIService::new()),
            SpeechServiceType::GoogleCloud => Box::new(crate::speech::services::google::GoogleCloudService::new()),
            SpeechServiceType::Azure => Box::new(crate::speech::services::azure::AzureService::new()),
            SpeechServiceType::Baidu => Box::new(crate::speech::services::baidu::BaiduService::new()),
            SpeechServiceType::Alibaba => Box::new(crate::speech::services::alibaba::AlibabaService::new()),
            SpeechServiceType::Tencent => Box::new(crate::speech::services::tencent::TencentService::new()),
        }
    }
    
    /// 根据配置创建语音识别服务
    pub fn create_from_config(config: &Arc<ConfigManager>) -> Box<dyn SpeechService> {
        let service_type = config.get_speech_service();
        match SpeechServiceType::from_str(&service_type) {
            Some(service_type) => Self::create(service_type),
            None => Self::create(SpeechServiceType::SiliconFlow), // 默认使用SiliconFlow
        }
    }
}
