//! 豆包大模型流式 ASR WebSocket 客户端

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async,
    tungstenite::client::IntoClientRequest,
    tungstenite::Message,
};
use uuid::Uuid;

use crate::config::ConfigManager;
use crate::streaming::protocol::{self, AsrResponse};

const WS_URL: &str = "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel";

pub struct StreamingAsrClient {
    write_tx: mpsc::Sender<Message>,
}

impl Clone for StreamingAsrClient {
    fn clone(&self) -> Self {
        Self {
            write_tx: self.write_tx.clone(),
        }
    }
}

impl StreamingAsrClient {
    pub async fn connect(
        config: Arc<ConfigManager>,
        result_tx: mpsc::Sender<Result<AsrResponse>>,
    ) -> Result<(Self, tokio::task::JoinHandle<()>)> {
        let api_key = config.get_doubao_api_key();
        if api_key.is_empty() {
            anyhow::bail!("豆包 API Key 未配置，请在「模型与密钥」中填写");
        }

        let resource_id = config.streaming_resource_id();
        let request_id = Uuid::new_v4().to_string();
        let connect_id = Uuid::new_v4().to_string();

        let mut request = WS_URL.into_client_request().context("build ws request")?;
        let headers = request.headers_mut();
        headers.insert(
            "X-Api-Key",
            api_key.parse().context("invalid api key header")?,
        );
        headers.insert(
            "X-Api-Resource-Id",
            resource_id.parse().context("invalid resource id")?,
        );
        headers.insert(
            "X-Api-Request-Id",
            request_id.parse().context("invalid request id")?,
        );
        headers.insert(
            "X-Api-Connect-Id",
            connect_id.parse().context("invalid connect id")?,
        );

        let (ws, _) = connect_async(request).await.context("websocket connect")?;

        let (mut sink, mut stream) = ws.split();

        let lang = map_output_language(config.streaming_output_language());
        let json = protocol::build_start_json(
            &config.streaming_model_name(),
            lang.as_deref(),
            config.streaming_allow_punctuation(),
        )?;
        let start_frame = protocol::encode_full_client_request(&json)?;
        sink.send(Message::Binary(start_frame))
            .await
            .context("send full client request")?;

        let (write_tx, mut write_rx) = mpsc::channel::<Message>(64);
        let writer = tokio::spawn(async move {
            while let Some(msg) = write_rx.recv().await {
                if sink.send(msg).await.is_err() {
                    break;
                }
            }
        });

        let read_task = tokio::spawn(async move {
            while let Some(msg) = stream.next().await {
                match msg {
                    Ok(Message::Binary(data)) => {
                        match protocol::decode_server_message(&data) {
                            Ok(Some(resp)) => {
                                let _ = result_tx.send(Ok(resp)).await;
                            }
                            Ok(None) => {}
                            Err(e) => {
                                let _ = result_tx.send(Err(e)).await;
                            }
                        }
                    }
                    Ok(Message::Close(_)) => break,
                    Err(e) => {
                        let _ = result_tx
                            .send(Err(anyhow::anyhow!("websocket read: {}", e)))
                            .await;
                        break;
                    }
                    _ => {}
                }
            }
        });

        let handle = tokio::spawn(async move {
            let _ = read_task.await;
            writer.abort();
        });

        Ok((Self { write_tx }, handle))
    }

    pub async fn send_audio(&self, pcm: Vec<u8>, sequence: i32, is_last: bool) -> Result<()> {
        let frame = protocol::encode_audio_chunk(&pcm, sequence, is_last)?;
        self.write_tx
            .send(Message::Binary(frame))
            .await
            .context("queue audio frame")?;
        Ok(())
    }

    pub async fn close(&self) {
        let _ = self.write_tx.send(Message::Close(None)).await;
    }
}

fn map_output_language(lang: String) -> Option<String> {
    match lang.as_str() {
        "" | "auto" => None,
        "zh" => Some("zh-CN".to_string()),
        "en" => Some("en-US".to_string()),
        other => Some(other.to_string()),
    }
}
