//! 代码库关键词索引（路线图 §2.1）：遍历 → 哈希去重 → 分块入库，
//! 检索走 Turso 原生 FTS（Tantivy 实现，`fts_match` 过滤 + `fts_score` BM25 排序）。
//! 零新依赖、确定性、可单测。

use crate::shared::util::is_binary;
use crate::store::StoreError;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// 每块行数；步进小于块长，重叠部分避免关键词落在切块边界两侧。
const CHUNK_LINES: usize = 100;
const CHUNK_STEP: usize = 80;
/// 超过该大小的文件不索引（生成物、打包物）。ponytail: 超限文件即使已在
/// 索引中也不清除旧分块，1MB 边界来回跨越的情况极少，撞上了删库重建即可。
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// 不进入索引的目录名。ponytail: 不解析 .gitignore（零依赖），
/// 用固定名单挡住最常见的噪声目录。
const SKIP_DIRS: &[&str] = &[".git", ".jj", "target", "node_modules"];

/// 一次索引运行的统计。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct IndexStats {
    /// 遍历到的候选文件数。
    pub scanned: usize,
    /// 内容变化、重新分块入库的文件数。
    pub updated: usize,
    /// mtime+hash 未变、跳过的文件数。
    pub skipped: usize,
    /// 已从磁盘消失、从索引移除的文件数。
    pub removed: usize,
    /// 本次写入的分块总数。
    pub chunks: usize,
}

/// 一条检索命中。
#[derive(Debug)]
pub struct Hit {
    pub path: String,
    pub start_line: i64,
    /// BM25 得分，越小越相关。
    pub score: f64,
    /// 命中词已被 `**` 包裹的分块内容。
    pub content: String,
}

/// 将文本按行切块，返回 `(起始行号（1-based), 块内容)` 列表。
pub(crate) fn chunk_text(text: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    loop {
        let end = (start + CHUNK_LINES).min(lines.len());
        chunks.push((start + 1, lines[start..end].join("\n")));
        if end == lines.len() {
            break;
        }
        start += CHUNK_STEP;
    }
    chunks
}

/// 递归收集 `root` 下的普通文件（不跟随符号链接，跳过名单目录）。
fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                if !SKIP_DIRS.contains(&entry.file_name().to_string_lossy().as_ref()) {
                    stack.push(entry.path());
                }
            } else if ft.is_file() {
                out.push(entry.path());
            }
        }
    }
    out.sort();
    out
}

fn hash_bytes(bytes: &[u8]) -> String {
    // ponytail: DefaultHasher 非密码学哈希，本地去重够用，碰撞概率可忽略。
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish().to_string()
}

/// 单条 SQL 中的最大行数 / 参数组数。
/// turso 对 chunks 的每条写语句触发一次 Tantivy flush，必须批量：
/// 逐行插入 400 块约 26s，100 行/条降至 ~0.1s。
const BATCH_ROWS: usize = 100;

/// 内容变化、需重新分块入库的文件：(路径, hash, mtime, 分块列表)。
type FileUpdate = (String, String, i64, Vec<(usize, String)>);

/// 增量索引 `root` 目录：变更文件重新分块，消失文件清除，未变文件跳过。
pub async fn index_root(conn: &turso::Connection, root: &Path) -> Result<IndexStats, StoreError> {
    let mut stats = IndexStats::default();
    let files = walk(root);
    // 已索引状态：path → (hash, mtime)
    let mut known: HashMap<String, (String, i64)> = HashMap::new();
    let mut rows = conn
        .query("SELECT path, hash, mtime FROM indexed_files", ())
        .await
        .map_err(|source| StoreError::Query { source })?;
    while let Some(row) = rows
        .next()
        .await
        .map_err(|source| StoreError::Query { source })?
    {
        known.insert(
            row.get::<String>(0)
                .map_err(|source| StoreError::Query { source })?,
            (
                row.get::<String>(1)
                    .map_err(|source| StoreError::Query { source })?,
                row.get::<i64>(2)
                    .map_err(|source| StoreError::Query { source })?,
            ),
        );
    }

    // 第一遍只做文件系统操作与 diff，不写库：
    // 收集 touch（仅 mtime 变）、update（内容变）、remove（已消失）三组动作。
    let mut seen: HashSet<String> = HashSet::new();
    let mut touch: Vec<(String, i64)> = Vec::new();
    let mut update: Vec<FileUpdate> = Vec::new();
    for file in files {
        let Ok(rel) = file.strip_prefix(root) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        stats.scanned += 1;
        seen.insert(rel.clone());
        let Ok(meta) = std::fs::metadata(&file) else {
            continue;
        };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        if meta.len() > MAX_FILE_BYTES {
            continue;
        }
        if known
            .get(&rel)
            .is_some_and(|(_, old_mtime)| *old_mtime == mtime)
        {
            stats.skipped += 1;
            continue;
        }
        let Ok(bytes) = std::fs::read(&file) else {
            continue;
        };
        if is_binary(&bytes) {
            continue;
        }
        let hash = hash_bytes(&bytes);
        if known.get(&rel).is_some_and(|(h, _)| *h == hash) {
            // 仅 mtime 变化（touch、检出等），内容未变。
            touch.push((rel, mtime));
            stats.skipped += 1;
            continue;
        }
        // ponytail: 只索引 UTF-8 文本，其他编码的文件跳过（本项目代码均为 UTF-8）。
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        let chunks = chunk_text(&text);
        stats.chunks += chunks.len();
        update.push((rel, hash, mtime, chunks));
        stats.updated += 1;
    }
    let remove: Vec<&str> = known
        .keys()
        .filter(|p| !seen.contains(*p))
        .map(String::as_str)
        .collect();
    stats.removed = remove.len();

    // FTS 在事务提交后才可见，写入全部放在一个事务里；检索在提交后进行。
    let tx = turso::transaction::Transaction::new_unchecked(
        conn,
        turso::transaction::TransactionBehavior::Immediate,
    )
    .await
    .map_err(|source| StoreError::Query { source })?;

    // 仅 mtime 变化：indexed_files 是普通表，逐行 UPDATE 开销可忽略。
    for (rel, mtime) in &touch {
        tx.execute(
            "UPDATE indexed_files SET mtime = ?2 WHERE path = ?1",
            turso::params_from_iter([turso::Value::from(rel.as_str()), turso::Value::from(*mtime)]),
        )
        .await
        .map_err(|source| StoreError::Query { source })?;
    }

    // 变更 + 消失文件的旧分块：按批 DELETE（IN 列表）。
    let dirty: Vec<&str> = update
        .iter()
        .map(|(rel, ..)| rel.as_str())
        .chain(remove.iter().copied())
        .collect();
    for batch in dirty.chunks(BATCH_ROWS) {
        let placeholders = vec!["?"; batch.len()].join(", ");
        let params: Vec<_> = batch
            .iter()
            .map(|p| turso::Value::from((*p).to_string()))
            .collect();
        tx.execute(
            &format!("DELETE FROM chunks WHERE path IN ({placeholders})"),
            turso::params_from_iter(params),
        )
        .await
        .map_err(|source| StoreError::Query { source })?;
    }
    for batch in remove.chunks(BATCH_ROWS) {
        let placeholders = vec!["?"; batch.len()].join(", ");
        let params: Vec<_> = batch
            .iter()
            .map(|p| turso::Value::from((*p).to_string()))
            .collect();
        tx.execute(
            &format!("DELETE FROM indexed_files WHERE path IN ({placeholders})"),
            turso::params_from_iter(params),
        )
        .await
        .map_err(|source| StoreError::Query { source })?;
    }

    // 新分块：多行 INSERT 批量写入。
    let flat: Vec<(&str, i64, &str)> = update
        .iter()
        .flat_map(|(rel, .., chunks)| {
            chunks
                .iter()
                .map(|(line, content)| (rel.as_str(), *line as i64, content.as_str()))
        })
        .collect();
    for batch in flat.chunks(BATCH_ROWS) {
        let placeholders = vec!["(?, ?, ?)"; batch.len()].join(", ");
        // 用 for 循环收集参数：闭包形式会触发 rustc 生命周期泛化报错。
        let mut params = Vec::with_capacity(batch.len() * 3);
        for &(path, line, content) in batch {
            params.push(turso::Value::from(path));
            params.push(turso::Value::from(line));
            params.push(turso::Value::from(content));
        }
        tx.execute(
            &format!("INSERT INTO chunks (path, start_line, content) VALUES {placeholders}"),
            turso::params_from_iter(params),
        )
        .await
        .map_err(|source| StoreError::Query { source })?;
    }

    // 文件级状态 upsert（普通表）。
    for (rel, hash, mtime, _) in &update {
        tx.execute(
            "INSERT INTO indexed_files (path, hash, mtime) VALUES (?1, ?2, ?3)
             ON CONFLICT(path) DO UPDATE SET hash = ?2, mtime = ?3",
            turso::params_from_iter([
                turso::Value::from(rel.as_str()),
                turso::Value::from(hash.as_str()),
                turso::Value::from(*mtime),
            ]),
        )
        .await
        .map_err(|source| StoreError::Query { source })?;
    }

    tx.commit()
        .await
        .map_err(|source| StoreError::Query { source })?;
    Ok(stats)
}

/// FTS 关键词检索：BM25 升序（越小越相关），命中词以 `**` 高亮。
pub async fn search(
    conn: &turso::Connection,
    query: &str,
    limit: i64,
) -> Result<Vec<Hit>, StoreError> {
    let mut rows = conn
        .query(
            "SELECT path, start_line, fts_score(content, ?1), \
                    fts_highlight(content, '**', '**', ?1) \
             FROM chunks WHERE fts_match(content, ?1) \
             ORDER BY fts_score(content, ?1) ASC LIMIT ?2",
            turso::params_from_iter([turso::Value::from(query), turso::Value::from(limit)]),
        )
        .await
        .map_err(|source| StoreError::Query { source })?;
    let mut hits = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|source| StoreError::Query { source })?
    {
        hits.push(Hit {
            path: row.get(0).map_err(|source| StoreError::Query { source })?,
            start_line: row.get(1).map_err(|source| StoreError::Query { source })?,
            score: row.get(2).map_err(|source| StoreError::Query { source })?,
            content: row.get(3).map_err(|source| StoreError::Query { source })?,
        });
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_text_splits_with_overlap() {
        let text: String = (1..=250).map(|i| format!("line {i}\n")).collect();
        let chunks = chunk_text(&text);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].0, 1);
        assert_eq!(chunks[0].1.lines().count(), 100);
        // 重叠 20 行：第二块从第 81 行开始
        assert_eq!(chunks[1].0, 81);
        assert_eq!(chunks[2].0, 161);
        assert_eq!(chunks[2].1.lines().count(), 90);
    }

    #[test]
    fn chunk_text_small_and_empty() {
        assert!(chunk_text("").is_empty());
        assert!(chunk_text("\n\n").len() == 1);
        let chunks = chunk_text("a\nb\n");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], (1, "a\nb".to_string()));
    }

    struct TestEnv {
        root: PathBuf,
        db: PathBuf,
    }

    impl TestEnv {
        fn new() -> Self {
            let base =
                std::env::temp_dir().join(format!("togi_index_test_{}", uuid::Uuid::new_v4()));
            let root = base.join("root");
            std::fs::create_dir_all(&root).unwrap();
            Self {
                root,
                db: base.join("test.db"),
            }
        }
        async fn store(&self) -> crate::store::HistoryStore {
            crate::store::HistoryStore::open(&self.db).await.unwrap()
        }
        fn write(&self, rel: &str, content: &str) {
            write_file(&self.root, rel, content);
        }
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.root.parent().unwrap());
        }
    }

    fn write_file(dir: &Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[tokio::test]
    async fn index_and_search_roundtrip() {
        let env = TestEnv::new();
        env.write(
            "src/agent.rs",
            "fn retry() {\n    // retry with exponential backoff\n}\n",
        );
        env.write("src/main.rs", "fn main() { println!(\"hi\"); }\n");
        env.write("notes.txt", "nothing relevant here\n");
        let store = env.store().await;

        let stats = index_root(store.conn(), &env.root).await.unwrap();
        assert_eq!(stats.scanned, 3);
        assert_eq!(stats.updated, 3);
        assert_eq!(stats.chunks, 3);

        let hits = search(store.conn(), "retry backoff", 5).await.unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].path, "src/agent.rs");
        assert_eq!(hits[0].start_line, 1);
        assert!(hits[0].content.contains("**"));
    }

    #[tokio::test]
    async fn incremental_update_and_removal() {
        let env = TestEnv::new();
        env.write("a.txt", "alpha bravo charlie\n");
        env.write("b.txt", "delta echo foxtrot\n");
        let store = env.store().await;

        let first = index_root(store.conn(), &env.root).await.unwrap();
        assert_eq!(first.updated, 2);

        let second = index_root(store.conn(), &env.root).await.unwrap();
        assert_eq!(second.updated, 0);
        assert_eq!(second.removed, 0);
        assert_eq!(second.skipped, 2);

        // 修改一个文件 → 只更新它
        env.write("a.txt", "alpha bravo charlie changed\n");
        let third = index_root(store.conn(), &env.root).await.unwrap();
        assert_eq!(third.updated, 1);
        let hits = search(store.conn(), "changed", 5).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "a.txt");

        // 删除一个文件 → 从索引移除
        std::fs::remove_file(env.root.join("b.txt")).unwrap();
        let fourth = index_root(store.conn(), &env.root).await.unwrap();
        assert_eq!(fourth.removed, 1);
        let hits = search(store.conn(), "foxtrot", 5).await.unwrap();
        assert!(hits.is_empty());
    }

    /// 路线图 §2.1 验收：索引本项目（~1.4 万行）应秒级完成，
    /// `retry backoff` 应命中 agent.rs。手动跑：
    /// `cargo test --lib index_self_project -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn index_self_project() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let db = std::env::temp_dir().join(format!("togi_self_index_{}.db", uuid::Uuid::new_v4()));
        let store = crate::store::HistoryStore::open(&db).await.unwrap();
        let start = std::time::Instant::now();
        let stats = index_root(store.conn(), &root).await.unwrap();
        let elapsed = start.elapsed();
        println!("indexed in {elapsed:?}: {stats:?}");
        assert!(elapsed.as_secs() < 10);

        let hits = search(store.conn(), "retry backoff", 5).await.unwrap();
        assert!(!hits.is_empty());
        assert!(
            hits.iter().any(|h| h.path.ends_with("agent.rs")),
            "expected agent.rs hit, got: {:?}",
            hits.iter().map(|h| &h.path).collect::<Vec<_>>()
        );
        let _ = std::fs::remove_file(&db);
    }

    #[tokio::test]
    async fn binary_files_are_skipped() {
        let env = TestEnv::new();
        std::fs::write(env.root.join("bin.dat"), b"\x00\x01\x02binaryneedle\x00").unwrap();
        env.write("text.txt", "just text\n");
        let store = env.store().await;
        index_root(store.conn(), &env.root).await.unwrap();
        let hits = search(store.conn(), "binaryneedle", 5).await.unwrap();
        assert!(hits.is_empty());
    }
}
