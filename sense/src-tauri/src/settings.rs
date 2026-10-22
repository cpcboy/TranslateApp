//! `settings.json` 的唯一读写入口。
//!
//! 所有模块都从这里进出，避免各处自己拼路径、自己 write，
//! 把用户手改的字段或未来版本写的字段冲掉。

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use tauri::{AppHandle, Manager};

/// 首次生成设置文件时写入的默认快捷键。
///
/// 选择理由：`⌥Space` 被 Gemini / ChatGPT 占用；`⌥⌘Space` 是 macOS 系统
/// 符号快捷键（Finder 搜索）且被 Claude 桌面端占用。带 ⌃ 的三修饰键组合
/// 不在系统符号表里。
pub const DEFAULT_HOTKEY_SPEC: &str = "ctrl+alt+cmd+space";

pub const FILE_NAME: &str = "settings.json";

/// 配置文件路径由 `tauri.conf.json` 的 `identifier` 决定：
/// `~/Library/Application Support/<identifier>/settings.json`
pub fn path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|dir| dir.join(FILE_NAME))
        .map_err(|err| format!("取不到配置目录: {err}"))
}

fn default_document() -> Value {
    serde_json::json!({ "hotkey": DEFAULT_HOTKEY_SPEC })
}

/// 读配置；文件不存在就写一份默认的。
pub fn read_at(path: &Path) -> Result<Value, String> {
    if !path.exists() {
        let doc = default_document();
        write_at(path, &doc)?;
        return Ok(doc);
    }

    let text =
        fs::read_to_string(path).map_err(|err| format!("读取 {} 失败: {err}", path.display()))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|err| format!("{} 不是合法 JSON: {err}", path.display()))?;
    if !value.is_object() {
        return Err(format!("{} 的根节点必须是对象", path.display()));
    }
    Ok(value)
}

/// 原子写：先写临时文件再 rename，避免并发读到半截文件。
pub fn write_at(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)
            .map_err(|err| format!("创建目录 {} 失败: {err}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(value)
        .map_err(|err| format!("序列化设置失败: {err}"))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, format!("{text}\n"))
        .map_err(|err| format!("写 {} 失败: {err}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|err| format!("替换 {} 失败: {err}", path.display()))
}

/// 读-改-写：只动指定字段，其余（包括不认识的）原样保留。
pub fn update_at<F>(path: &Path, mutate: F) -> Result<Value, String>
where
    F: FnOnce(&mut Map<String, Value>),
{
    let mut value = read_at(path)?;
    {
        let map = value
            .as_object_mut()
            .ok_or_else(|| format!("{} 的根节点不是对象", path.display()))?;
        mutate(map);
    }
    write_at(path, &value)?;
    Ok(value)
}

pub fn read_value(app: &AppHandle) -> Result<Value, String> {
    let path = path(app)?;
    read_at(&path)
}

pub fn update<F>(app: &AppHandle, mutate: F) -> Result<Value, String>
where
    F: FnOnce(&mut Map<String, Value>),
{
    let path = path(app)?;
    update_at(&path, mutate)
}

/// 取一个字符串字段，类型不对就当没有。
pub fn get_string(app: &AppHandle, key: &str) -> Option<String> {
    read_value(app)
        .ok()
        .and_then(|value| value.get(key).and_then(Value::as_str).map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("sense-settings-test-{name}-{}.json", std::process::id()));
        let _ = fs::remove_file(&path);
        path
    }

    #[test]
    fn missing_file_gets_default_document() {
        let path = temp_path("missing");
        let value = read_at(&path).expect("应生成默认配置");
        assert_eq!(
            value.get("hotkey").and_then(Value::as_str),
            Some(DEFAULT_HOTKEY_SPEC)
        );
        assert!(path.exists(), "应把默认配置落盘");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn update_preserves_unknown_fields() {
        let path = temp_path("preserve");
        write_at(&path, &serde_json::json!({ "hotkey": "ctrl+alt+k", "未来字段": { "a": 1 } }))
            .expect("写入");

        update_at(&path, |map| {
            map.insert("active_provider".to_string(), Value::String("deepseek".into()));
        })
        .expect("更新");

        let value = read_at(&path).expect("读取");
        assert_eq!(value.get("hotkey").and_then(Value::as_str), Some("ctrl+alt+k"));
        assert_eq!(
            value.get("active_provider").and_then(Value::as_str),
            Some("deepseek")
        );
        assert_eq!(
            value.pointer("/未来字段/a").and_then(Value::as_i64),
            Some(1),
            "未知字段不能被冲掉"
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn broken_json_reports_path_and_reason() {
        let path = temp_path("broken");
        fs::write(&path, "{ \"hotkey\": ,, }").expect("写坏文件");
        let err = read_at(&path).expect_err("坏 JSON 必须报错");
        assert!(err.contains("不是合法 JSON"), "错误信息要说明原因: {err}");
        assert!(err.contains("settings-test-broken"), "错误信息要带路径: {err}");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn non_object_root_is_rejected() {
        let path = temp_path("array");
        fs::write(&path, "[1,2,3]").expect("写数组");
        let err = read_at(&path).expect_err("数组根节点必须拒绝");
        assert!(err.contains("根节点必须是对象"), "{err}");
        let _ = fs::remove_file(&path);
    }
}
