use crate::support::TestDir;
use rig::tool::ToolContext;
use togi::agent::AgentEvent;
use togi::tools::agent::AgentTool;

/// 隔离的测试工作区：自带 profile 定义与被调查文件，不依赖仓库自身内容。
fn fixture_dir() -> TestDir {
    let dir = TestDir::new();
    std::fs::write(
        dir.join(".togi/agents/reader.md"),
        "---\ndescription = \"只读分析\"\ntools = [\"read\", \"shell\"]\n---\n你是只读分析子代理。\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("docs/design.txt"),
        "ToolEffect 标记工具调用的副作用类别：ReadOnly 或 Mutating。\n",
    )
    .unwrap();
    dir
}

/// 返回上下文与 cancel sender——sender 必须活到调用结束：watch sender
/// 全部 drop 后 `changed()` 立即就绪，子代理会被瞬时取消。
fn context(tx: togi::agent::AgentEventSender) -> (ToolContext, tokio::sync::watch::Sender<bool>) {
    let mut ctx = ToolContext::new();
    ctx.insert(tx);
    let (cancel_tx, _cancel_rx) = tokio::sync::watch::channel(false);
    ctx.insert(cancel_tx.subscribe());
    (ctx, cancel_tx)
}

/// 路线图 §1.1 验收：委派子任务 → 子代理事件带深度戳记转发 → 结论回传。
#[tokio::test]
#[ignore = "live: needs DEEPSEEK_API_KEY"]
async fn agent_tool_runs_subagent_and_returns_conclusion() {
    let dir = fixture_dir();
    let tool = crate::support::TestTool::new(togi::pipeline::adapt(AgentTool::new(
        dir.path(),
        rig::providers::deepseek::DEEPSEEK_V4_PRO.to_string(),
        None,
        10,
    )));
    assert_eq!(tool.name(), "agent");
    // profile 出现在工具描述中，供模型选择。
    assert!(tool.description().contains("reader"));

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let (mut ctx, _cancel_tx) = context(tx);
    let args = serde_json::json!({
        "task": "读 docs/design.txt，用一句话说明 ToolEffect 的作用。",
        "profile": "reader",
    })
    .to_string();
    let conclusion = tool.call_with(args, &mut ctx).await.unwrap();
    assert!(!conclusion.trim().is_empty());

    // 子代理的工具活动应以 Child{depth:1} 形式转发。
    let mut saw_child_tool_event = false;
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::Child { depth: 1, event } = event
            && matches!(
                *event,
                AgentEvent::ToolCall { .. } | AgentEvent::ToolResult { .. }
            )
        {
            saw_child_tool_event = true;
        }
    }
    assert!(saw_child_tool_event, "expected forwarded child tool events");
}

/// 未知 profile 作为工具错误回传给模型（不 panic、不带出进程）。
#[tokio::test]
#[ignore = "live: needs DEEPSEEK_API_KEY"]
async fn agent_tool_unknown_profile_is_a_tool_error() {
    let dir = fixture_dir();
    let tool = crate::support::TestTool::new(togi::pipeline::adapt(AgentTool::new(
        dir.path(),
        rig::providers::deepseek::DEEPSEEK_V4_PRO.to_string(),
        None,
        10,
    )));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let (mut ctx, _cancel_tx) = context(tx);
    let args = serde_json::json!({"task": "x", "profile": "nope"}).to_string();
    let err = tool.call_with(args, &mut ctx).await.unwrap_err();
    assert!(err.to_string().contains("reader"), "{err}");
}
