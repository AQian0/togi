//! Agent 事件到 UI 输出项的消息转换层。
//!
//! 本模块位于 [`crate::agent`] 与 [`crate::ui`] 之间，负责将 Agent
//! 流式产生的事件转换为 UI 可消费的 [`OutputItem`]，包括：
//! - 推理 / 回答分区标记
//! - 工具调用的副作用分类与摘要
//! - 只读工具结果的折叠展示

use crate::agent::{AgentEvent, AgentSection};
use crate::tools::{ToolEffect, ToolRegistry};
use crate::ui::interaction::{OutputItem, SectionKind};
use std::collections::VecDeque;

/// 将单个 [`AgentEvent`] 转换为 [`OutputItem`]。
///
/// `pending_effects` 用于延迟关联工具结果与其前置调用：
/// - `AgentEvent::ToolCall` 将其副作用类别入队；
/// - `AgentEvent::ToolResult` 出队队首来决定展示策略。
///
/// rig-core 在 `tool_concurrency > 1` 时先按调用顺序发完本轮所有
/// ToolCall，再按同一顺序逐个发 ToolResult，FIFO 队列因此保持正确配对。
pub fn to_output(
    event: AgentEvent,
    pending_effects: &mut VecDeque<ToolEffect>,
    registry: &ToolRegistry,
) -> OutputItem {
    match event {
        AgentEvent::Section(section) => OutputItem::Section(match section {
            AgentSection::Reasoning => SectionKind::Reasoning,
            AgentSection::Answer => SectionKind::Answer,
        }),
        AgentEvent::Text(text) => OutputItem::Chunk(text),
        AgentEvent::ToolCall {
            name,
            arguments,
            internal_call_id: _,
        } => {
            let summary = crate::ui::summarize::summarize_call(&name, &arguments);
            pending_effects.push_back(registry.classify(&name, &arguments));
            OutputItem::ToolCall { name, summary }
        }
        AgentEvent::ToolResult {
            text,
            internal_call_id: _,
        } => {
            match pending_effects.pop_front().unwrap_or(ToolEffect::Mutating) {
                // 只读 / 查询：仅展示行为与简短摘要，不在对话区铺开具体内容。
                ToolEffect::ReadOnly => {
                    OutputItem::ToolResult(crate::ui::summarize::summarize_readonly_result(&text))
                }
                ToolEffect::Mutating => OutputItem::ToolResult(text),
            }
        }
        AgentEvent::Notice(text) => OutputItem::Notice(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::modify::Modify;
    use crate::tools::read::Read;
    use serde_json::json;

    fn registry() -> ToolRegistry {
        let mut r = ToolRegistry::new();
        r.register::<Read>();
        r.register::<Modify>();
        r
    }

    fn call(name: &str) -> AgentEvent {
        AgentEvent::ToolCall {
            name: name.to_string(),
            arguments: json!({}),
            internal_call_id: String::new(),
        }
    }

    fn result(text: &str) -> AgentEvent {
        AgentEvent::ToolResult {
            text: text.to_string(),
            internal_call_id: String::new(),
        }
    }

    /// 并发批次：N 个 ToolCall 先到，N 个 ToolResult 随后按序到达，
    /// 每个结果必须与各自调用（而非相邻调用）的副作用类别配对。
    #[test]
    fn batched_calls_and_results_pair_in_fifo_order() {
        let registry = registry();
        let mut pending = VecDeque::new();
        let body = "line1\nline2\nline3";

        for name in ["read", "modify"] {
            let _ = to_output(call(name), &mut pending, &registry);
        }
        let read_out = to_output(result(body), &mut pending, &registry);
        let modify_out = to_output(result(body), &mut pending, &registry);

        // read 的结果被折叠为摘要（不等于原文），modify 的结果原样透出。
        assert!(
            matches!(&read_out, OutputItem::ToolResult(t) if t != body),
            "read-only result should be folded"
        );
        assert!(
            matches!(&modify_out, OutputItem::ToolResult(t) if t == body),
            "mutating result should pass through verbatim"
        );
    }

    /// 结果先于任何调用到达（容错路径）：按保守的 Mutating 处理。
    #[test]
    fn orphan_result_defaults_to_mutating() {
        let registry = registry();
        let mut pending = VecDeque::new();
        let out = to_output(result("raw"), &mut pending, &registry);
        assert!(matches!(&out, OutputItem::ToolResult(t) if t == "raw"));
    }
}
