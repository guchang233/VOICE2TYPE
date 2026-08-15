//! 旧配置 JSON → v3 模型的迁移。
//!
//! 在 `ConfigManager::load_config` 反序列化之前对 `serde_json::Value` 原地转换：
//! v2 平铺字段 + `subtitle_scenes[]` → v3 `windows[]`（窗口 + 主题 + 统一元素）。
//! v3 结构体使用 camelCase 序列化，因此迁移产物也输出 camelCase 键。

use serde_json::{json, Map, Value};

/// 入口：检测旧模型并原地改写 root["subtitle"]。
pub fn migrate_subtitle_json(root: &mut Value) {
    let Some(sub) = root.get_mut("subtitle").and_then(|v| v.as_object_mut()) else {
        return;
    };
    if sub.contains_key("windows") {
        return; // 已是 v3 模型
    }
    let is_legacy = sub.contains_key("subtitleScenes") || sub.contains_key("subtitle_scenes")
        || sub.contains_key("subtitleFontSize") || sub.contains_key("subtitle_font_size");
    if !is_legacy {
        return;
    }

    let hotkey = pick(sub, &["subtitleHotkey", "subtitle_hotkey"]).unwrap_or_else(|| json!(0x76));
    let audio_source = pick(sub, &["subtitleAudioSource", "subtitle_audio_source"])
        .unwrap_or_else(|| json!("microphone"));
    let input_device = pick(sub, &["subtitleInputDevice", "subtitle_input_device"])
        .unwrap_or_else(|| json!(""));
    let llm = json!({
        "apiUrl": pick(sub, &["subtitleTranslationLlmApiUrl", "subtitle_translation_llm_api_url"]).unwrap_or_else(|| json!("")),
        "apiKey": pick(sub, &["subtitleTranslationLlmApiKey", "subtitle_translation_llm_api_key"]).unwrap_or_else(|| json!("")),
        "model": pick(sub, &["subtitleTranslationLlmModel", "subtitle_translation_llm_model"]).unwrap_or_else(|| json!("")),
    });

    let mut windows: Vec<Value> = Vec::new();
    let scenes: Vec<Map<String, Value>> = ["subtitleScenes", "subtitle_scenes"]
        .iter()
        .find_map(|k| sub.get(*k).and_then(|v| v.as_array()))
        .map(|arr| {
            arr.iter()
                .filter_map(|s| s.as_object().cloned())
                .collect()
        })
        .unwrap_or_default();

    if scenes.is_empty() {
        // 只有平铺字段的旧配置：合成默认窗口
        windows.push(migrate_scene(&Map::new(), sub));
    } else {
        for scene in &scenes {
            windows.push(migrate_scene(scene, sub));
        }
    }

    *sub = json!({
        "hotkey": hotkey,
        "audioSource": audio_source,
        "inputDevice": input_device,
        "translationLlm": llm,
        "windows": windows,
    })
    .as_object()
    .cloned()
    .unwrap_or_default();
}

/// 从 map 中取第一个存在的键
fn pick(map: &Map<String, Value>, keys: &[&str]) -> Option<Value> {
    keys.iter().find_map(|k| map.get(*k).cloned())
}

/// 读取嵌套对象（scene.window / scene.style / scene.translation）
fn nested(map: &Map<String, Value>, key: &str) -> Map<String, Value> {
    map.get(key)
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default()
}

fn str_of(map: &Map<String, Value>, keys: &[&str], fallback: &str) -> String {
    pick(map, keys)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn f64_of(map: &Map<String, Value>, keys: &[&str], fallback: f64) -> f64 {
    pick(map, keys)
        .and_then(|v| v.as_f64())
        .unwrap_or(fallback)
}

fn u32_of(map: &Map<String, Value>, keys: &[&str], fallback: u32) -> u32 {
    pick(map, keys)
        .and_then(|v| v.as_u64())
        .map(|n| n as u32)
        .unwrap_or(fallback)
}

fn i32_of(map: &Map<String, Value>, keys: &[&str], fallback: i32) -> i32 {
    pick(map, keys)
        .and_then(|v| v.as_i64())
        .map(|n| n as i32)
        .unwrap_or(fallback)
}

fn bool_of(map: &Map<String, Value>, keys: &[&str], fallback: bool) -> bool {
    pick(map, keys)
        .and_then(|v| v.as_bool())
        .unwrap_or(fallback)
}

/// 迁移单个场景：scene 自身字段优先，缺失时回退到旧平铺字段。
fn migrate_scene(scene: &Map<String, Value>, flat: &Map<String, Value>) -> Value {
    let win = nested(scene, "window");
    let style = nested(scene, "style");
    let tr = nested(scene, "translation");

    // ===== 窗口控制 =====
    let id = if str_of(scene, &["id"], "default") == "default" {
        "primary".to_string()
    } else {
        str_of(scene, &["id"], "w_legacy")
    };
    let name = str_of(scene, &["name"], "默认字幕");
    let x = i32_of(&win, &["x"], i32_of(flat, &["subtitleWindowX", "subtitle_window_x"], -1));
    let y = i32_of(&win, &["y"], i32_of(flat, &["subtitleWindowY", "subtitle_window_y"], -1));
    let width = u32_of(&win, &["width"], u32_of(flat, &["subtitleWindowWidth", "subtitle_window_width"], 1200));
    let height = u32_of(&win, &["height"], u32_of(flat, &["subtitleWindowHeight", "subtitle_window_height"], 120));
    let always_on_top = bool_of(&win, &["alwaysOnTop", "always_on_top"], true);
    let click_through = bool_of(&win, &["clickThrough", "click_through"], false);
    let obs_mode = bool_of(&win, &["obsMode", "obs_mode"], false);
    let auto_fit = bool_of(&win, &["autoFit", "auto_fit"], true);

    // ===== 主题 =====
    let font_size = u32_of(&style, &["fontSize", "font_size"], u32_of(flat, &["subtitleFontSize", "subtitle_font_size"], 32));
    let theme = json!({
        "preset": str_of(&style, &["preset"], &str_of(flat, &["subtitlePreset", "subtitle_preset"], "custom")),
        "fontFamily": str_of(&style, &["fontFamily", "font_family"], &str_of(flat, &["subtitleFontFamily", "subtitle_font_family"], "SimHei")),
        "fontSize": font_size,
        "fontWeight": u32_of(&style, &["fontWeight", "font_weight"], u32_of(flat, &["subtitleFontWeight", "subtitle_font_weight"], 400)),
        "italic": bool_of(&style, &["italic"], bool_of(flat, &["subtitleItalic", "subtitle_italic"], false)),
        "fontColor": str_of(&style, &["fontColor", "font_color"], &str_of(flat, &["subtitleFontColor", "subtitle_font_color"], "#ffffff")),
        "textAlign": str_of(&style, &["textAlign", "text_align"], &str_of(flat, &["subtitleTextAlign", "subtitle_text_align"], "center")),
        "letterSpacing": f64_of(&style, &["letterSpacing", "letter_spacing"], f64_of(flat, &["subtitleLetterSpacing", "subtitle_letter_spacing"], 0.0)),
        "lineHeight": f64_of(&style, &["lineHeight", "line_height"], f64_of(flat, &["subtitleLineHeight", "subtitle_line_height"], 1.4)),
        "textShadowColor": str_of(&style, &["textShadowColor", "text_shadow_color"], &str_of(flat, &["subtitleTextShadowColor", "subtitle_text_shadow_color"], "#000000")),
        "textShadowStrength": u32_of(&style, &["textShadowStrength", "text_shadow_strength"], u32_of(flat, &["subtitleTextShadowStrength", "subtitle_text_shadow_strength"], 4)),
        "interimColor": str_of(&style, &["interimColor", "interim_color"], &str_of(flat, &["subtitleInterimColor", "subtitle_interim_color"], "#ffffff")),
        "interimOpacity": f64_of(&style, &["interimOpacity", "interim_opacity"], f64_of(flat, &["subtitleInterimOpacity", "subtitle_interim_opacity"], 0.7)),
        "bgColor": str_of(&style, &["bgColor", "bg_color"], &str_of(flat, &["subtitleBgColor", "subtitle_bg_color"], "#000000")),
        "bgOpacity": f64_of(&style, &["bgOpacity", "bg_opacity"], f64_of(flat, &["subtitleBgOpacity", "subtitle_bg_opacity"], 0.6)),
        "blur": u32_of(&style, &["blur"], u32_of(flat, &["subtitleBlur", "subtitle_blur"], 20)),
        "paddingX": u32_of(&style, &["paddingX", "padding_x"], u32_of(flat, &["subtitlePaddingX", "subtitle_padding_x"], 24)),
        "paddingY": u32_of(&style, &["paddingY", "padding_y"], u32_of(flat, &["subtitlePaddingY", "subtitle_padding_y"], 12)),
        "maxLines": u32_of(&style, &["maxLines", "max_lines"], u32_of(flat, &["subtitleMaxLines", "subtitle_max_lines"], 3)),
        "layout": str_of(&style, &["layout"], &str_of(flat, &["subtitleLayout", "subtitle_layout"], "vertical")),
        "anchorX": str_of(&style, &["containerAlignX", "container_align_x"], "center"),
        "anchorY": str_of(&style, &["containerAlignY", "container_align_y"], "bottom"),
        "maxWidthPct": u32_of(&style, &["boxMaxWidth", "box_max_width"], 100),
        "translation": {
            "size": u32_of(&style, &["translationFontSize", "translation_font_size"], u32_of(flat, &["subtitleTranslationFontSize", "subtitle_translation_font_size"], 24)),
            "weight": u32_of(&style, &["translationFontWeight", "translation_font_weight"], u32_of(flat, &["subtitleTranslationFontWeight", "subtitle_translation_font_weight"], 400)),
            "color": str_of(&style, &["translationFontColor", "translation_font_color"], &str_of(flat, &["subtitleTranslationFontColor", "subtitle_translation_font_color"], "#ffffff")),
            "opacity": f64_of(&style, &["translationOpacity", "translation_opacity"], f64_of(flat, &["subtitleTranslationOpacity", "subtitle_translation_opacity"], 0.85)),
            "prefix": str_of(&style, &["translationPrefix", "translation_prefix"], &str_of(flat, &["subtitleTranslationPrefix", "subtitle_translation_prefix"], "")),
        },
        "speaker": {
            "color": str_of(&style, &["speakerColor", "speaker_color"], &str_of(flat, &["subtitleSpeakerColor", "subtitle_speaker_color"], "#818cf8")),
            "size": u32_of(&style, &["speakerFontSize", "speaker_font_size"], u32_of(flat, &["subtitleSpeakerFontSize", "subtitle_speaker_font_size"], 16)),
            "prefix": str_of(&style, &["speakerPrefix", "speaker_prefix"], &str_of(flat, &["subtitleSpeakerPrefix", "subtitle_speaker_prefix"], "")),
        },
        "timestamp": {
            "color": str_of(&style, &["timestampColor", "timestamp_color"], &str_of(flat, &["subtitleTimestampColor", "subtitle_timestamp_color"], "#a1a1aa")),
            "size": u32_of(&style, &["timestampFontSize", "timestamp_font_size"], u32_of(flat, &["subtitleTimestampFontSize", "subtitle_timestamp_font_size"], 14)),
            "format": str_of(&style, &["timestampFormat", "timestamp_format"], &str_of(flat, &["subtitleTimestampFormat", "subtitle_timestamp_format"], "HH:MM:SS")),
        },
        "secondary": { "color": "#7dd3fc", "size": 0, "opacity": 0.9 },
    });

    // ===== 统一元素列表 =====
    let show_original = bool_of(
        &style,
        &["showOriginal", "show_original"],
        bool_of(flat, &["subtitleShowOriginal", "subtitle_show_original"], true),
    );
    let show_translation = bool_of(
        &style,
        &["showTranslation", "show_translation"],
        bool_of(flat, &["subtitleShowTranslation", "subtitle_show_translation"], false),
    );
    let show_speaker = bool_of(
        &style,
        &["showSpeaker", "show_speaker"],
        bool_of(flat, &["subtitleShowSpeaker", "subtitle_show_speaker"], false),
    );
    let show_timestamp = bool_of(
        &style,
        &["showTimestamp", "show_timestamp"],
        bool_of(flat, &["subtitleShowTimestamp", "subtitle_show_timestamp"], false),
    );
    let show_secondary = bool_of(
        &style,
        &["showOriginalSecondary", "show_original_secondary"],
        bool_of(
            flat,
            &["subtitleShowOriginalSecondary", "subtitle_show_original_secondary"],
            false,
        ),
    );

    let custom: Vec<Map<String, Value>> = pick(&style, &["customElements", "custom_elements"])
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|c| c.as_object().cloned())
        .collect();

    // 按旧 element_order 排列；旧 key "original2" → 新 "secondary"
    let mut order: Vec<String> = pick(&style, &["elementOrder", "element_order"])
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_else(|| {
            json!(["speaker", "original", "translation", "timestamp", "original2"])
                .as_array()
                .cloned()
                .unwrap()
        })
        .into_iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();
    for key in order.iter_mut() {
        if *key == "original2" {
            *key = "secondary".to_string();
        }
    }
    for fixed in ["speaker", "original", "translation", "secondary", "timestamp"] {
        if !order.iter().any(|k| k == fixed) {
            order.push(fixed.to_string());
        }
    }

    let fixed_enabled = |kind: &str| -> bool {
        match kind {
            "original" => show_original,
            "translation" => show_translation,
            "speaker" => show_speaker,
            "timestamp" => show_timestamp,
            _ => show_secondary,
        }
    };

    let mut elements: Vec<Value> = Vec::new();
    for key in &order {
        match key.as_str() {
            "speaker" | "original" | "translation" | "secondary" | "timestamp" => {
                let label = match key.as_str() {
                    "speaker" => "说话人",
                    "original" => "原文",
                    "translation" => "译文",
                    "secondary" => "副原文（麦克风）",
                    _ => "时间戳",
                };
                elements.push(json!({
                    "kind": key, "id": key, "enabled": fixed_enabled(key),
                    "label": label, "content": "", "prefix": "", "color": "",
                    "fontSize": 0, "fontWeight": 0, "opacity": 1.0, "align": "",
                }));
            }
            _ => {
                // 自定义元素
                if let Some(c) = custom.iter().find(|c| str_of(c, &["id"], "") == *key) {
                    let kind = str_of(c, &["elementType", "element_type"], "text");
                    elements.push(json!({
                        "kind": kind,
                        "id": key.clone(),
                        "enabled": bool_of(c, &["visible"], true),
                        "label": str_of(c, &["label"], "自定义文本"),
                        "content": str_of(c, &["content"], ""),
                        "prefix": str_of(c, &["prefix"], ""),
                        "color": str_of(c, &["color"], "#ffffff"),
                        "fontSize": u32_of(c, &["fontSize", "font_size"], 18),
                        "fontWeight": u32_of(c, &["fontWeight", "font_weight"], 400),
                        "opacity": f64_of(c, &["opacity"], 0.9),
                        "align": str_of(c, &["align"], "center"),
                    }));
                }
            }
        }
    }

    json!({
        "id": id,
        "name": name,
        "enabled": bool_of(scene, &["enabled"], true),
        "x": x, "y": y, "width": width, "height": height,
        "alwaysOnTop": always_on_top,
        "clickThrough": click_through,
        "obsMode": obs_mode,
        "autoFit": auto_fit,
        "translation": {
            "engine": str_of(&tr, &["engine"], &str_of(flat, &["subtitleTranslationEngine", "subtitle_translation_engine"], "none")),
            "targetLang": str_of(&tr, &["targetLang", "target_lang"], &str_of(flat, &["subtitleTranslationTargetLang", "subtitle_translation_target_lang"], "英文")),
            "interim": bool_of(&tr, &["interim"], true),
        },
        "theme": theme,
        "elements": elements,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn migrates_flat_v2_to_windows() {
        let mut root = parse(r##"{
            "subtitle": {
                "subtitle_hotkey": 119,
                "subtitle_audio_source": "dual",
                "subtitle_input_device": "立体声混音",
                "subtitle_font_size": 40,
                "subtitle_bg_color": "#112233",
                "subtitle_max_lines": 2,
                "subtitle_show_speaker": true,
                "subtitle_translation_llm_model": "qwen"
            }
        }"##);
        migrate_subtitle_json(&mut root);
        let sub = root.get("subtitle").unwrap();
        assert_eq!(sub.get("hotkey").unwrap().as_u64().unwrap(), 119);
        assert_eq!(sub.get("audioSource").unwrap().as_str().unwrap(), "dual");
        let windows = sub.get("windows").unwrap().as_array().unwrap();
        assert_eq!(windows.len(), 1);
        let w = &windows[0];
        assert_eq!(w.get("id").unwrap().as_str().unwrap(), "primary");
        assert_eq!(w.get("theme").unwrap().get("fontSize").unwrap().as_u64().unwrap(), 40);
        assert_eq!(w.get("theme").unwrap().get("bgColor").unwrap().as_str().unwrap(), "#112233");
        // speaker 元素被启用
        let speaker = w
            .get("elements")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e.get("kind").unwrap().as_str().unwrap() == "speaker")
            .unwrap()
            .clone();
        assert_eq!(speaker.get("enabled").unwrap().as_bool().unwrap(), true);
        assert_eq!(
            sub.get("translationLlm").unwrap().get("model").unwrap().as_str().unwrap(),
            "qwen"
        );
    }

    #[test]
    fn migrates_scenes_with_custom_elements_and_order() {
        let mut root = parse(r##"{
            "subtitle": {
                "subtitle_scenes": [{
                    "id": "sc_1",
                    "name": "会议字幕",
                    "enabled": true,
                    "window": { "x": 10, "y": 20, "width": 800, "height": 200, "obsMode": true },
                    "style": {
                        "fontSize": 28,
                        "show_original": true,
                        "show_translation": true,
                        "custom_elements": [
                            { "id": "clock", "element_type": "text", "content": "{time}", "color": "#00ff00" }
                        ],
                        "element_order": ["clock", "original", "translation"]
                    },
                    "translation": { "engine": "llm", "targetLang": "日文", "interim": false }
                }]
            }
        }"##);
        migrate_subtitle_json(&mut root);
        let w = &root.get("subtitle").unwrap().get("windows").unwrap()[0];
        assert_eq!(w.get("id").unwrap().as_str().unwrap(), "sc_1");
        assert_eq!(w.get("name").unwrap().as_str().unwrap(), "会议字幕");
        assert_eq!(w.get("obsMode").unwrap().as_bool().unwrap(), true);
        let elements = w.get("elements").unwrap().as_array().unwrap();
        assert_eq!(elements[0].get("kind").unwrap().as_str().unwrap(), "text");
        assert_eq!(elements[0].get("content").unwrap().as_str().unwrap(), "{time}");
        assert_eq!(elements[1].get("kind").unwrap().as_str().unwrap(), "original");
        assert_eq!(elements[2].get("kind").unwrap().as_str().unwrap(), "translation");
        assert_eq!(
            w.get("translation").unwrap().get("targetLang").unwrap().as_str().unwrap(),
            "日文"
        );
    }

    #[test]
    fn noop_for_v3_and_missing_subtitle() {
        let mut v3 = parse(r##"{"subtitle": {"windows": [], "hotkey": 118}}"##);
        migrate_subtitle_json(&mut v3);
        assert!(v3.get("subtitle").unwrap().get("windows").is_some());

        let mut none = parse(r##"{"basic": {}}"##);
        migrate_subtitle_json(&mut none);
        assert!(none.get("subtitle").is_none());
    }
}
