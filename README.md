# Voice2Type

Windows 托盘语音输入法，支持两种互不干扰的识别模式：

| 模式 | 热键（默认） | 说明 |
|------|-------------|------|
| **录音文件识别** | `F2` | 按住说话，松开后整段转写并一次性输入 |
| **流式语音识别** | `F6` | 边说边出字，类似手机语音输入，需豆包流式 API |

> 建议从 [Releases](https://github.com/guchang233/VOICE2TYPE/releases) 下载最新版本。

## 核心功能

### 录音文件识别（`语音转文字` 菜单）

- 全局热键录音，支持按住说话 / 按一下切换
- 剪贴板粘贴 / 键盘注入两种输出方式
- SiliconFlow / Groq / 本地 Whisper 转写引擎
- 可选过滤标点和表情符号
- 支持重新粘贴上一条结果

### 流式语音识别（`流式语音识别` 菜单）

- 火山引擎豆包大模型流式 ASR（WebSocket，`X-Api-Key` 鉴权）
- 默认热键 `F6`，可在菜单中修改
- 识别结果实时写入当前输入框，不抢占输入框内原有文字
- 三种**后处理模式**（默认：**本地轻量修正**）：
  - **本地轻量修正**：极保守规则（少量错别字、重复标点等），适合日常使用
  - **AI 润色**：结束时用硅基流动 / Groq 免费 Chat 模型润色（需对应 API Key）；含数字、版本号（如 `0.0.65`）时自动跳过 AI
  - **关闭后处理**：仅保留识别原文与菜单中的标点/表情开关

### 通用

- 开机自启、状态浮窗、更新检查、日志窗口

## 快速开始

### 1. 录音文件识别（F2）

1. 托盘 **模型与密钥 → API Key**，填写 SiliconFlow 或 Groq Key（本地 Whisper 可跳过）。
2. 选中目标输入框，按住 `F2` 说话，松开后自动输入。
3. 输出方式建议选 **剪贴板粘贴**（`语音转文字 → 输出方式`）。

### 2. 流式语音识别（F6）

1. 在 [火山引擎控制台](https://console.volcengine.com/) 开通豆包流式语音识别，获取 **API Key**（非 Access Token）。
2. 托盘 **模型与密钥 → API Key**，填写 **豆包流式 API Key**。
3. **流式语音识别 → 资源 ID** 选择与控制台一致的计费资源（默认 `volc.bigasr.sauc.duration`）。
4. **流式语音识别 → 后处理模式** 按需选择（默认 **本地轻量修正**）。
5. 光标放入输入框，按住 `F6` 说话；松手结束。按住时按 `ESC` 可取消。

## 本地 Whisper（离线，仅录音模式）

无需 API Key，转写在本地执行。

1. **模型与密钥 → 设置本地 Whisper 目录**，选择空文件夹并按目录内 `README.txt` 准备模型与 CLI。
2. **模型设置** 中切换为 **本地 Whisper（离线）**。

## 配置与菜单

配置文件：当前目录 `voice2type_config.json`，否则为系统配置目录下的 `settings.json`。日志：`logs/app.log`。

| 托盘菜单 | 作用 |
|----------|------|
| 语音转文字（录音识别） | F2 热键、输出方式、识别语言、通用设置 |
| 流式语音识别 | F6 热键、后处理模式、资源 ID、识别语言 |
| 模型与密钥 | SiliconFlow / Groq / **豆包流式** API Key、模型设置 |

流式后处理配置项 `streaming.post_process_mode`：`local`（默认）| `ai` | `none`。

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

管理员权限构建（部分受保护窗口需要）：

```powershell
cargo run --features admin
```

## 参考文档

- [大模型流式语音识别 API](https://www.volcengine.com/docs/6561/1354868?lang=zh)
