use crate::constants;
use std::path::{Path, PathBuf};

const HISTORY_FILE: &str = ".togi_history";

pub(crate) struct History {
    entries: Vec<String>,
    path: Option<PathBuf>,
    cursor: Option<usize>,
    draft: Option<String>,
}

impl History {
    pub(crate) fn load_default() -> Self {
        Self::load(history_path())
    }

    pub(crate) fn load(path: Option<PathBuf>) -> Self {
        let entries = path.as_deref().map(load_history).unwrap_or_default();
        Self {
            entries,
            path,
            cursor: None,
            draft: None,
        }
    }

    pub(crate) fn detach(&mut self) {
        self.cursor = None;
        self.draft = None;
    }

    pub(crate) fn previous(&mut self, current_draft: String) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        let next = match self.cursor {
            None => {
                self.draft = Some(current_draft);
                self.entries.len() - 1
            }
            Some(0) => return None,
            Some(i) => i - 1,
        };
        self.cursor = Some(next);
        Some(self.entries[next].clone())
    }

    pub(crate) fn next(&mut self) -> Option<String> {
        match self.cursor {
            Some(i) if i + 1 < self.entries.len() => {
                self.cursor = Some(i + 1);
                Some(self.entries[i + 1].clone())
            }
            Some(_) => {
                self.cursor = None;
                Some(self.draft.take().unwrap_or_default())
            }
            None => None,
        }
    }

    pub(crate) fn push(&mut self, entry: String) {
        self.detach();
        if self.entries.last() == Some(&entry) {
            return;
        }
        self.entries.push(entry);
    }

    pub(crate) fn save(&self) -> std::io::Result<()> {
        let Some(path) = self.path.as_deref() else {
            return Ok(());
        };
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let start = self
            .entries
            .len()
            .saturating_sub(constants::MAX_HISTORY_ENTRIES);
        let mut body = String::new();
        for entry in &self.entries[start..] {
            body.push_str(&entry.replace('\n', " "));
            body.push('\n');
        }
        std::fs::write(path, body)?;
        Ok(())
    }
}

fn load_history(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .map(|content| {
            content
                .lines()
                .map(str::to_string)
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn history_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(constants::ENV_HISTORY_PATH)
        && !path.is_empty()
    {
        return Some(PathBuf::from(path));
    }
    home_dir().map(|home| home.join(HISTORY_FILE))
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_history(entries: Vec<&str>) -> History {
        History {
            entries: entries.into_iter().map(String::from).collect(),
            path: None,
            cursor: None,
            draft: None,
        }
    }

    #[test]
    fn previous_on_empty_returns_none() {
        let mut h = make_history(vec![]);
        assert_eq!(h.previous("draft".into()), None);
    }

    #[test]
    fn previous_returns_last_entry() {
        let mut h = make_history(vec!["a", "b", "c"]);
        assert_eq!(h.previous("draft".into()), Some("c".into()));
    }

    #[test]
    fn previous_navigates_backward() {
        let mut h = make_history(vec!["a", "b", "c"]);
        assert_eq!(h.previous("draft".into()), Some("c".into()));
        assert_eq!(h.previous("c".into()), Some("b".into()));
        assert_eq!(h.previous("b".into()), Some("a".into()));
        assert_eq!(h.previous("a".into()), None); // 已到顶部
    }

    #[test]
    fn previous_saves_draft() {
        let mut h = make_history(vec!["a", "b"]);
        let _ = h.previous("my draft".into());
        assert_eq!(h.draft, Some("my draft".into()));
    }

    #[test]
    fn next_navigates_forward() {
        let mut h = make_history(vec!["a", "b", "c"]);
        // 先导航到最前面
        let _ = h.previous("draft".into()); // c
        let _ = h.previous("c".into()); // b
        let _ = h.previous("b".into()); // a
        // 然后向前导航
        assert_eq!(h.next(), Some("b".into()));
        assert_eq!(h.next(), Some("c".into()));
    }

    #[test]
    fn next_past_end_returns_draft() {
        let mut h = make_history(vec!["a", "b"]);
        let _ = h.previous("my draft".into()); // b
        assert_eq!(h.next(), Some("my draft".into())); // 回到 draft
    }

    #[test]
    fn next_on_detached_returns_none() {
        let mut h = make_history(vec!["a"]);
        assert_eq!(h.next(), None); // 未导航过，直接返回 None
    }

    #[test]
    fn push_appends_entry() {
        let mut h = make_history(vec![]);
        h.push("hello".into());
        assert_eq!(h.entries, vec!["hello"]);
    }

    #[test]
    fn push_deduplicates_consecutive() {
        let mut h = make_history(vec!["hello"]);
        h.push("hello".into());
        assert_eq!(h.entries, vec!["hello"]); // 不重复添加
    }

    #[test]
    fn push_allows_non_consecutive_duplicates() {
        let mut h = make_history(vec!["hello", "world"]);
        h.push("hello".into());
        assert_eq!(h.entries, vec!["hello", "world", "hello"]);
    }

    #[test]
    fn push_detaches_navigation() {
        let mut h = make_history(vec!["a", "b"]);
        let _ = h.previous("draft".into());
        assert!(h.cursor.is_some());
        h.push("c".into());
        assert!(h.cursor.is_none()); // push 后应脱离导航状态
    }

    #[test]
    fn detach_clears_cursor_and_draft() {
        let mut h = make_history(vec!["a"]);
        let _ = h.previous("draft".into());
        assert!(h.cursor.is_some());
        assert!(h.draft.is_some());
        h.detach();
        assert!(h.cursor.is_none());
        assert!(h.draft.is_none());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = std::env::temp_dir().join("togi_history_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_history");
        let _ = std::fs::remove_file(&path);

        let mut h = History::load(Some(path.clone()));
        h.push("line one".into());
        h.push("line two".into());
        h.save().unwrap();

        let h2 = History::load(Some(path.clone()));
        assert_eq!(h2.entries, vec!["line one", "line two"]);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn load_nonexistent_returns_empty() {
        let h = History::load(Some(PathBuf::from("/tmp/togi_nonexistent_history_file")));
        assert!(h.entries.is_empty());
    }

    #[test]
    fn save_replaces_newlines() {
        let dir = std::env::temp_dir().join("togi_history_nl_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_history_nl");
        let _ = std::fs::remove_file(&path);

        let mut h = History::load(Some(path.clone()));
        h.push("line with\nnewline".into());
        h.save().unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.contains('\n') || content.lines().count() <= 2);
        // 包含的换行符应被替换为空格
        assert!(content.contains("line with newline"));

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }
}
