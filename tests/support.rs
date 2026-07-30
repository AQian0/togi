#![allow(dead_code)]

use rig::completion::ToolDefinition;
use rig::tool::{DynamicTool, Tool, ToolContext, ToolSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// 测试侧工具句柄：包装 [`DynamicTool`]，提供旧的 `ToolDyn::call` 风格接口。
pub struct TestTool {
    set: ToolSet,
    definition: ToolDefinition,
}

impl TestTool {
    pub fn new(tool: DynamicTool) -> Self {
        let definition = tool.definition();
        Self {
            set: ToolSet::from_dynamic_tools(vec![tool]),
            definition,
        }
    }

    pub fn name(&self) -> &str {
        &self.definition.name
    }

    pub fn description(&self) -> &str {
        &self.definition.description
    }

    pub fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    pub async fn call(&self, args: String) -> Result<String, String> {
        self.call_with(args, &mut ToolContext::new()).await
    }

    pub async fn call_with(
        &self,
        args: String,
        context: &mut ToolContext,
    ) -> Result<String, String> {
        let result = self.set.execute(&self.definition.name, args, context).await;
        if let Some(error) = result.error().or_else(|| result.refusal()) {
            return Err(error.message().to_string());
        }
        Ok(result.output().render())
    }
}

pub fn inject_cwd<T>(cwd: impl AsRef<Path>, tool: T) -> TestTool
where
    T: Tool + 'static,
{
    let mut params = serde_json::Map::new();
    params.insert(
        togi::pipeline::inject::CWD_PARAM.into(),
        cwd.as_ref().display().to_string().into(),
    );
    TestTool::new(
        togi::pipeline::inject::inject(params, vec![togi::pipeline::adapt(tool)])
            .pop()
            .unwrap(),
    )
}

pub struct TestDir {
    path: PathBuf,
    #[allow(dead_code)]
    inner: Option<tempfile::TempDir>,
    fallback: Option<PathBuf>,
}

impl TestDir {
    pub fn new() -> Self {
        let name = format!(
            "togi-test-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        );
        match tempfile::TempDir::with_prefix(&name) {
            Ok(inner) => {
                let path = inner.path().to_path_buf();
                Self {
                    path,
                    inner: Some(inner),
                    fallback: None,
                }
            }
            Err(_) => {
                let path = std::env::temp_dir().join(&name);
                let _ = std::fs::create_dir_all(&path);
                Self {
                    path: path.clone(),
                    inner: None,
                    fallback: Some(path.clone()),
                }
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn join(&self, relative: impl AsRef<Path>) -> PathBuf {
        let rel = relative.as_ref();
        assert!(!rel.is_absolute(), "TestDir::join requires a relative path");
        let full = self.path.join(rel);
        if let Some(parent) = full.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        full
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        if let Some(ref fallback) = self.fallback {
            let _ = std::fs::remove_dir_all(fallback);
        }
    }
}

impl AsRef<Path> for TestDir {
    fn as_ref(&self) -> &Path {
        &self.path
    }
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

pub struct TestContext {
    pub dir: TestDir,
    pub tool: TestTool,
}

impl TestContext {
    pub fn new<T>(tool: T) -> Self
    where
        T: Tool + 'static,
    {
        let dir = TestDir::new();
        let tool = inject_cwd(dir.as_ref(), tool);
        Self { dir, tool }
    }

    pub fn join(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.dir.join(relative)
    }
}
