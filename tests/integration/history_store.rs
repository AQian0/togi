use rig::message::{AssistantContent, Message, UserContent};
use rig::OneOrMany;
use togi::store::{HistoryStore, MessageStore};

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