//! 密钥读写：只走 macOS Keychain（`keyring` → Security.framework）。
//!
//! 这里没有任何「读出来给前端」的接口。前端只能拿到掩码后的字符串。

const SERVICE: &str = "com.sense.app";

fn account(provider: &str) -> String {
    format!("provider:{provider}")
}

fn entry(provider: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, &account(provider))
        .map_err(|err| format!("打开钥匙串失败: {err}"))
}

/// 只暴露掩码：`sk-****abcd`。长度不足时一律 `****`。
pub fn mask(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() < 8 {
        return "****".to_string();
    }
    let tail: String = chars[chars.len() - 4..].iter().collect();
    let prefix = if key.starts_with("sk-") { "sk-" } else { "" };
    format!("{prefix}****{tail}")
}

pub fn set_key(provider: &str, key: &str) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("Key 不能为空".to_string());
    }
    entry(provider)?
        .set_password(key)
        .map_err(|err| format!("写入钥匙串失败: {err}"))
}

/// 只有 Rust 内部能用。前端永远拿不到返回值。
pub fn get_key(provider: &str) -> Result<Option<String>, String> {
    match entry(provider)?.get_password() {
        Ok(key) => Ok(Some(key)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) => Err(format!("读取钥匙串失败: {err}")),
    }
}

pub fn delete_key(provider: &str) -> Result<(), String> {
    match entry(provider)?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(format!("删除钥匙串条目失败: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_keeps_only_last_four() {
        assert_eq!(mask("sk-1234567890abcd"), "sk-****abcd");
        assert_eq!(mask("12345678"), "****5678");
    }

    #[test]
    fn mask_hides_short_keys_entirely() {
        assert_eq!(mask("short"), "****");
        assert_eq!(mask(""), "****");
        assert_eq!(mask("1234567"), "****");
    }

    #[test]
    fn mask_never_leaks_prefix() {
        let masked = mask("sk-live-abcdefghijkl");
        assert!(!masked.contains("sk-live"), "掩码不能泄露前缀: {masked}");
        assert!(masked.ends_with("ijkl"));
    }
}
