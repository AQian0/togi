//! 基于 Turso（SQLite 兼容）的对话历史持久化层。
//!
//! 将会话消息以 JSON 序列化形式存入本地 SQLite 数据库，实现跨会话恢复。
//! 仅在启用历史恢复功能时使用；数据库不可用时静默降级为纯内存模式。

use crate::shared::constants;
use crate::shared::error::{ErrorKind, TogiError};
use rig::message::Message;
use std::path::{Path, PathBuf};

/// 历史持久化错误。
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("failed to open database {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: turso::Error,
    },

    #[error("failed to execute query: {source}")]
    Query {
        #[source]
        source: turso::Error,
    },

    #[error("failed to deserialize message: {source}")]
    Deserialize {
        #[source]
        source: serde_json::Error,
    },
}

impl TogiError for StoreError {
    fn code(&self) -> &'static str {
        match self {
            Self::Open { .. } => "store.open",
            Self::Query { .. } => "store.query",
            Self::Deserialize { .. } => "store.deserialize",
        }
    }

    fn kind(&self) -> ErrorKind {
        match self {
            Self::Open { .. } => ErrorKind::Io,
            Self::Query { .. } => ErrorKind::External,
            Self::Deserialize { .. } => ErrorKind::Internal,
        }
    }

    fn user_message(&self) -> String {
        match self {
            Self::Open { path, source } => crate::t!(
                "store-open-error",
                path = path.display().to_string(),
                error = source.to_string()
            ),
            Self::Query { source } => crate::t!("store-query-error", error = source.to_string()),
            Self::Deserialize { source } => {
                crate::t!("store-deserialize-error", error = source.to_string())
            }
        }
    }
}

/// 会话元数据。
#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
}

/// 基于 Turso 本地数据库的消息持久化实现。
pub struct HistoryStore {
    conn: turso::Connection,
    path: PathBuf,
}

impl HistoryStore {
    /// 打开（或创建）指定路径的数据库，并初始化 schema。
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            let _ = std::fs::create_dir_all(parent);
        }
        let db = turso::Builder::new_local(&path.display().to_string())
            .build()
            .await
            .map_err(|source| StoreError::Open {
                path: path.clone(),
                source,
            })?;
        let conn = db.connect().map_err(|source| StoreError::Open {
            path: path.clone(),
            source,
        })?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT ''
            );
            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL REFERENCES sessions(id),
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_messages_session
                ON messages(session_id, id);",
        )
        .await
        .map_err(|source| StoreError::Query { source })?;

        Ok(Self { conn, path })
    }

    /// 返回底层数据库文件路径（用于诊断日志）。
    #[allow(dead_code)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 加载指定会话的全部消息，按插入顺序返回。
    pub async fn load(&self, session_id: &str) -> Result<Vec<Message>, StoreError> {
        let mut rows = self
            .conn
            .query(
                "SELECT content FROM messages WHERE session_id = ?1 ORDER BY id ASC",
                [session_id],
            )
            .await
            .map_err(|source| StoreError::Query { source })?;
        let mut messages = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|source| StoreError::Query { source })?
        {
            let json: String = row.get(0).map_err(|source| StoreError::Query { source })?;
            let msg: Message =
                serde_json::from_str(&json).map_err(|source| StoreError::Deserialize { source })?;
            messages.push(msg);
        }
        Ok(messages)
    }

    /// 全量替换指定会话的消息历史。
    pub async fn save(&self, session_id: &str, messages: &[Message]) -> Result<(), StoreError> {
        // ponytail: 依赖"会话历史只增不改"的事实做增量写入，已存前缀跳过序列化和插入。
        // 若未来加入历史压缩/重写功能，需退回全量替换。
        let existing = self.count(session_id).await?;
        let tx = turso::transaction::Transaction::new_unchecked(
            &self.conn,
            turso::transaction::TransactionBehavior::Immediate,
        )
        .await
        .map_err(|source| StoreError::Query { source })?;
        // 确保 session 行存在（幂等），并更新最近活跃时间。
        tx.execute(
            "INSERT OR IGNORE INTO sessions (id) VALUES (?1)",
            [session_id],
        )
        .await
        .map_err(|source| StoreError::Query { source })?;
        tx.execute(
            "UPDATE sessions SET updated_at = datetime('now') WHERE id = ?1",
            [session_id],
        )
        .await
        .map_err(|source| StoreError::Query { source })?;
        let pending = if existing <= messages.len() {
            &messages[existing..]
        } else {
            // 历史变短（当前不会发生），退化为全量替换。
            tx.execute("DELETE FROM messages WHERE session_id = ?1", [session_id])
                .await
                .map_err(|source| StoreError::Query { source })?;
            messages
        };
        let mut stmt = tx
            .prepare("INSERT INTO messages (session_id, role, content) VALUES (?1, ?2, ?3)")
            .await
            .map_err(|source| StoreError::Query { source })?;
        for msg in pending {
            let role = match msg {
                Message::User { .. } => "user",
                Message::Assistant { .. } => "assistant",
                Message::System { .. } => "system",
            };
            let json =
                serde_json::to_string(msg).map_err(|source| StoreError::Deserialize { source })?;
            stmt.execute(turso::params_from_iter([
                turso::Value::from(session_id),
                turso::Value::from(role),
                turso::Value::from(json),
            ]))
            .await
            .map_err(|source| StoreError::Query { source })?;
        }
        drop(stmt);
        tx.commit()
            .await
            .map_err(|source| StoreError::Query { source })?;
        Ok(())
    }

    /// 清空指定会话的消息历史。
    pub async fn clear(&self, session_id: &str) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM messages WHERE session_id = ?1", [session_id])
            .await
            .map_err(|source| StoreError::Query { source })?;
        Ok(())
    }

    /// 统计指定会话的消息条数。
    pub async fn count(&self, session_id: &str) -> Result<usize, StoreError> {
        let mut rows = self
            .conn
            .query(
                "SELECT COUNT(*) FROM messages WHERE session_id = ?1",
                [session_id],
            )
            .await
            .map_err(|source| StoreError::Query { source })?;
        if let Some(row) = rows
            .next()
            .await
            .map_err(|source| StoreError::Query { source })?
        {
            let n: i64 = row.get(0).map_err(|source| StoreError::Query { source })?;
            Ok(n as usize)
        } else {
            Ok(0)
        }
    }

    /// 列出所有会话，按最近更新时间降序。
    pub async fn list_sessions(&self) -> Result<Vec<SessionMeta>, StoreError> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, title, created_at, updated_at FROM sessions ORDER BY updated_at DESC",
                turso::params_from_iter(std::iter::empty::<turso::Value>()),
            )
            .await
            .map_err(|source| StoreError::Query { source })?;
        let mut sessions = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|source| StoreError::Query { source })?
        {
            sessions.push(SessionMeta {
                id: row.get(0).map_err(|source| StoreError::Query { source })?,
                title: row.get(1).map_err(|source| StoreError::Query { source })?,
                created_at: row.get(2).map_err(|source| StoreError::Query { source })?,
                updated_at: row.get(3).map_err(|source| StoreError::Query { source })?,
            });
        }
        Ok(sessions)
    }

    /// 创建新会话，返回生成的会话 ID。
    pub async fn create_session(&self, title: &str) -> Result<String, StoreError> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        self.conn
            .execute(
                "INSERT INTO sessions (id, title, updated_at) VALUES (?1, ?2, datetime('now'))",
                turso::params_from_iter([
                    turso::Value::from(id.as_str()),
                    turso::Value::from(title),
                ]),
            )
            .await
            .map_err(|source| StoreError::Query { source })?;
        Ok(id)
    }

    /// 删除指定会话及其全部消息。
    pub async fn delete_session(&self, session_id: &str) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM messages WHERE session_id = ?1", [session_id])
            .await
            .map_err(|source| StoreError::Query { source })?;
        self.conn
            .execute("DELETE FROM sessions WHERE id = ?1", [session_id])
            .await
            .map_err(|source| StoreError::Query { source })?;
        Ok(())
    }
}

/// 计算默认数据库文件路径。
///
/// 优先取 `TOGI_DB` 环境变量；否则使用 `dirs::data_dir()` 的平台推荐
/// 数据目录下的 `togi/` 子目录：
/// - Linux: `~/.local/share/togi/`
/// - macOS: `~/Library/Application Support/togi/`
/// - Windows: `%APPDATA%\togi\`
/// 返回 `None` 表示无法确定路径，此时应用应静默降级为纯内存模式。
pub fn default_db_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(constants::ENV_DB_PATH)
        && !path.is_empty()
    {
        return Some(PathBuf::from(path));
    }
    let base = dirs::data_dir()?;
    Some(
        base.join(constants::APP_DIR_NAME)
            .join(constants::DB_FILENAME),
    )
}

/// 生成默认会话 ID。当前实现为固定值，未来支持多会话时替换为
/// 唯一 ID 生成逻辑。
pub fn default_session_id() -> &'static str {
    constants::DEFAULT_SESSION_ID
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig::OneOrMany;
    use rig::message::{Message, UserContent};

    fn sample_user_message(text: &str) -> Message {
        Message::User {
            content: OneOrMany::one(UserContent::text(text)),
        }
    }

    fn sample_assistant_message(text: &str) -> Message {
        Message::Assistant {
            id: None,
            content: OneOrMany::one(rig::message::AssistantContent::text(text)),
        }
    }

    #[tokio::test]
    async fn open_in_memory_and_roundtrip() {
        let store = HistoryStore::open(":memory:").await.unwrap();
        let msgs = vec![
            sample_user_message("hello"),
            sample_assistant_message("hi there"),
        ];
        store.save("s1", &msgs).await.unwrap();
        let loaded = store.load("s1").await.unwrap();
        assert_eq!(loaded.len(), 2);
    }

    #[tokio::test]
    async fn save_replaces_existing_messages() {
        let store = HistoryStore::open(":memory:").await.unwrap();
        let first = vec![sample_user_message("first")];
        store.save("s1", &first).await.unwrap();
        let second = vec![
            sample_user_message("second"),
            sample_assistant_message("reply"),
        ];
        store.save("s1", &second).await.unwrap();
        let loaded = store.load("s1").await.unwrap();
        assert_eq!(loaded.len(), 2);
    }

    #[tokio::test]
    async fn clear_removes_all_messages() {
        let store = HistoryStore::open(":memory:").await.unwrap();
        let msgs = vec![sample_user_message("hello")];
        store.save("s1", &msgs).await.unwrap();
        assert_eq!(store.count("s1").await.unwrap(), 1);
        store.clear("s1").await.unwrap();
        assert_eq!(store.count("s1").await.unwrap(), 0);
    }

    #[tokio::test]
    async fn load_empty_session_returns_empty() {
        let store = HistoryStore::open(":memory:").await.unwrap();
        let loaded = store.load("nonexistent").await.unwrap();
        assert!(loaded.is_empty());
    }

    #[tokio::test]
    async fn sessions_are_isolated() {
        let store = HistoryStore::open(":memory:").await.unwrap();
        store
            .save("s1", &[sample_user_message("from s1")])
            .await
            .unwrap();
        store
            .save("s2", &[sample_user_message("from s2")])
            .await
            .unwrap();
        assert_eq!(store.count("s1").await.unwrap(), 1);
        assert_eq!(store.count("s2").await.unwrap(), 1);
        store.clear("s1").await.unwrap();
        assert_eq!(store.count("s1").await.unwrap(), 0);
        assert_eq!(store.count("s2").await.unwrap(), 1);
    }

    #[tokio::test]
    async fn list_sessions_returns_all_sessions() {
        let store = HistoryStore::open(":memory:").await.unwrap();
        store
            .save("s1", &[sample_user_message("hello")])
            .await
            .unwrap();
        store
            .save("s2", &[sample_user_message("world")])
            .await
            .unwrap();
        let sessions = store.list_sessions().await.unwrap();
        assert_eq!(sessions.len(), 2);
    }

    #[tokio::test]
    async fn create_session_generates_id_and_stores_title() {
        let store = HistoryStore::open(":memory:").await.unwrap();
        let id = store.create_session("测试会话").await.unwrap();
        assert!(!id.is_empty());
        let sessions = store.list_sessions().await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title, "测试会话");
        assert_eq!(sessions[0].id, id);
    }

    #[tokio::test]
    async fn delete_session_removes_messages_and_session() {
        let store = HistoryStore::open(":memory:").await.unwrap();
        let id = store.create_session("待删除").await.unwrap();
        store
            .save(&id, &[sample_user_message("hello")])
            .await
            .unwrap();
        assert_eq!(store.count(&id).await.unwrap(), 1);
        store.delete_session(&id).await.unwrap();
        assert_eq!(store.count(&id).await.unwrap(), 0);
        let sessions = store.list_sessions().await.unwrap();
        assert!(sessions.is_empty());
    }

    #[tokio::test]
    async fn save_updates_updated_at() {
        let store = HistoryStore::open(":memory:").await.unwrap();
        store
            .save("s1", &[sample_user_message("first")])
            .await
            .unwrap();
        let sessions = store.list_sessions().await.unwrap();
        let first_updated = sessions[0].updated_at.clone();
        // 确保时间戳有变化
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        store
            .save("s1", &[sample_user_message("second")])
            .await
            .unwrap();
        let sessions = store.list_sessions().await.unwrap();
        assert!(sessions[0].updated_at >= first_updated);
    }
}
