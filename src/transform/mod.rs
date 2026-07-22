//! Agent 事件到 UI 输出项的消息转换层。
//!
//! 本模块位于 [`crate::agent`] 与 [`crate::ui`] 之间，负责将 Agent
//! 流式产生的事件转换为 UI 可消费的 [`OutputItem`]，包括：
//! - 推理 / 回答分区标记
//! - 工具调用的副作用分类与摘要
//! - 只读工具结果的折叠展示
//! - 子代理（[`AgentEvent::Child`]）事件的深度展开与缩进标记

use crate::agent::{AgentEvent, AgentSection};
use crate::tools::{ToolEffect, ToolRegistry};
use crate::ui::interaction::{OutputItem, SectionKind};
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
    to_output_at(event, 0, pending_effects, registry)
}

fn to_output_at(
    event: AgentEvent,
    depth: u32,
    pending_effects: &mut HashMap<String, ToolEffect>,
    registry: &ToolRegistry,
) -> OutputItem {
    match event {
        // 嵌套时最内层戳记的深度生效（外层 Child 只是转发路径上的包装）。
        AgentEvent::Child { depth, event } => to_output_at(*event, depth, pending_effects, registry),
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
                ToolEffect::Mutating => OutputItem::ToolResult { text, depth },
            }
        }
        AgentEvent::Notice(text) => OutputItem::Notice(indent(text, depth, "↳ ")),
    }
}

/// 子代理事件的文本缩进：depth 1 → `↳ x`，depth 2 → `  ↳ x`。
fn indent(text: String, depth: u32, marker: &str) -> String {
    if depth == 0 {
        text
    } else {
        format!("{}{marker}{text}", "  ".repeat(depth as usize - 1))
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

    fn child(depth: u32, event: AgentEvent) -> AgentEvent {
        AgentEvent::Child {
            depth,
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
    /// 调用/结果到达，双方按各自 id 配对、互不污染，且子事件带深度。
    #[test]
    fn interleaved_child_events_pair_independently_and_carry_depth() {
        let registry = registry();
        let mut pending = HashMap::new();
        let body = "line1\nline2\nline3";

        let _ = to_output(call("agent", "parent"), &mut pending, &registry);
        let child_call = to_output(child(1, call("read", "child-1")), &mut pending, &registry);
        assert!(
            matches!(&child_call, OutputItem::ToolCall { depth: 1, .. }),
            "child call should carry depth 1"
        );
        let child_out = to_output(child(1, result(body, "child-1")), &mut pending, &registry);
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

    /// 嵌套 Child：最内层深度生效。
    #[test]
    fn nested_child_uses_innermost_depth() {
        let registry = registry();
        let mut pending = HashMap::new();
        let out = to_output(
            child(1, child(2, call("shell", "c"))),
            &mut pending,
            &registry,
        );
        assert!(matches!(&out, OutputItem::ToolCall { depth: 2, .. }));
    }

    #[test]
    fn child_notice_is_indented() {
        let registry = registry();
        let mut pending = HashMap::new();
        let out = to_output(child(2, AgentEvent::Notice("n".into())), &mut pending, &registry);
        assert!(matches!(&out, OutputItem::Notice(t) if t == "  ↳ n"));
        let out = to_output(AgentEvent::Notice("n".into()), &mut pending, &registry);
        assert!(matches!(&out, OutputItem::Notice(t) if t == "n"));
    }
}
