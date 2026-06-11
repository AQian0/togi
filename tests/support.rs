#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rig::tool::ToolDyn;

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

pub fn inject_cwd<T>(cwd: impl AsRef<Path>, tool: T) -> Box<dyn ToolDyn>
where
    T: ToolDyn + 'static,
{
    togi::inject::inject(
        togi::inject::Injection::new()
            .value(togi::inject::CWD_PARAM, cwd.as_ref().display().to_string()),
        tool,
    )
}

pub fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "togi-test-{}-{}-{name}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ))
}

pub fn remove_file(path: impl AsRef<Path>) {
    let _ = std::fs::remove_file(path);
}
