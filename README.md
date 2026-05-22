# Voice2Type

Voice2Type 是一个 Windows 托盘语音输入工具：按下热键录音，松开后自动转写并输入到当前光标位置。

## 主要功能

- 全局热键录音，支持按住说话和按一下开始/结束两种模式
- 支持剪贴板粘贴和键盘注入两种输出方式
- 支持 SiliconFlow / Groq / 自定义 API 转写接口
- 可选本地 Whisper（whisper.cpp），离线转写，根目录由用户自行选择
- 可保留或过滤标点、表情符号
- 托盘菜单配置 API Key、模型、热键、麦克风、状态浮窗和日志
- 转写进行中自动阻止重复录音，避免重叠输入
- 支持重新粘贴上一条识别结果
- 支持开机自动启动、更新检查和日志窗口

## 使用方式

1. 启动程序后，在托盘图标中打开 `配置 -> API Key`。
2. 按所选模型填写对应 API Key。
3. 选中目标输入框，按住默认热键 `F2` 说话。
4. 松开热键后，识别结果会自动输入到目标窗口。

## 推荐设置

- 输出方式优先使用 `剪贴板粘贴（推荐）`，兼容性通常更好。
- 若识别结果未粘贴到目标窗口，可用托盘「重新粘贴上一条」恢复。
- 如果要在管理员权限程序或游戏中输入，建议以管理员权限启动 Voice2Type。

## 开发

```powershell
cargo fmt
cargo check
cargo run
```

开发调试默认不要求管理员（manifest 为 `asInvoker`）。发布构建或需要测试管理员场景时使用：

```powershell
cargo build --release
cargo run --features admin
```

发布构建：

```powershell
cargo build --release
```

## 配置文件

程序优先读取当前目录下的 `voice2type_config.json`，否则使用系统配置目录中的 `settings.json`。日志文件位于配置目录下的 `logs/app.log`。

## 本地 Whisper（可选）

1. 托盘 **设置 → 配置 → 设置本地 Whisper 目录**，选择或新建一个文件夹作为根目录（保存后会自动创建 `bin/`、`models/`、`tmp/`）。
2. 将 [whisper.cpp](https://github.com/ggml-org/whisper.cpp/releases) 的 `whisper-cli.exe`（或 `main.exe`）放入该目录下的 `bin/`。
3. 将 `ggml-base.bin` 等模型放入 `models/`（详见目录内 `README.txt`）。
4. 在 **设置 → 配置 → 模型选择** 中选用 **本地 Whisper（离线）**。

路径保存在 `settings.json` 的 `model.local_whisper_dir`。若你曾使用旧版默认目录 `{配置目录}/whisper`，首次升级会自动迁移该路径。

无需 API Key；转写在本地执行，按需启动 whisper-cli。
