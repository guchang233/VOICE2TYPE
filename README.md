# Voice2Type

Windows 语音输入与实时字幕工具，支持云端 API 和本地离线模型。基于 Tauri 2.0 构建。

## 功能

| 功能 | 触发方式 | 说明 |
|------|----------|------|
| 整段识别 | `F2` | 录音结束后整段转写，一次性输出 |
| 流式识别 | 界面按钮 | 边说边出字，实时显示（需豆包 API） |
| 实时字幕 | 界面按钮 / `F7` | 悬浮字幕窗口 + 同声传译，多窗口多语言，OBS 直接窗口采集 |
| 语音合成 | 界面按钮 | 文本转语音，支持官方音色库、参数调节、试听与下载（Fish Audio） |
| 视频配音 | 界面按钮 | 识别原声字幕 → 逐句编辑 → 换音色重新配音 → 导出成片 + SRT 字幕 |

## 实时字幕

- **音源**：麦克风、系统声音（WASAPI 环回），或「双向同传」——系统声音作为「对方」显示原文与译文，麦克风作为「我」显示在副字幕
- **所见即所得**：页面顶部的舞台就是字幕窗口本身的渲染器，改样式立即预览，开启后实时显示画面；所有设置自动保存
- **多窗口**：每个字幕窗口独立的样式、元素顺序与翻译语言（例如一个窗口出英文、一个出日文），同一语言只翻译一次
- **同声传译**：定稿句带上文翻译，说话过程中先出当前句的译文草稿；原文与译文按句对齐，不会错位
- **转录**：每句带真实起止时间，可导出 SRT / TXT / Markdown

## 视频配音

「视频配音」页把视频拖进来（或点「选择视频」），按四步完成：

1. **识别字幕**：提取音轨并分块，云端 ASR 带时间戳转写。默认**阿里云百炼 Qwen ASR**（qwen3-asr-flash-filetrans，句/词级毫秒时间戳，需在「设置 → API 密钥」填写阿里云百炼 Key），也可切换为跟随「设置 → 整段识别」的云端配置；已有字幕可直接导入 SRT 跳过识别
2. **编辑**：边看视频边改字——Enter 在光标处拆句，行首 Backspace 并入上一句，可改起止时间、按字数重新分段、单句试听配音，语速超标的句子会标黄 / 标红；修改可撤销，工程自动保存，重启不丢
3. **配音**：逐句 Fish Audio TTS（音色与「语音合成」页共用）。放得下的句子保持自然语速，放不下才压缩，并可借用句间停顿；每句的合成状态实时显示
4. **导出**：替换原声，或保留压低的原声做背景（适合有背景音乐的视频），输出 `<原名>_dubbed.mp4` 与同名 SRT

ffmpeg 优先使用系统 PATH 中已安装的版本；未安装时会自动下载内置引擎到模型目录。

## 语音合成（TTS）

基于 [Fish Audio](https://fish.audio) 的文本转语音能力，默认使用免费层 `s2.1-pro-free` 模型。

1. 在 [fish.audio/app/api-keys](https://fish.audio/app/api-keys) 获取 API Key
2. 打开「语音合成」页，在右侧参数面板填入 API Key
3. 点击「音色库」浏览官方音色库并选择音色（可选，留空使用默认）
4. 输入文本，点击「试听合成」生成并播放；点击「下载」保存为音频文件

支持调节模型（S2.1 Pro Free / S2.1 Pro / S2 Pro / S1）、格式（MP3/WAV/Opus/PCM）、语速、音量、延迟模式、温度、Top P、分段长度、文本归一化等参数。

## 识别引擎

**整段识别**（录音后转写）：
- SiliconFlow（SenseVoiceSmall / TeleSpeechASR）
- Groq（Whisper Large v3）
- 本地 Whisper（whisper.cpp 离线，无需 API Key）
- 自定义提供商（兼容 OpenAI 接口）

**流式识别 / 实时字幕**：
- 豆包大模型流式 ASR（火山引擎 WebSocket）

## 快速开始

从 [Releases](https://github.com/guchang233/VOICE2TYPE/releases) 下载便携 exe，免安装直接运行。

### 整段识别（F2）

1. 设置 → API 密钥，填写 SiliconFlow 或 Groq Key（本地 Whisper 无需 Key）
2. 选中输入框，按住 `F2` 说话，松开后自动输入

### 流式识别

1. [火山引擎控制台](https://console.volcengine.com/) 开通豆包流式语音识别，获取 API Key
2. 设置 → API 密钥，填写豆包 API Key
3. 语音输入页切换为"流式"模式，按住麦克风按钮说话，松手结束

### 本地 Whisper（离线）

1. 设置 → 本地模型管理，点击"下载引擎"自动下载 whisper.cpp 二进制
2. 点击模型链接下载 `ggml-*.bin`（推荐 base），放入模型目录
3. 语音识别模型 → 整段识别，选择"本地 Whisper"

## 流式后处理

| 模式 | 说明 |
|------|------|
| 关闭 | 仅保留识别原文 |
| 本地修正 | 轻量规则修正错别字、重复标点 |
| AI 润色 | 结束时用 LLM 润色（需 SiliconFlow / Groq Key） |

## 开发

依赖：Node.js + Rust（stable）。

```powershell
npm install
npm run dev      # 开发模式
npm run build    # 构建 NSIS 安装包
```

管理员权限构建（注入受保护窗口）：

```powershell
npm run dev -- -- --features admin
```

> GitHub Actions 使用 `cargo build --release` 直接产出便携 exe，与本地 `tauri build` 的 NSIS 安装包不同。

## 参考

- [豆包流式语音识别 API](https://www.volcengine.com/docs/6561/1354868)
- [whisper.cpp](https://github.com/ggml-org/whisper.cpp)
