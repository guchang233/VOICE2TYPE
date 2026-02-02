# Voice2Type

极简高效的 Windows 语音输入助手
常驻后台，**极低占用**，
## ✏️ 用法
```
`选中`目标输入框，按住 `F2` 说话，松开 `F2` 后即文字将直接注入输入框。
```
```
若想取消，在按住 `F2` 时按下 `ESC` 即可。
```
```
那里有免费、功能强大的语音识别api
```
## ✨ 特性

*   **轻量**：Rust + Tokio 编写，资源占用极低。
*   **隐私**：音频全程内存处理，零磁盘写入。
*   **准确**：接入 SiliconFlow (SenseVoiceSmall) API。
*   **隐形**：无主窗口，仅托盘图标，不占任务栏。

## � 快速开始

1.  **下载**：从 [Releases](../../releases) 下载最新 `voice2type.exe`。
2.  **配置**：
    *   运行程序，右键托盘图标 -> **配置** -> **API Key**。
    *   填入你的 [SiliconFlow API Key](https://cloud.siliconflow.cn/)
## 🛠️ 开发

需要 Rust 环境和 Visual Studio Build Tools (C++)。

```powershell
# 开发运行
cargo run

# 发布编译
cargo build --release
```

## 📄 License

MIT License
