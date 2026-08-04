//! `search` 工具：对 `/index` 建立的本地代码索引做关键词检索（路线图 §2.1）。
//! 只读；索引不存在或为空时返回可操作的错误提示。

use crate::shared::error::{ErrorKind, TogiError};
use crate::store::HistoryStore;
use rig::tool::{Tool, ToolContext, ToolExecutionError};
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;

pub struct Search {
    store: Arc<HistoryStore>,
}

impl Search {
    pub fn new(store: Arc<HistoryStore>) -> Self {
        Self { store }
    }
}

impl crate::tools::ClassifyEffect for Search {
    fn name() -> &'static str {
        Self::NAME
    }
    fn classify(_args: &serde_json::Value) -> crate::tools::ToolEffect {
        crate::tools::ToolEffect::ReadOnly
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct SearchArgs {
    /// Full-text query in Tantivy syntax: terms are OR-ed by default, use
    /// `AND`/`NOT`, `"exact phrase"`, or `prefix*`. Example: `retry AND backoff`.
    query: String,
    /// Maximum number of chunks to return (1-10). Defaults to 5.
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("`query` must not be empty.")]
    EmptyQuery,
    #[error("index query failed: {0}. If the index is missing, ask the user to run `/index`.")]
    Query(#[from] crate::store::StoreError),
}

impl TogiError for SearchError {
    fn code(&self) -> &'static str {
        match self {
            Self::EmptyQuery => "search.empty_query",
            Self::Query(_) => "search.query",
        }
    }

    fn kind(&self) -> ErrorKind {
        match self {
            Self::EmptyQuery => ErrorKind::InvalidArgument,
            Self::Query(_) => ErrorKind::External,
        }
    }

    fn user_message(&self) -> String {
        match self {
            Self::EmptyQuery => crate::t!("search-empty-query"),
            Self::Query(source) => {
                crate::t!("search-query-error", error = source.user_message())
            }
        }
    }
}

impl Tool for Search {
    const NAME: &'static str = "search";
    type Error = SearchError;
    type Args = SearchArgs;
    type Output = String;

    fn description(&self) -> String {
        "Search the local full-text code index built by the `/index` command \
         (keyword/BM25, not semantic). Returns matching chunks with file path, \
         starting line, and content with matched terms wrapped in `**`. Prefer \
         this over scanning files when looking for where a keyword, identifier, \
         or concept lives in the project. If the result mentions the index is \
         missing or empty, ask the user to run `/index` first."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(SearchArgs)).unwrap()
    }

    fn map_error(&self, error: Self::Error) -> ToolExecutionError {
        match error {
            SearchError::EmptyQuery => ToolExecutionError::invalid_args(error.to_string()),
            SearchError::Query(_) => ToolExecutionError::other(error.to_string()),
        }
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        let query = args.query.trim();
        if query.is_empty() {
            return Err(SearchError::EmptyQuery);
        }
        let limit = args.limit.unwrap_or(5).clamp(1, 10);
        let hits = crate::index::search(self.store.conn(), query, limit).await?;
        if hits.is_empty() {
            return Ok(format!(
                "No matches for `{query}`. The index may be missing or stale — \
                 the user can refresh it with `/index`."
            ));
        }
        Ok(hits
            .iter()
            .map(|h| {
                format!(
                    "=== {}:{} (score {:.2}) ===\n{}",
                    h.path, h.start_line, h.score, h.content
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn search_tool_returns_hits() {
        let base = std::env::temp_dir().join(format!("togi_search_test_{}", uuid::Uuid::new_v4()));
        let root = base.join("root");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("agent.rs"),
            "fn retry() {\n    // retry with exponential backoff\n}\n",
        )
        .unwrap();
        let store = Arc::new(HistoryStore::open(base.join("test.db")).await.unwrap());
        crate::index::index_root(store.conn(), &root).await.unwrap();

        let tool = crate::pipeline::adapt(Search::new(store));
        let out = crate::pipeline::call(&tool, r#"{"query": "retry backoff"}"#)
            .await
            .unwrap();
        assert!(out.contains("agent.rs:1"));
        assert!(out.contains("**backoff**"));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn search_tool_rejects_empty_query() {
        let store = Arc::new(HistoryStore::open(":memory:").await.unwrap());
        let tool = crate::pipeline::adapt(Search::new(store));
        let err = crate::pipeline::call(&tool, r#"{"query": "  "}"#)
            .await
            .unwrap_err();
        assert!(err.contains("must not be empty"));
    }
}
