//! SQLite 缓存：同一个词第二次查不走网络。
//!
//! 表结构照抄方案 §7.1。缓存里存的是**模型原始输出**，不是校验后的对象：
//! zod 是唯一校验者，缓存命中时前端照样再校验一次（成本可忽略）。

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};

/// 与前端 `src/lib/schema.ts` 的常量保持一致；改契约/改 prompt 都会让旧缓存失效。
pub const SCHEMA_VERSION: i64 = 1;

pub struct Cache {
    conn: Mutex<Connection>,
}

impl Cache {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|err| format!("创建缓存目录失败: {err}"))?;
        }
        let conn = Connection::open(path).map_err(|err| format!("打开缓存库失败: {err}"))?;
        Self::from_conn(conn)
    }

    pub fn in_memory() -> Result<Self, String> {
        let conn = Connection::open_in_memory().map_err(|err| format!("打开内存库失败: {err}"))?;
        Self::from_conn(conn)
    }

    fn from_conn(conn: Connection) -> Result<Self, String> {
        migrate(&conn).map_err(|err| format!("建表失败: {err}"))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 命中就把 payload 返回，并把 hit_count 加一。
    pub fn get(&self, key: &str) -> Result<Option<String>, String> {
        let conn = self.lock();
        let payload: Option<String> = conn
            .query_row(
                "SELECT payload FROM entries WHERE key = ?1 AND stale = 0",
                params![key],
                |row| row.get(0),
            )
            .ok();

        if payload.is_some() {
            let _ = conn.execute(
                "UPDATE entries SET hit_count = hit_count + 1 WHERE key = ?1",
                params![key],
            );
        }
        Ok(payload)
    }

    pub fn put(
        &self,
        key: &str,
        word: &str,
        lang: &str,
        model: &str,
        payload: &str,
    ) -> Result<(), String> {
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0);

        self.lock()
            .execute(
                "INSERT INTO entries (key, word, lang, model, schema_ver, prompt_ver, payload, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(key) DO UPDATE SET payload = excluded.payload, stale = 0",
                params![key, word, lang, model, SCHEMA_VERSION, 1, payload, created_at],
            )
            .map_err(|err| format!("写缓存失败: {err}"))?;
        Ok(())
    }

    /// 标记为过期：校验失败的结果不该被第二次读到。
    pub fn mark_stale(&self, key: &str) -> Result<(), String> {
        self.lock()
            .execute("UPDATE entries SET stale = 1 WHERE key = ?1", params![key])
            .map_err(|err| format!("标记缓存失效失败: {err}"))?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn count(&self) -> Result<i64, String> {
        self.lock()
            .query_row("SELECT COUNT(*) FROM entries", [], |row| row.get(0))
            .map_err(|err| format!("统计缓存失败: {err}"))
    }
}

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS entries (
            key            TEXT PRIMARY KEY,
            word           TEXT NOT NULL,
            lang           TEXT NOT NULL,
            model          TEXT NOT NULL,
            schema_ver     INTEGER NOT NULL,
            prompt_ver     INTEGER NOT NULL,
            payload        TEXT NOT NULL,
            created_at     INTEGER NOT NULL,
            hit_count      INTEGER DEFAULT 0,
            stale          INTEGER DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_entries_word ON entries(word, lang);
        CREATE TABLE IF NOT EXISTS clicks (
            word TEXT, sense_rank INTEGER, gloss TEXT,
            views INTEGER DEFAULT 0, clicks INTEGER DEFAULT 0,
            PRIMARY KEY (word, gloss)
        );",
    )
}

/// 缓存 key：词 | 语言 | 模型 | 服务地址 | 上下文 | 契约版本 | prompt 指纹。
///
/// prompt 用内容哈希而不是手写版本号：改提示词自动失效，不用记得改常量。
pub fn entry_key(
    word: &str,
    lang: &str,
    model: &str,
    base_url: &str,
    context: Option<&str>,
    prompt_hash: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!(
        "{word}|{lang}|{model}|{base_url}|{}|{SCHEMA_VERSION}|{prompt_hash}",
        context.unwrap_or("")
    ));
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_changes_with_every_input() {
        let base = entry_key("charge", "en", "m", "https://a/v1", None, "h1");
        assert_ne!(base, entry_key("charge", "zh", "m", "https://a/v1", None, "h1"));
        assert_ne!(base, entry_key("charge", "en", "m2", "https://a/v1", None, "h1"));
        assert_ne!(base, entry_key("charge", "en", "m", "https://b/v1", None, "h1"));
        assert_ne!(base, entry_key("charge", "en", "m", "https://a/v1", Some("ctx"), "h1"));
        assert_ne!(base, entry_key("charge", "en", "m", "https://a/v1", None, "h2"), "prompt 变了必须换 key");
        assert_eq!(base, entry_key("charge", "en", "m", "https://a/v1", None, "h1"));
    }

    #[test]
    fn stale_rows_are_not_served() {
        let cache = Cache::in_memory().expect("内存库");
        let key = entry_key("charge", "en", "m", "u", None, "h");
        cache.put(&key, "charge", "en", "m", "v1").expect("写");
        cache.mark_stale(&key).expect("标记");
        assert!(cache.get(&key).expect("查").is_none(), "过期数据不能再返回");
    }

    #[test]
    fn put_then_get_roundtrip() {
        let cache = Cache::in_memory().expect("内存库");
        let key = entry_key("charge", "en", "m", "u", None, "h");
        assert!(cache.get(&key).expect("查").is_none(), "初始应为空");

        cache
            .put(&key, "charge", "en", "m", "{\"ok\":1}")
            .expect("写");
        assert_eq!(cache.get(&key).expect("查").as_deref(), Some("{\"ok\":1}"));
        assert_eq!(cache.count().expect("计数"), 1);
    }

    #[test]
    fn put_is_idempotent_for_same_key() {
        let cache = Cache::in_memory().expect("内存库");
        let key = entry_key("charge", "en", "m", "u", None, "h");
        cache.put(&key, "charge", "en", "m", "v1").expect("写");
        cache.put(&key, "charge", "en", "m", "v2").expect("覆盖");
        assert_eq!(cache.get(&key).expect("查").as_deref(), Some("v2"));
        assert_eq!(cache.count().expect("计数"), 1, "同一 key 不应产生两行");
    }
}
