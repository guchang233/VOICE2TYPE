# Voice2Type

Windows 托盘语音输入工具：按住热键说话，松开后自动转写并输入到当前光标位置。

## 核心功能

- 全局热键录音，支持按住说话和点击切换两种模式
- 剪贴板粘贴 / 键盘注入两种输出方式
- SiliconFlow / Groq / 本地 Whisper 三种转写引擎
- 可选过滤标点和表情符号
- 支持重新粘贴上一条结果
- 开机自启、更新检查、日志窗口

## 快速开始

1. 启动后在托盘图标打开 **配置 → API Key**，填写对应 Key。
2. 选中目标输入框，按住 `F2` 说话，松开后自动输入。
3. 输出方式优先选 **剪贴板粘贴**，兼容性更好。

## 本地 Whisper（可选）

1. 托盘 **设置 → 配置 → 设置本地 Whisper 目录**，选择一个空文件夹。
2. 将 `whisper-cli.exe` 放入 `bin/`，模型放入 `models/`。
3. 在模型选择中切换为 **本地 Whisper（离线）**。

无需 API Key，转写在本地执行。

## 配置文件

优先读取当前目录 `voice2type_config.json`，否则使用系统配置目录 `settings.json`。日志位于配置目录 `logs/app.log`。

## 开发

```powershell
cargo fmt
cargo check
cargo run
```

发布构建：

```powershell
cargo build --release
```

管理员场景：

```powershell
cargo run --features admin
```
