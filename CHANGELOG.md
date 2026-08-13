# 更新日志

本项目所有重要变更均会记录在此文件中。

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [0.1.8] - 2026-08-12

### 新增
- **Fish Audio 文本转语音（TTS）功能**
  - 调用 Fish Audio 免费 S2.1 PRO FREE 模型进行文本转语音
  - 支持连接官方音色库，可搜索、分页浏览、按语言/排序/自建筛选
  - 支持调整合成参数：语速、音量、温度、top_p、chunk_length、mp3_bitrate、输出格式
  - 支持生成语音、播放、下载导出
  - 设置面板支持展开/收缩，参数实时保存
  - 自动检测并使用 Windows 系统代理（含环境变量与注册表）

### 修复
- **后端安全修复**
  - 修复 Windows 注册表 ProxyServer 读取越界（UB），增加返回值检查与缓冲区长度上限
  - `tts_export` 增加源路径校验，防止路径遍历攻击
  - SOCKS 代理解析修正，支持 `socks=` 前缀并正确映射为 `socks5://`
  - 代理 URL 日志脱敏，移除可能包含的凭据信息
- **前端安全修复**
  - 新增 `escapeAttr` 函数，修复 `escapeHtml` 在属性上下文未转义引号导致的 XSS
  - `showConfirmDialog` 对 title/message/confirmText 进行 HTML 转义

### 变更
- 将「试听合成」按钮改为「生成」按钮，移除自动播放逻辑，由用户手动控制播放
- 每次生成使用 UUID 唯一文件名，解决再次生成时播放器仍播放上一条音频的问题
- 生成按钮增加重入防护：文本改动时弹窗确认，生成期间隐藏播放器并禁用按钮
- 阻塞文件 IO 改用 `tokio::fs`，避免阻塞 tokio 运行时
- preview 文件清理改为忽略扩展名，防止跨格式残留
- `docs/` 目录加入 .gitignore

## [0.1.7] - 2026-08-10

### 变更
- 版本发布与构建配置调整
