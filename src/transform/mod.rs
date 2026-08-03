//! Agent 事件到 UI 输出项的消息转换层。
//!
//! 本模块位于 [`crate::agent`] 与 [`crate::ui`] 之间，负责将 Agent
//! 流式产生的事件转换为 UI 可消费的 [`OutputItem`]，包括：
//! - 推理 / 回答分区标记
//! - 工具调用的副作用分类与摘要
//! - 变更类工具确认请求的 UI 转发
//! - 只读工具结果的折叠展示
//! - 子代理（[`AgentEvent::Child`]）事件的深度展开、委派标识与缩进标记

use crate::agent::{AgentEvent, AgentSection};
use crate::tools::{ToolEffect, ToolRegistry};
use crate::ui::{OutputItem, SectionKind};
use std::collections::HashMap;

/// 将单个 [`AgentEvent`] 转换为 [`OutputItem`]。
///
/// `pending_effects` 以 rig 生成的 `internal_call_id`（随机、跨流唯一）
/// 键控工具调用的副作用类别，供对应结果到达时取出。子代理事件与父代理
/// 事件在时间上交错（父 `agent` 调用未返回时子代理的工具事件已到），
/// 按 id 键控在交错下仍能正确配对——FIFO 队列则会被打乱。
pub fn to_output(
    event: AgentEvent,
    pending_effects: &mut HashMap<String, ToolEffect>,
    registry: &ToolRegistry,
) -> OutputItem {
    to_output_at(event, 0, None, pending_effects, registry)
}

fn to_output_at(
    event: AgentEvent,
    depth: u32,
    label: Option<&str>,
    pending_effects: &mut HashMap<String, ToolEffect>,
    registry: &ToolRegistry,
) -> OutputItem {
    match event {
        // 嵌套时最内层戳记生效（外层 Child 只是转发路径上的包装）。
        AgentEvent::Child {
            depth,
            label,
            event,
        } => to_output_at(*event, depth, Some(&label), pending_effects, registry),
        AgentEvent::Section(section) => OutputItem::Section(match section {
            AgentSection::Reasoning => SectionKind::Reasoning,
            AgentSection::Answer => SectionKind::Answer,
        }),
        AgentEvent::Text(text) => OutputItem::Chunk(text),
        AgentEvent::ToolCall {
            name,
            arguments,
            internal_call_id,
        } => {
            let summary = crate::ui::summarize::summarize_call(&name, &arguments);
            pending_effects.insert(internal_call_id, registry.classify(&name, &arguments));
            // 子代理的调用行加 `[标识]` 前缀（仅展示用途，分类用原始名）：
            // 并行子代理事件交错时据以区分归属。
            let name = if let Some(l) = label {
                format!("[{l}] {name}")
            } else {
                name
            };
            OutputItem::ToolCall {
                name,
                summary,
                depth,
            }
        }
        AgentEvent::ToolResult {
            text,
            internal_call_id,
        } => {
            match pending_effects
                .remove(&internal_call_id)
                .unwrap_or(ToolEffect::Mutating)
            {
                // 只读 / 查询：仅展示行为与简短摘要，不在对话区铺开具体内容。
                ToolEffect::ReadOnly => OutputItem::ToolResult {
                    text: crate::ui::summarize::summarize_readonly_result(&text),
                    depth,
                },
                ToolEffect::Mutating | ToolEffect::ReadOnlyVerbose => {
                    OutputItem::ToolResult { text, depth }
                }
            }
        }
        AgentEvent::ApprovalRequest {
            name,
            arguments,
            response,
        } => OutputItem::Approval {
            summary: crate::ui::summarize::summarize_approval(&name, &arguments),
            name,
            depth,
            response,
        },
        AgentEvent::Notice(text) => OutputItem::Notice(indent(text, depth, label)),
    }
}

/// 子代理事件的文本缩进与归属标识：depth 1 → `↳ [标识] x`，depth 2 → `  ↳ [标识] x`。
fn indent(text: String, depth: u32, label: Option<&str>) -> String {
    if let Some(l) = label {
        format!("{}↳ [{l}] {text}", "  ".repeat(depth as usize - 1))
    } else {
        text
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

    fn call(name: &str, id: &str) -> AgentEvent {
        AgentEvent::ToolCall {
            name: name.to_string(),
            arguments: json!({}),
            internal_call_id: id.to_string(),
        }
    }

    fn result(text: &str, id: &str) -> AgentEvent {
        AgentEvent::ToolResult {
            text: text.to_string(),
            internal_call_id: id.to_string(),
        }
    }

    fn child(depth: u32, label: &str, event: AgentEvent) -> AgentEvent {
        AgentEvent::Child {
            depth,
            label: std::sync::Arc::from(label),
            event: Box::new(event),
        }
    }

    /// 结果按 internal_call_id 与调用配对，与到达顺序无关。
    #[test]
    fn results_pair_with_calls_by_id() {
        let registry = registry();
        let mut pending = HashMap::new();
        let body = "line1\nline2\nline3";

        let _ = to_output(call("read", "id-1"), &mut pending, &registry);
        let _ = to_output(call("modify", "id-2"), &mut pending, &registry);
        // 结果乱序到达仍能正确配对。
        let modify_out = to_output(result(body, "id-2"), &mut pending, &registry);
        let read_out = to_output(result(body, "id-1"), &mut pending, &registry);

        assert!(
            matches!(&read_out, OutputItem::ToolResult { text, .. } if text != body),
            "read-only result should be folded"
        );
        assert!(
            matches!(&modify_out, OutputItem::ToolResult { text, .. } if text == body),
            "mutating result should pass through verbatim"
        );
    }

    /// 结果先于任何调用到达（容错路径）：按保守的 Mutating 处理。
    #[test]
    fn orphan_result_defaults_to_mutating() {
        let registry = registry();
        let mut pending = HashMap::new();
        let out = to_output(result("raw", "id-x"), &mut pending, &registry);
        assert!(matches!(&out, OutputItem::ToolResult { text, .. } if text == "raw"));
    }

    /// 子代理事件与父代理事件交错：父 `agent` 调用挂起期间子代理的
    /// 调用/结果到达，双方按各自 id 配对、互不污染，且子事件带深度与标识。
    #[test]
    fn interleaved_child_events_pair_independently_and_carry_depth() {
        let registry = registry();
        let mut pending = HashMap::new();
        let body = "line1\nline2\nline3";

        let _ = to_output(call("agent", "parent"), &mut pending, &registry);
        let child_call = to_output(
            child(1, "审查a", call("read", "child-1")),
            &mut pending,
            &registry,
        );
        assert!(
            matches!(&child_call, OutputItem::ToolCall { depth: 1, name, .. }
                if name == "[审查a] read"),
            "child call should carry depth 1 and the delegation label"
        );
        let child_out = to_output(
            child(1, "审查a", result(body, "child-1")),
            &mut pending,
            &registry,
        );
        // 子 read 结果折叠，且没有弹出父 agent 的条目。
        assert!(
            matches!(&child_out, OutputItem::ToolResult { text, depth: 1 } if text != body)
        );
        let parent_out = to_output(result("结论", "parent"), &mut pending, &registry);
        // 父 agent 结果（未注册时按 Mutating）原样透出，未被误折叠。
        assert!(
            matches!(&parent_out, OutputItem::ToolResult { text, depth: 0 } if text == "结论")
        );
    }

    /// 并行子代理：两个子代理事件交错，各自标识正确、结果按 id 各归各。
    #[test]
    fn parallel_children_keep_their_own_labels() {
        let registry = registry();
        let mut pending = HashMap::new();

        let a_call = to_output(
            child(1, "审查a", call("read", "a-1")),
            &mut pending,
            &registry,
        );
        let b_call = to_output(
            child(1, "审查b", call("read", "b-1")),
            &mut pending,
            &registry,
        );
        assert!(matches!(&a_call, OutputItem::ToolCall { name, .. } if name == "[审查a] read"));
        assert!(matches!(&b_call, OutputItem::ToolCall { name, .. } if name == "[审查b] read"));
        // 结果乱序到达仍各归各的调用（按 internal_call_id，而非标识）。
        let _ = to_output(
            child(1, "审查b", result("b 结果", "b-1")),
            &mut pending,
            &registry,
        );
        let _ = to_output(
            child(1, "审查a", result("a 结果", "a-1")),
            &mut pending,
            &registry,
        );
        assert!(pending.is_empty());
    }

    /// 嵌套 Child：最内层深度与标识生效。
    #[test]
    fn nested_child_uses_innermost_depth_and_label() {
        let registry = registry();
        let mut pending = HashMap::new();
        let out = to_output(
            child(1, "外层", child(2, "内层", call("shell", "c"))),
            &mut pending,
            &registry,
        );
        assert!(
            matches!(&out, OutputItem::ToolCall { depth: 2, name, .. } if name == "[内层] shell")
        );
    }

    #[test]
    fn child_notice_is_indented_and_labeled() {
        let registry = registry();
        let mut pending = HashMap::new();
        let out = to_output(
            child(2, "审查a", AgentEvent::Notice("n".into())),
            &mut pending,
            &registry,
        );
        assert!(matches!(&out, OutputItem::Notice(t) if t == "  ↳ [审查a] n"));
        let out = to_output(AgentEvent::Notice("n".into()), &mut pending, &registry);
        assert!(matches!(&out, OutputItem::Notice(t) if t == "n"));
    }

    #[test]
    fn child_approval_keeps_depth_and_call_summary() {
        let registry = registry();
        let mut pending = HashMap::new();
        let (response, _decision_rx) = tokio::sync::oneshot::channel();
        let out = to_output(
            child(
                1,
                "审查a",
                AgentEvent::ApprovalRequest {
                    name: "modify".into(),
                    arguments: json!({ "path": "a.rs", "content": "x" }),
                    response,
                },
            ),
            &mut pending,
            &registry,
        );
        assert!(matches!(
            out,
            OutputItem::Approval { name, summary, depth: 1, .. }
                if name == "modify" && summary.contains("a.rs")
        ));
    }
}
