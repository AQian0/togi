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

/// 将单个 [`AgentEvent`] 转换为 [`OutputItem`]。
///
/// `pending_effect` 用于延迟关联工具结果与其前置调用：
/// - `AgentEvent::ToolCall` 将其副作用类别写入 `pending_effect`；
/// - `AgentEvent::ToolResult` 读取并清除 `pending_effect` 来决定展示策略。
///
/// 因为 rig-core 的流保证每对 tool_call / tool_result 严格 1:1 交替，
/// 单向记忆最近一次调用的分类即可正确工作。
pub fn to_output(
    event: AgentEvent,
    pending_effect: &mut Option<ToolEffect>,
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
            *pending_effect = Some(registry.classify(&name, &arguments));
            OutputItem::ToolCall { name, summary }
        }
        AgentEvent::ToolResult {
            text,
            internal_call_id: _,
        } => {
            match pending_effect.take().unwrap_or(ToolEffect::Mutating) {
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
