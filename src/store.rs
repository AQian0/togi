//! 基于 Turso（SQLite 兼容）的对话历史持久化层。
//!
//! 将会话消息以 JSON 序列化形式存入本地 SQLite 数据库，实现跨会话恢复。
//! 仅在启用历史恢复功能时使用；数据库不可用时静默降级为纯内存模式。
//!
//! 消息 `content` 列直接存 `Message` 的 JSON 序列化形式。
//! 某行无法解码时（rig 格式漂移或数据损坏），该会话历史整体作废清除——
//! 本地历史是可弃缓存，不保留语义可疑的前缀。

use crate::shared::constants;
use crate::shared::error::{ErrorKind, TogiError};
use rig::message::{AssistantContent, Message};
use std::path::{Path, PathBuf};

/// 剥离历史末尾的悬空工具调用：截断点可能落在 assistant tool call 与
/// 对应 result 之间，多数 provider 拒绝存在无 result 的 tool call 的历史。
/// 规则与 `agent::PartialTurn::finish` 一致。
fn strip_dangling_tool_calls(messages: &mut Vec<Message>) {
    while matches!(
        messages.last(),
        Some(Message::Assistant { content, .. })
            if content.iter().any(|c| matches!(c, AssistantContent::ToolCall(_)))
    ) {
        messages.pop();
    }
}

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
    pub updated_at: String,
}

/// 基于 Turso 本地数据库的消息持久化实现。
pub struct HistoryStore {
    conn: turso::Connection,
}

impl HistoryStore {
    /// 打开（或创建）指定路径的数据库，并初始化 schema。
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            let _ = std::fs::create_dir_all(parent);
        }
        let db = turso::Builder::new_local(&path.display().to_string())
            // 原生 FTS 索引（路线图 §2.1）需要打开实验索引方法开关。
            .experimental_index_method(true)
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
                updated_at TEXT NOT NULL DEFAULT ''
            );
            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL REFERENCES sessions(id),
                content TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_messages_session
                ON messages(session_id, id);
            CREATE TABLE IF NOT EXISTS context_checkpoints (
                session_id TEXT PRIMARY KEY,
                summary TEXT NOT NULL,
                covered_messages INTEGER NOT NULL
            );",
        )
        .await
        .map_err(|source| StoreError::Query { source })?;

        // 索引 schema（路线图 §2.1）。FTS 为实验特性：创建失败仅降级检索
        // 能力（search 查询时报错），不影响会话历史持久化。
        if let Err(err) = conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS indexed_files (
                    path TEXT PRIMARY KEY,
                    hash TEXT NOT NULL,
                    mtime INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS chunks (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    path TEXT NOT NULL,
                    start_line INTEGER NOT NULL,
                    content TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS chunks_fts ON chunks USING fts (content);",
            )
            .await
        {
            tracing::warn!(error = %err, "index schema init failed, search degraded");
        }

        // 旧版表（含 role 列的 envelope 时代）直接重建：本地历史是可弃缓存，
        // 不做原地迁移；旧数据本就解码不了，留着只会让 INSERT 违反 NOT NULL。
        let mut rows = conn
            .query("PRAGMA table_info(messages)", ())
            .await
            .map_err(|source| StoreError::Query { source })?;
        let mut legacy = false;
        while let Some(row) = rows
            .next()
            .await
            .map_err(|source| StoreError::Query { source })?
        {
            let name: String = row.get(1).map_err(|source| StoreError::Query { source })?;
            if name == "role" {
                legacy = true;
                break;
            }
        }
        if legacy {
            conn.execute_batch(
                "DROP TABLE messages;
                 CREATE TABLE messages (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    session_id TEXT NOT NULL REFERENCES sessions(id),
                    content TEXT NOT NULL
                 );
                 CREATE INDEX idx_messages_session ON messages(session_id, id);",
            )
            .await
            .map_err(|source| StoreError::Query { source })?;
        }

        Ok(Self { conn })
    }

    /// 暴露底层连接，供索引 / 检索（`crate::index`）直接执行 SQL。
    pub(crate) fn conn(&self) -> &turso::Connection {
        &self.conn
    }

    /// 加载指定会话的消息历史，按插入顺序返回。
    ///
    /// 任一行无法解码（rig 格式漂移或数据损坏）时，该会话历史整体作废并
    /// 清除；否则剥离末尾悬空的工具调用（取消中途落盘所致）。
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
            let raw: String = row.get(0).map_err(|source| StoreError::Query { source })?;
            match serde_json::from_str(&raw) {
                Ok(msg) => messages.push(msg),
                Err(_) => {
                    self.clear(session_id).await?;
                    return Ok(Vec::new());
                }
            }
        }
        strip_dangling_tool_calls(&mut messages);
        Ok(messages)
    }

    /// 全量替换指定会话的消息历史。
    pub async fn save(&self, session_id: &str, messages: &[Message]) -> Result<(), StoreError> {
        // ponytail: 依赖“会话历史只增不改”的常态做增量写入，已存前缀跳过序列化和插入。
        // 历史变短（多实例写同一 DB 等）时退化为全量替换，避免增量路径静默漏写。
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
            tx.execute("DELETE FROM messages WHERE session_id = ?1", [session_id])
                .await
                .map_err(|source| StoreError::Query { source })?;
            messages
        };
        let mut stmt = tx
            .prepare("INSERT INTO messages (session_id, content) VALUES (?1, ?2)")
            .await
            .map_err(|source| StoreError::Query { source })?;
        for msg in pending {
            let json =
                serde_json::to_string(msg).map_err(|source| StoreError::Deserialize { source })?;
            stmt.execute(turso::params_from_iter([
                turso::Value::from(session_id),
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

    /// 加载指定会话的上下文 checkpoint。
    ///
    /// checkpoint 是可重建的派生数据：覆盖位置超过当前消息数量时
    /// （例如历史被外部改写）自动失效，返回空 checkpoint。
    pub async fn load_context(
        &self,
        session_id: &str,
    ) -> Result<crate::context::ContextCheckpoint, StoreError> {
        let mut rows = self
            .conn
            .query(
                "SELECT summary, covered_messages FROM context_checkpoints \
                 WHERE session_id = ?1",
                [session_id],
            )
            .await
            .map_err(|source| StoreError::Query { source })?;
        let Some(row) = rows
            .next()
            .await
            .map_err(|source| StoreError::Query { source })?
        else {
            return Ok(crate::context::ContextCheckpoint::default());
        };
        let summary: String = row.get(0).map_err(|source| StoreError::Query { source })?;
        let covered: i64 = row.get(1).map_err(|source| StoreError::Query { source })?;
        Ok(crate::context::ContextCheckpoint {
            summary: (!summary.is_empty()).then_some(summary),
            covered_messages: usize::try_from(covered).unwrap_or(0),
        })
    }

    /// 保存（upsert）指定会话的上下文 checkpoint。应在消息保存成功后调用。
    pub async fn save_context(
        &self,
        session_id: &str,
        checkpoint: &crate::context::ContextCheckpoint,
    ) -> Result<(), StoreError> {
        self.conn
            .execute(
                "INSERT INTO context_checkpoints (session_id, summary, covered_messages)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(session_id) DO UPDATE SET
                     summary = ?2, covered_messages = ?3",
                turso::params_from_iter([
                    turso::Value::from(session_id),
                    turso::Value::from(checkpoint.summary.clone().unwrap_or_default()),
                    turso::Value::from(checkpoint.covered_messages as i64),
                ]),
            )
            .await
            .map_err(|source| StoreError::Query { source })?;
        Ok(())
    }

    /// 删除指定会话的上下文 checkpoint。
    pub async fn clear_context(&self, session_id: &str) -> Result<(), StoreError> {
        self.conn
            .execute(
                "DELETE FROM context_checkpoints WHERE session_id = ?1",
                [session_id],
            )
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
                "SELECT id, title, updated_at FROM sessions ORDER BY updated_at DESC",
                (),
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
                updated_at: row.get(2).map_err(|source| StoreError::Query { source })?,
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

    /// 删除指定会话及其全部消息与上下文 checkpoint。
    pub async fn delete_session(&self, session_id: &str) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM messages WHERE session_id = ?1", [session_id])
            .await
            .map_err(|source| StoreError::Query { source })?;
        self.conn
            .execute(
                "DELETE FROM context_checkpoints WHERE session_id = ?1",
                [session_id],
            )
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
/// 优先取 `TOGI_DB` 环境变量（全局单一数据库，旧行为）；否则按当前工作
/// 目录分别存放（与 Pi 的 `sessions/<编码cwd>/` 一致，每个项目独立会话列表）：
/// - Linux: `~/.local/share/togi/<编码cwd>/togi.db`
/// - macOS: `~/Library/Application Support/togi/<编码cwd>/togi.db`
/// - Windows: `%APPDATA%\togi\<编码cwd>\togi.db`
///
/// 返回 `None` 表示无法确定路径，此时应用应静默降级为纯内存模式。
pub fn default_db_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(constants::ENV_DB_PATH)
        && !path.is_empty()
    {
        return Some(PathBuf::from(path));
    }
    let base = dirs::data_dir()?;
    let cwd = std::env::current_dir().ok()?;
    Some(
        base.join(constants::APP_DIR_NAME)
            .join(encode_cwd(&cwd.to_string_lossy()))
            .join(constants::DB_FILENAME),
    )
}

/// 将工作目录编码为目录名：非字母数字字符一律替换为 `-`（同 Pi 的编码风格）。
/// ponytail: `/a-b` 与 `/a/b` 编码相同会共享数据库，概率极低，撞上了用 TOGI_DB 分开。
fn encode_cwd(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect()
}


#[cfg(test)]
mod tests {
    #[test]
    fn encode_cwd_replaces_separators() {
        assert_eq!(super::encode_cwd("/Users/aqian/togi"), "-Users-aqian-togi");
        assert_eq!(super::encode_cwd("C:\\code\\my proj"), "C--code-my-proj");
    }

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

    #[tokio::test]
    async fn context_checkpoint_roundtrip() {
        use crate::context::ContextCheckpoint;
        let store = HistoryStore::open(":memory:").await.unwrap();
        assert_eq!(
            store.load_context("s1").await.unwrap(),
            ContextCheckpoint::default()
        );
        store
            .save("s1", &[sample_user_message("a"), sample_user_message("b")])
            .await
            .unwrap();
        let checkpoint = ContextCheckpoint {
            summary: Some("摘要".into()),
            covered_messages: 1,
        };
        store.save_context("s1", &checkpoint).await.unwrap();
        assert_eq!(store.load_context("s1").await.unwrap(), checkpoint);
        // upsert 覆盖
        let updated = ContextCheckpoint {
            summary: Some("新摘要".into()),
            covered_messages: 2,
        };
        store.save_context("s1", &updated).await.unwrap();
        assert_eq!(store.load_context("s1").await.unwrap(), updated);
        // clear 删除
        store.clear_context("s1").await.unwrap();
        assert_eq!(
            store.load_context("s1").await.unwrap(),
            ContextCheckpoint::default()
        );
    }

    #[tokio::test]
    async fn delete_session_removes_context_checkpoint() {
        use crate::context::ContextCheckpoint;
        let store = HistoryStore::open(":memory:").await.unwrap();
        store.save("s1", &[sample_user_message("a")]).await.unwrap();
        store
            .save_context(
                "s1",
                &ContextCheckpoint {
                    summary: Some("s".into()),
                    covered_messages: 1,
                },
            )
            .await
            .unwrap();
        store.delete_session("s1").await.unwrap();
        assert_eq!(
            store.load_context("s1").await.unwrap(),
            ContextCheckpoint::default()
        );
    }

    // ── 容错加载 ─────────────────────────────────────────────────────

    use rig::message::{Text, ToolCall, ToolFunction, ToolResult, ToolResultContent};

    fn assistant_tool_call_message() -> Message {
        Message::Assistant {
            id: None,
            content: OneOrMany::one(AssistantContent::ToolCall(ToolCall::new(
                "c1".into(),
                ToolFunction::new("shell".into(), serde_json::json!({})),
            ))),
        }
    }

    /// 直接向 messages 表写入一行原始 content（绕过正常序列化）。
    async fn insert_raw(store: &HistoryStore, session_id: &str, content: &str) {
        store
            .conn
            .execute(
                "INSERT INTO messages (session_id, content) VALUES (?1, ?2)",
                turso::params_from_iter([
                    turso::Value::from(session_id),
                    turso::Value::from(content),
                ]),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn undecodable_row_wipes_session() {
        let store = HistoryStore::open(":memory:").await.unwrap();
        store
            .save(
                "s1",
                &[sample_user_message("a"), sample_assistant_message("b")],
            )
            .await
            .unwrap();
        insert_raw(&store, "s1", "not json at all").await;
        assert!(store.load("s1").await.unwrap().is_empty());
        assert_eq!(store.count("s1").await.unwrap(), 0);
    }

    /// 旧版表（含 role 列）打开时重建，历史清空但写入恢复正常。
    #[tokio::test]
    async fn legacy_role_table_is_rebuilt() {
        let dir = std::env::temp_dir().join(format!("togi_store_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.db");
        let db = turso::Builder::new_local(&path.display().to_string())
            .build()
            .await
            .unwrap();
        let conn = db.connect().unwrap();
        conn.execute_batch(
            "CREATE TABLE messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL
            );",
        )
        .await
        .unwrap();
        drop(conn);
        drop(db);

        let store = HistoryStore::open(&path).await.unwrap();
        store.save("s1", &[sample_user_message("new")]).await.unwrap();
        assert_eq!(store.load("s1").await.unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn dangling_tool_call_stripped_on_load() {
        // 取消中途落盘可能留下末尾悬空 tool call，加载时剥离
        let store = HistoryStore::open(":memory:").await.unwrap();
        store
            .save(
                "s1",
                &[sample_user_message("run it"), assistant_tool_call_message()],
            )
            .await
            .unwrap();
        let messages = store.load("s1").await.unwrap();
        assert_eq!(messages.len(), 1);
        assert!(matches!(&messages[0], Message::User { .. }));
    }

    #[tokio::test]
    async fn roundtrip_preserves_tool_pairs() {
        let store = HistoryStore::open(":memory:").await.unwrap();
        let msgs = vec![
            sample_user_message("run it"),
            assistant_tool_call_message(),
            Message::User {
                content: OneOrMany::one(rig::message::UserContent::ToolResult(ToolResult {
                    id: "c1".into(),
                    call_id: None,
                    content: OneOrMany::one(ToolResultContent::Text(Text::new("done"))),
                })),
            },
        ];
        store.save("s1", &msgs).await.unwrap();
        let loaded = store.load("s1").await.unwrap();
        // rig 的 serde 实现将 None 序列化为 {}、再读回为 Some({})，
        // 精确等值不成立；按序列化形式比较（落盘的本来就是序列化形式）。
        assert_eq!(
            serde_json::to_value(&loaded).unwrap(),
            serde_json::to_value(&msgs).unwrap()
        );
    }
}
