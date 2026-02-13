/// 应用状态
#[derive(Debug, Clone, PartialEq)]
pub enum AppState {
    Idle,
    Recording,
    Streaming, // 新增：流式处理状态
    Processing,
    Cancelled, // 新增：取消状态
}

/// 流式处理状态
#[derive(Debug, Clone)]
pub struct StreamingState {
    pub current_chunk: Vec<f32>,
    pub chunk_count: usize,
    pub last_result: String,
    pub output_sent: bool,
    // API调用节流相关字段
    pub processing: bool, // 是否有正在进行的API调用
}

impl StreamingState {
    /// 创建新的流式处理状态
    pub fn new() -> Self {
        // 合理设置初始容量，避免过度分配
        let initial_chunk_capacity = 16000; // 1秒音频 @ 16kHz
        
        Self {
            current_chunk: Vec::with_capacity(initial_chunk_capacity),
            chunk_count: 0,
            last_result: String::new(),
            output_sent: false,
            // API调用节流默认参数
            processing: false,
        }
    }
    
    /// 更新最后结果
    pub fn update_last_result(&mut self, result: &str) {
        self.last_result = result.to_string();
    }
    
    /// 重置流式处理状态
    pub fn reset(&mut self) {
        self.current_chunk.clear();
        self.chunk_count = 0;
        self.last_result.clear();
        self.output_sent = false;
        self.processing = false;
    }
}