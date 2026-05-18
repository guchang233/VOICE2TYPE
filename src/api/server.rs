use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::config::ConfigManager;
use crate::core::state::AppState;
use crate::utils::logger::{write_log, LogLevel};

/// API请求类型
#[derive(Debug, Deserialize)]
pub enum ApiRequest {
    /// 开始录音
    StartRecording,
    /// 停止录音
    StopRecording,
    /// 取消录音
    CancelRecording,
    /// 获取当前状态
    GetState,
    /// 设置配置
    SetConfig { key: String, value: String },
    /// 获取配置
    GetConfig { key: String },
}

/// API响应类型
#[derive(Debug, Serialize)]
pub enum ApiResponse {
    /// 成功
    Success { message: String },
    /// 错误
    Error { message: String },
    /// 状态响应
    StateResponse { state: String },
    /// 配置响应
    ConfigResponse { value: String },
}

/// API服务器
pub struct ApiServer {
    config: Arc<ConfigManager>,
    tx: mpsc::Sender<ApiRequest>,
}

impl ApiServer {
    /// 创建新的API服务器
    pub fn new(config: Arc<ConfigManager>) -> Self {
        let (tx, rx) = mpsc::channel(100);

        // 启动处理线程
        Self::start_processing_thread(rx, config.clone());

        Self { config, tx }
    }

    /// 启动处理线程
    fn start_processing_thread(mut rx: mpsc::Receiver<ApiRequest>, config: Arc<ConfigManager>) {
        thread::spawn(move || {
            // 这里可以处理API请求
            while let Some(request) = rx.blocking_recv() {
                match request {
                    ApiRequest::StartRecording => {
                        // 处理开始录音请求
                        write_log(LogLevel::INFO, "API: 收到开始录音请求", Some(&config));
                    }
                    ApiRequest::StopRecording => {
                        // 处理停止录音请求
                        write_log(LogLevel::INFO, "API: 收到停止录音请求", Some(&config));
                    }
                    ApiRequest::CancelRecording => {
                        // 处理取消录音请求
                        write_log(LogLevel::INFO, "API: 收到取消录音请求", Some(&config));
                    }
                    ApiRequest::GetState => {
                        // 处理获取状态请求
                        write_log(LogLevel::INFO, "API: 收到获取状态请求", Some(&config));
                    }
                    ApiRequest::SetConfig { key, value } => {
                        // 处理设置配置请求
                        write_log(
                            LogLevel::INFO,
                            &format!("API: 收到设置配置请求: {} = {}", key, value),
                            Some(&config),
                        );
                    }
                    ApiRequest::GetConfig { key } => {
                        // 处理获取配置请求
                        write_log(
                            LogLevel::INFO,
                            &format!("API: 收到获取配置请求: {}", key),
                            Some(&config),
                        );
                    }
                }
            }
        });
    }

    /// 启动服务器
    pub fn start(&self) {
        let config = self.config.clone();
        let tx = self.tx.clone();

        thread::spawn(move || {
            let listener = TcpListener::bind("127.0.0.1:8080").unwrap();
            write_log(
                LogLevel::INFO,
                "API服务器已启动，监听端口 8080",
                Some(&config),
            );

            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        let tx = tx.clone();
                        let config = config.clone();
                        thread::spawn(move || {
                            Self::handle_connection(stream, tx, config);
                        });
                    }
                    Err(e) => {
                        write_log(
                            LogLevel::ERROR,
                            &format!("API服务器错误: {}", e),
                            Some(&config),
                        );
                    }
                }
            }
        });
    }

    /// 处理连接
    fn handle_connection(
        mut stream: TcpStream,
        tx: mpsc::Sender<ApiRequest>,
        config: Arc<ConfigManager>,
    ) {
        let mut buffer = [0; 1024];

        match stream.read(&mut buffer) {
            Ok(size) => {
                let message = String::from_utf8_lossy(&buffer[..size]);
                write_log(
                    LogLevel::INFO,
                    &format!("API服务器收到请求: {}", message),
                    Some(&config),
                );

                // 解析请求
                match serde_json::from_str::<ApiRequest>(&message) {
                    Ok(request) => {
                        // 发送请求到处理线程
                        if let Err(e) = tx.blocking_send(request) {
                            write_log(
                                LogLevel::ERROR,
                                &format!("API服务器错误: {}", e),
                                Some(&config),
                            );
                            let response = ApiResponse::Error {
                                message: e.to_string(),
                            };
                            Self::send_response(&mut stream, response);
                        } else {
                            let response = ApiResponse::Success {
                                message: "请求已接收".to_string(),
                            };
                            Self::send_response(&mut stream, response);
                        }
                    }
                    Err(e) => {
                        write_log(
                            LogLevel::ERROR,
                            &format!("API服务器错误: {}", e),
                            Some(&config),
                        );
                        let response = ApiResponse::Error {
                            message: e.to_string(),
                        };
                        Self::send_response(&mut stream, response);
                    }
                }
            }
            Err(e) => {
                write_log(
                    LogLevel::ERROR,
                    &format!("API服务器错误: {}", e),
                    Some(&config),
                );
                let response = ApiResponse::Error {
                    message: e.to_string(),
                };
                Self::send_response(&mut stream, response);
            }
        }
    }

    /// 发送响应
    fn send_response(stream: &mut TcpStream, response: ApiResponse) {
        let json = serde_json::to_string(&response).unwrap();
        stream.write_all(json.as_bytes()).unwrap();
        stream.write_all(b"\n").unwrap();
    }
}
