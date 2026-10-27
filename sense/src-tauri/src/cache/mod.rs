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

/// 缓存条目上限。单条约几 KB，5000 条也就十几 MB；超出时先淘汰最没价值的。
pub const MAX_ENTRIES: i64 = 5000;

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

        let conn = self.lock();
        conn.execute(
            "INSERT INTO entries (key, word, lang, model, schema_ver, prompt_ver, payload, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(key) DO UPDATE SET payload = excluded.payload, stale = 0",
            params![key, word, lang, model, SCHEMA_VERSION, 1, payload, created_at],
        )
        .map_err(|err| format!("写缓存失败: {err}"))?;
        evict_over(&conn, MAX_ENTRIES, key).map_err(|err| format!("淘汰旧缓存失败: {err}"))?;
        Ok(())
    }

    /// 标记为过期：校验失败的结果不该被第二次读到。
    pub fn mark_stale(&self, key: &str) -> Result<(), String> {
        self.lock()
            .execute("UPDATE entries SET stale = 1 WHERE key = ?1", params![key])
            .map_err(|err| format!("标记缓存失效失败: {err}"))?;
        Ok(())
    }

    #[cfg(test)]
    pub fn count(&self) -> Result<i64, String> {
        self.lock()
            .query_row("SELECT COUNT(*) FROM entries", [], |row| row.get(0))
            .map_err(|err| format!("统计缓存失败: {err}"))
    }
}

/// 超过上限时删掉最没价值的条目：先删已标过期的，再删命中次数最少的，同样少就删最旧的。
/// `keep` 是刚写入的那条，**不参与淘汰**：新写入的命中次数是 0，缓存里的旧条目都命中过时，
/// 不排除它的话，新词一写进去就会因为「命中最少」被立刻删掉，缓存满了之后再也存不进新词。
fn evict_over(conn: &Connection, limit: i64, keep: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM entries WHERE key IN (
             SELECT key FROM entries
             WHERE key != ?2
             ORDER BY stale DESC, hit_count ASC, created_at ASC
             LIMIT MAX(0, (SELECT COUNT(*) FROM entries) - ?1)
         )",
        params![limit, keep],
    )
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
        -- clicks 目前没有任何读写（预留给义项点击统计）。按「只加不删」的约定保留，不 DROP。
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
    fn eviction_drops_stale_then_least_hit_then_oldest() {
        let cache = Cache::in_memory().expect("内存库");
        let conn = cache.lock();
        for (key, hits, stale, created) in [
            ("hot", 9, 0, 1),
            ("cold-old", 0, 0, 1),
            ("cold-new", 0, 0, 5),
            ("stale", 7, 1, 9),
        ] {
            conn.execute(
                "INSERT INTO entries (key, word, lang, model, schema_ver, prompt_ver, payload, created_at, hit_count, stale)
                 VALUES (?1, 'w', 'en', 'm', 1, 1, 'p', ?2, ?3, ?4)",
                params![key, created, hits, stale],
            )
            .expect("插入");
        }

        // 上限 3：只该删一条，且是过期的那条（哪怕它命中最多、最新）
        assert_eq!(evict_over(&conn, 3, "").expect("淘汰"), 1);
        let left: Vec<String> = conn
            .prepare("SELECT key FROM entries ORDER BY key")
            .expect("准备")
            .query_map([], |row| row.get(0))
            .expect("查询")
            .map(|row| row.expect("行"))
            .collect();
        assert_eq!(left, vec!["cold-new", "cold-old", "hot"]);

        // 上限 2：没有过期的了，删命中最少里最旧的
        assert_eq!(evict_over(&conn, 2, "").expect("淘汰"), 1);
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM entries WHERE key = 'cold-old'", [], |row| row.get(0)).expect("计数");
        assert_eq!(count, 0, "命中 0 次且最旧的先走");

        // 没超上限：什么都不删
        assert_eq!(evict_over(&conn, 10, "").expect("淘汰"), 0);
    }

    #[test]
    fn a_new_word_survives_in_a_full_cache_of_hit_entries() {
        let cache = Cache::in_memory().expect("内存库");
        {
            let conn = cache.lock();
            for index in 0..MAX_ENTRIES {
                conn.execute(
                    "INSERT INTO entries (key, word, lang, model, schema_ver, prompt_ver, payload, created_at, hit_count)
                     VALUES (?1, 'w', 'en', 'm', 1, 1, 'p', 1, 5)",
                    params![format!("old{index}")],
                )
                .expect("插入");
            }
        }
        // 缓存满了，而且每条旧的都命中过（hit_count=5）；新写入的 hit_count=0
        cache.put("fresh", "w", "en", "m", "new").expect("写");
        assert_eq!(cache.get("fresh").expect("查").as_deref(), Some("new"), "新词不能被立刻淘汰");
        assert_eq!(cache.count().expect("计数"), MAX_ENTRIES, "总数仍在上限内");
    }

    #[test]
    fn put_stays_under_the_cap() {
        let cache = Cache::in_memory().expect("内存库");
        for index in 0..(MAX_ENTRIES + 25) {
            let key = format!("k{index}");
            cache.put(&key, "w", "en", "m", "p").expect("写");
        }
        assert_eq!(cache.count().expect("计数"), MAX_ENTRIES);
        // 最新写入的永远还在
        let newest = format!("k{}", MAX_ENTRIES + 24);
        assert!(cache.get(&newest).expect("查").is_some());
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
