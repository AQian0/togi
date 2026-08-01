use rig::OneOrMany;
use rig::message::{AssistantContent, Message, UserContent};
use togi::context::ContextCheckpoint;
use togi::store::HistoryStore;

fn user_msg(text: &str) -> Message {
    Message::User {
        content: OneOrMany::one(UserContent::text(text)),
    }
}

fn assistant_msg(text: &str) -> Message {
    Message::Assistant {
        id: None,
        content: OneOrMany::one(AssistantContent::text(text)),
    }
}

fn sample_conversation() -> Vec<Message> {
    vec![
        user_msg("What is Rust?"),
        assistant_msg("Rust is a systems programming language."),
        user_msg("What about ownership?"),
        assistant_msg("Ownership is Rust's memory management model."),
    ]
}

#[tokio::test]
async fn open_and_empty_load() {
    let dir = crate::support::TestDir::new();
    let db_path = dir.path().join("test.db");
    let store = HistoryStore::open(&db_path).await.unwrap();
    let loaded = store.load("default").await.unwrap();
    assert!(loaded.is_empty());
}

#[tokio::test]
async fn save_and_load_roundtrip() {
    let dir = crate::support::TestDir::new();
    let db_path = dir.path().join("test.db");
    let store = HistoryStore::open(&db_path).await.unwrap();
    let msgs = sample_conversation();
    store.save("default", &msgs).await.unwrap();
    let loaded = store.load("default").await.unwrap();
    assert_eq!(loaded.len(), 4);
}

#[tokio::test]
async fn save_replaces_previous_history() {
    let dir = crate::support::TestDir::new();
    let db_path = dir.path().join("test.db");
    let store = HistoryStore::open(&db_path).await.unwrap();

    store.save("default", &[user_msg("first")]).await.unwrap();
    let loaded = store.load("default").await.unwrap();
    assert_eq!(loaded.len(), 1);

    let new_msgs = sample_conversation();
    store.save("default", &new_msgs).await.unwrap();
    let loaded = store.load("default").await.unwrap();
    assert_eq!(loaded.len(), 4);
}

#[tokio::test]
async fn save_with_shorter_history_falls_back_to_full_replace() {
    let dir = crate::support::TestDir::new();
    let db_path = dir.path().join("test.db");
    let store = HistoryStore::open(&db_path).await.unwrap();

    store.save("default", &sample_conversation()).await.unwrap();
    assert_eq!(store.count("default").await.unwrap(), 4);

    store.save("default", &[user_msg("only")]).await.unwrap();
    let loaded = store.load("default").await.unwrap();
    assert_eq!(loaded.len(), 1);
}

#[tokio::test]
async fn clear_removes_all_messages() {
    let dir = crate::support::TestDir::new();
    let db_path = dir.path().join("test.db");
    let store = HistoryStore::open(&db_path).await.unwrap();
    store.save("default", &sample_conversation()).await.unwrap();
    assert_eq!(store.count("default").await.unwrap(), 4);

    store.clear("default").await.unwrap();
    assert_eq!(store.count("default").await.unwrap(), 0);
    let loaded = store.load("default").await.unwrap();
    assert!(loaded.is_empty());
}

#[tokio::test]
async fn reopen_and_load_persisted_history() {
    let dir = crate::support::TestDir::new();
    let db_path = dir.path().join("test.db");

    // 第一次打开并写入
    {
        let store = HistoryStore::open(&db_path).await.unwrap();
        store.save("default", &sample_conversation()).await.unwrap();
    }

    // 重新打开并验证持久化
    {
        let store = HistoryStore::open(&db_path).await.unwrap();
        let loaded = store.load("default").await.unwrap();
        assert_eq!(loaded.len(), 4);
        assert!(matches!(loaded[0], Message::User { .. }));
        assert!(matches!(loaded[1], Message::Assistant { .. }));
    }
}

#[tokio::test]
async fn sessions_are_isolated() {
    let dir = crate::support::TestDir::new();
    let db_path = dir.path().join("test.db");
    let store = HistoryStore::open(&db_path).await.unwrap();

    store.save("s1", &[user_msg("from s1")]).await.unwrap();
    store.save("s2", &[user_msg("from s2")]).await.unwrap();
    assert_eq!(store.count("s1").await.unwrap(), 1);
    assert_eq!(store.count("s2").await.unwrap(), 1);

    store.clear("s1").await.unwrap();
    assert_eq!(store.count("s1").await.unwrap(), 0);
    assert_eq!(store.count("s2").await.unwrap(), 1);
}

#[tokio::test]
async fn count_empty_session_returns_zero() {
    let dir = crate::support::TestDir::new();
    let db_path = dir.path().join("test.db");
    let store = HistoryStore::open(&db_path).await.unwrap();
    assert_eq!(store.count("nonexistent").await.unwrap(), 0);
}

// ── 上下文 checkpoint ───────────────────────────────────────────────

#[tokio::test]
async fn context_checkpoint_roundtrip() {
    let dir = crate::support::TestDir::new();
    let db_path = dir.path().join("test.db");
    let store = HistoryStore::open(&db_path).await.unwrap();
    store.save("default", &sample_conversation()).await.unwrap();

    assert_eq!(
        store.load_context("default").await.unwrap(),
        ContextCheckpoint::default()
    );
    let checkpoint = ContextCheckpoint {
        summary: Some("前半部分摘要".into()),
        covered_messages: 2,
    };
    store.save_context("default", &checkpoint).await.unwrap();
    assert_eq!(store.load_context("default").await.unwrap(), checkpoint);
}

#[tokio::test]
async fn context_checkpoint_survives_reopen() {
    let dir = crate::support::TestDir::new();
    let db_path = dir.path().join("test.db");
    let checkpoint = ContextCheckpoint {
        summary: Some("重启前的摘要".into()),
        covered_messages: 2,
    };
    {
        let store = HistoryStore::open(&db_path).await.unwrap();
        store.save("default", &sample_conversation()).await.unwrap();
        store.save_context("default", &checkpoint).await.unwrap();
    }
    {
        // 重新打开（旧数据库自动创建新表）后 checkpoint 与 transcript 都在
        let store = HistoryStore::open(&db_path).await.unwrap();
        assert_eq!(store.load_context("default").await.unwrap(), checkpoint);
        assert_eq!(store.load("default").await.unwrap().len(), 4);
    }
}

#[tokio::test]
async fn summary_does_not_change_transcript() {
    let dir = crate::support::TestDir::new();
    let db_path = dir.path().join("test.db");
    let store = HistoryStore::open(&db_path).await.unwrap();
    let msgs = sample_conversation();
    store.save("default", &msgs).await.unwrap();
    let before = store.load("default").await.unwrap();
    store
        .save_context(
            "default",
            &ContextCheckpoint {
                summary: Some("摘要".into()),
                covered_messages: 3,
            },
        )
        .await
        .unwrap();
    // 完整 transcript 不因摘要 checkpoint 改变
    let after = store.load("default").await.unwrap();
    assert_eq!(before, after);
    assert_eq!(after.len(), 4);
}

#[tokio::test]
async fn context_checkpoints_are_isolated_between_sessions() {
    let dir = crate::support::TestDir::new();
    let db_path = dir.path().join("test.db");
    let store = HistoryStore::open(&db_path).await.unwrap();
    store.save("s1", &sample_conversation()).await.unwrap();
    store.save("s2", &sample_conversation()).await.unwrap();
    store
        .save_context(
            "s1",
            &ContextCheckpoint {
                summary: Some("s1 摘要".into()),
                covered_messages: 2,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store.load_context("s2").await.unwrap(),
        ContextCheckpoint::default()
    );
    assert_eq!(store.load_context("s1").await.unwrap().covered_messages, 2);
}

#[tokio::test]
async fn clear_context_removes_checkpoint_but_keeps_messages() {
    let dir = crate::support::TestDir::new();
    let db_path = dir.path().join("test.db");
    let store = HistoryStore::open(&db_path).await.unwrap();
    store.save("default", &sample_conversation()).await.unwrap();
    store
        .save_context(
            "default",
            &ContextCheckpoint {
                summary: Some("摘要".into()),
                covered_messages: 2,
            },
        )
        .await
        .unwrap();
    store.clear_context("default").await.unwrap();
    assert_eq!(
        store.load_context("default").await.unwrap(),
        ContextCheckpoint::default()
    );
    assert_eq!(store.count("default").await.unwrap(), 4);
}

#[tokio::test]
async fn delete_session_removes_context_checkpoint() {
    let dir = crate::support::TestDir::new();
    let db_path = dir.path().join("test.db");
    let store = HistoryStore::open(&db_path).await.unwrap();
    let id = store.create_session("待删除").await.unwrap();
    store.save(&id, &[user_msg("hello")]).await.unwrap();
    store
        .save_context(
            &id,
            &ContextCheckpoint {
                summary: Some("摘要".into()),
                covered_messages: 1,
            },
        )
        .await
        .unwrap();
    store.delete_session(&id).await.unwrap();
    assert_eq!(
        store.load_context(&id).await.unwrap(),
        ContextCheckpoint::default()
    );
}
