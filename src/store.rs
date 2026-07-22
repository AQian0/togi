//! 基于 Turso（SQLite 兼容）的对话历史持久化层。
//!
//! 将会话消息以 JSON 序列化形式存入本地 SQLite 数据库，实现跨会话恢复。
//! 仅在启用历史恢复功能时使用；数据库不可用时静默降级为纯内存模式。
//!
//! 消息 `content` 列采用带版本的 envelope：`{"v":1,"data":<Message>}`。
//! 无版本字段的行按 v0（裸 Message，早期版本写入）兼容解析。某行无法
//! 解码时（rig 格式漂移或更新版本写入）加载截断至该行之前，保留可解码
//! 前缀并报告丢弃行数，而不是整段失败。

use crate::shared::constants;
use crate::shared::error::{ErrorKind, TogiError};
use rig::message::{AssistantContent, Message};
use std::path::{Path, PathBuf};

/// 消息 `content` 列的当前持久化格式版本。
const MESSAGE_SCHEMA_VERSION: u64 = 1;

/// [`HistoryStore::load`] 的结果：成功解码的消息前缀 + 因无法解码
/// （版本不兼容或数据损坏）而被跳过的行数。
#[derive(Debug, Default)]
pub struct LoadReport {
    pub messages: Vec<Message>,
    pub dropped_rows: usize,
}

/// 编码一条消息为带版本的 envelope。
fn encode_message(msg: &Message) -> Result<String, serde_json::Error> {
    let data = serde_json::to_value(msg)?;
    serde_json::to_string(&serde_json::json!({
        "v": MESSAGE_SCHEMA_VERSION,
        "data": data,
    }))
}

/// 解码一行 `content`：含版本字段按 envelope 解析（仅接受当前版本，
/// 更高版本视为不可解码）；无版本字段按 v0 裸 Message 兼容解析。
/// 返回 `None` 表示该行无法解码。
fn decode_message(raw: &str) -> Option<Message> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    if let Some(v) = value.get("v").and_then(serde_json::Value::as_u64) {
        if v != MESSAGE_SCHEMA_VERSION {
            return None;
        }
        return serde_json::from_value(value.get("data")?.clone()).ok();
    }
    serde_json::from_value(value).ok()
}

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
                ON messages(session_id, id);
            CREATE TABLE IF NOT EXISTS context_checkpoints (
                session_id TEXT PRIMARY KEY,
                summary TEXT NOT NULL,
                covered_messages INTEGER NOT NULL,
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .await
        .map_err(|source| StoreError::Query { source })?;

        Ok(Self { conn })
    }

    /// 加载指定会话的消息历史，按插入顺序返回。
    ///
    /// 某行无法解码时截断至该行之前（后续行可能依赖被跳过的工具往返，
    /// 继续加载会产出语义断裂的历史），并剥离末尾悬空的工具调用；
    /// 调用方应在 `dropped_rows > 0` 时向用户提示。
    pub async fn load(&self, session_id: &str) -> Result<LoadReport, StoreError> {
        let mut rows = self
            .conn
            .query(
                "SELECT content FROM messages WHERE session_id = ?1 ORDER BY id ASC",
                [session_id],
            )
            .await
            .map_err(|source| StoreError::Query { source })?;
        let mut messages = Vec::new();
        let mut dropped_rows = 0;
        while let Some(row) = rows
            .next()
            .await
            .map_err(|source| StoreError::Query { source })?
        {
            let raw: String = row.get(0).map_err(|source| StoreError::Query { source })?;
            match decode_message(&raw) {
                Some(msg) => messages.push(msg),
                // 截断：后续行可能依赖被跳过的工具往返。
                None => {
                    dropped_rows = self.count(session_id).await.unwrap_or(messages.len()) - messages.len();
                    break;
                }
            }
        }
        strip_dangling_tool_calls(&mut messages);
        Ok(LoadReport {
            messages,
            dropped_rows,
        })
    }

    /// 全量替换指定会话的消息历史。
    pub async fn save(&self, session_id: &str, messages: &[Message]) -> Result<(), StoreError> {
        // ponytail: 依赖"会话历史只增不改"的常态做增量写入，已存前缀跳过序列化和插入。
        // 历史变短（加载截断不可解码行、或未来的历史重写功能）时退化为全量替换。
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
            // 历史变短（例如加载时截断了不可解码行），退化为全量替换：
            // 不可解码的行对本版本已不可读，随替换清除，DB 与内存收敛。
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
            let json = encode_message(msg).map_err(|source| StoreError::Deserialize { source })?;
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
        let covered = usize::try_from(covered).unwrap_or(0);
        if covered > self.count(session_id).await? {
            return Ok(crate::context::ContextCheckpoint::default());
        }
        Ok(crate::context::ContextCheckpoint {
            summary: (!summary.is_empty()).then_some(summary),
            covered_messages: covered,
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
                "INSERT INTO context_checkpoints (session_id, summary, covered_messages, updated_at)
                 VALUES (?1, ?2, ?3, datetime('now'))
                 ON CONFLICT(session_id) DO UPDATE SET
                     summary = ?2, covered_messages = ?3, updated_at = datetime('now')",
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
/// 优先取 `TOGI_DB` 环境变量；否则使用 `dirs::data_dir()` 的平台推荐
/// 数据目录下的 `togi/` 子目录：
/// - Linux: `~/.local/share/togi/`
/// - macOS: `~/Library/Application Support/togi/`
/// - Windows: `%APPDATA%\togi\`
///
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
        assert_eq!(loaded.messages.len(), 2);
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
        assert_eq!(loaded.messages.len(), 2);
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
        assert!(loaded.messages.is_empty());
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
    async fn context_checkpoint_invalidated_when_covered_exceeds_messages() {
        use crate::context::ContextCheckpoint;
        let store = HistoryStore::open(":memory:").await.unwrap();
        store
            .save("s1", &[sample_user_message("only")])
            .await
            .unwrap();
        store
            .save_context(
                "s1",
                &ContextCheckpoint {
                    summary: Some("stale".into()),
                    covered_messages: 5,
                },
            )
            .await
            .unwrap();
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

    // ── 消息格式版本化与容错加载 ─────────────────────────────────

    use rig::message::{
        AssistantContent, Text, ToolCall, ToolFunction, ToolResult, ToolResultContent,
    };

    fn assistant_tool_call_message() -> Message {
        Message::Assistant {
            id: None,
            content: OneOrMany::one(AssistantContent::ToolCall(ToolCall::new(
                "c1".into(),
                ToolFunction::new("shell".into(), serde_json::json!({})),
            ))),
        }
    }

    /// 直接向 messages 表写入一行原始 content（绕过 encode）。
    async fn insert_raw(store: &HistoryStore, session_id: &str, role: &str, content: &str) {
        store
            .conn
            .execute(
                "INSERT INTO messages (session_id, role, content) VALUES (?1, ?2, ?3)",
                turso::params_from_iter([
                    turso::Value::from(session_id),
                    turso::Value::from(role),
                    turso::Value::from(content),
                ]),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn legacy_bare_message_loads_as_v0() {
        let store = HistoryStore::open(":memory:").await.unwrap();
        let raw = serde_json::to_string(&sample_user_message("old")).unwrap();
        insert_raw(&store, "s1", "user", &raw).await;
        let report = store.load("s1").await.unwrap();
        assert_eq!(report.messages.len(), 1);
        assert_eq!(report.dropped_rows, 0);
    }

    #[tokio::test]
    async fn undecodable_row_truncates_load_with_dropped_count() {
        // 数据损坏与未知未来版本走同一截断分支
        for bad_row in ["not json at all", r#"{"v":99,"data":{}}"#] {
            let store = HistoryStore::open(":memory:").await.unwrap();
            store
                .save(
                    "s1",
                    &[sample_user_message("a"), sample_assistant_message("b")],
                )
                .await
                .unwrap();
            insert_raw(&store, "s1", "assistant", bad_row).await;
            let report = store.load("s1").await.unwrap();
            assert_eq!(report.messages.len(), 2, "row: {bad_row}");
            assert_eq!(report.dropped_rows, 1, "row: {bad_row}");
        }
    }

    #[tokio::test]
    async fn truncation_strips_dangling_tool_call() {
        let store = HistoryStore::open(":memory:").await.unwrap();
        store
            .save(
                "s1",
                &[sample_user_message("run it"), assistant_tool_call_message()],
            )
            .await
            .unwrap();
        // 截断点在 tool call 与 result 之间：悬空调用必须剥离
        insert_raw(&store, "s1", "user", "corrupt result row").await;
        let report = store.load("s1").await.unwrap();
        assert_eq!(report.messages.len(), 1);
        assert_eq!(report.dropped_rows, 1);
        assert!(matches!(&report.messages[0], Message::User { .. }));
    }

    #[tokio::test]
    async fn envelope_roundtrip_preserves_tool_pairs() {
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
        // 写入格式为带版本 envelope
        let mut rows = store
            .conn
            .query(
                "SELECT content FROM messages WHERE session_id = 's1'",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        let raw: String = row.get(0).unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value.get("v").and_then(|v| v.as_u64()), Some(1));
        assert!(value.get("data").is_some());
        // 读回内容保真
        let report = store.load("s1").await.unwrap();
        // rig 的 serde 实现将 None 序列化为 {}、再读回为 Some({})，
        // 精确等值不成立；按序列化形式比较（落盘的本来就是序列化形式）。
        assert_eq!(
            serde_json::to_value(&report.messages).unwrap(),
            serde_json::to_value(&msgs).unwrap()
        );
        assert_eq!(report.dropped_rows, 0);
    }
}
