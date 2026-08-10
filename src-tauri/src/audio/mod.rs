//! 音频处理模块。
//!
//! 包含采集、缓冲、重采样三个子模块：
//! - [`buffer`]：音频缓冲抽象（第二阶段实现 lock-free）
//! - [`capture`]：音频采集抽象
//! - [`processor`]：现有重采样与 WAV 编码实现（保留，向后兼容）
//! - [`resample`]：重采样入口（重导出 processor 中的函数）

pub mod buffer;
pub mod capture;
pub mod processor;
pub mod resample;
