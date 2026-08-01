//! Provider 无关的上下文窗口管理。
//!
//! 完整 transcript 始终保存在 `HistoryStore`；本模块只决定每次 completion
//! 实际发送给模型的活动上下文（滚动摘要 + 最近原文），不改写完整历史。
//!
//! 活动上下文结构：
//!
//! ```text
//! Message::System(summary)
//! + full_history[covered_messages..]
//! + current_prompt
//! ```

use rig::completion::Usage;
use rig::message::{AssistantContent, Message, ToolResult, ToolResultContent, UserContent};
use std::collections::HashSet;

// ── 数据模型 ────────────────────────────────────────────────────────

/// 预算配置。`window_tokens` 必须由用户显式配置，否则上下文管理整体关闭。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextPolicy {
    pub window_tokens: u64,
    pub reserve_tokens: u64,
    pub keep_recent_tokens: u64,
}

impl ContextPolicy {
    /// 触发压缩的输入预算：`window - reserve`。
    pub fn trigger_tokens(&self) -> u64 {
        self.window_tokens - self.reserve_tokens
    }

    /// 摘要请求的最大输出 token：`min(8192, reserve / 2)`。
    pub fn summary_max_tokens(&self) -> u64 {
        (self.reserve_tokens / 2).min(8192)
    }
}

/// 可持久化的压缩状态：摘要文本 + 已覆盖的完整历史前缀长度。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextCheckpoint {
    pub summary: Option<String>,
    pub covered_messages: usize,
}

impl ContextCheckpoint {
    /// 是否为空状态（无摘要且未覆盖任何消息）。
    pub fn is_empty(&self) -> bool {
        self.summary.is_none() && self.covered_messages == 0
    }
}

/// 一次提交所需的上下文输入：未配置窗口时 `policy` 为 None，管理关闭。
#[derive(Debug, Clone, Default)]
pub struct ContextInput {
    pub policy: Option<ContextPolicy>,
    pub checkpoint: ContextCheckpoint,
}

/// 一次真实请求的 usage 样本：provider 报告的输入 token 与当时的本地估算。
/// 运行期优化，不持久化——重启后模型 / system prompt / 工具定义可能已变化。
#[derive(Debug, Clone, Copy)]
pub struct UsageSample {
    pub input_tokens: u64,
    pub estimated_request_tokens: u64,
}

/// 单次提交内的上下文运行期状态（由 hook 与 usage 处理共享）。
#[derive(Debug, Default)]
pub struct ContextRuntime {
    pub checkpoint: ContextCheckpoint,
    pub usage: Option<UsageSample>,
    pub force_compaction: bool,
    /// hook 为最近一次请求计算的本地估算，与下一条 provider usage 配对成样本。
    pub last_request_estimate: u64,
    /// 触发强制压缩的原始 overflow 错误，用于无合法切点时的错误说明。
    pub overflow_error: Option<String>,
}

/// 压缩决策（纯函数结果，由 hook 执行）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// 未超预算且无 checkpoint：不 patch，发送原始历史。
    None,
    /// 使用当前 checkpoint 的活动历史 patch 本次请求。
    PatchActive,
    /// 将 `history[covered..k]` 并入摘要，然后用新活动历史 patch。
    Compact(usize),
    /// 强制压缩但无合法切点 / 已无进展：终止并保留完整历史。
    Terminate,
}

// ── token 估算 ──────────────────────────────────────────────────────

/// 每条消息的固定结构开销（role、JSON 骨架等），保守取值。
const MESSAGE_OVERHEAD_TOKENS: u64 = 8;

/// ponytail: 非文本内容（图片 / 音视频 / 文档）按固定值估算；
/// 本应用的工具不产生此类内容，出现多模态误估时再细化。
const NON_TEXT_CONTENT_TOKENS: u64 = 1024;

/// 保守 token 估算：ASCII 约 4 字符 / token，非 ASCII 约 1 字符 / token。
pub fn estimate_tokens(text: &str) -> u64 {
    let ascii = text.bytes().filter(u8::is_ascii).count() as u64;
    ascii.div_ceil(4) + text.chars().count() as u64 - ascii
}

/// 估算单条消息，包含工具参数、工具结果、reasoning 等全部序列化内容。
pub fn estimate_message(msg: &Message) -> u64 {
    let body = match msg {
        Message::System { content } => estimate_tokens(content),
        Message::User { content } => content
            .iter()
            .map(|c| match c {
                UserContent::Text(t) => estimate_tokens(&t.text),
                UserContent::ToolResult(r) => r
                    .content
                    .iter()
                    .map(|rc| match rc {
                        ToolResultContent::Text(t) => estimate_tokens(&t.text),
                        _ => NON_TEXT_CONTENT_TOKENS,
                    })
                    .sum(),
                _ => NON_TEXT_CONTENT_TOKENS,
            })
            .sum(),
        Message::Assistant { content, .. } => content
            .iter()
            .map(|c| match c {
                AssistantContent::Text(t) => estimate_tokens(&t.text),
                AssistantContent::ToolCall(tc) => {
                    estimate_tokens(&tc.function.name)
                        + estimate_tokens(&tc.function.arguments.to_string())
                }
                AssistantContent::Reasoning(r) => estimate_tokens(&r.display_text()),
                _ => NON_TEXT_CONTENT_TOKENS,
            })
            .sum(),
    };
    MESSAGE_OVERHEAD_TOKENS + body
}

/// 估算一组消息的总 token。
pub fn estimate_history(messages: &[Message]) -> u64 {
    messages.iter().map(estimate_message).sum()
}

// ── 预算投影 ────────────────────────────────────────────────────────

/// 从 provider usage 提取有效输入 token；全零视为未上报，返回 None。
pub fn effective_input_tokens(usage: &Usage) -> Option<u64> {
    if !usage.has_values() {
        return None;
    }
    Some(
        usage
            .input_tokens
            .max(usage.total_tokens.saturating_sub(usage.output_tokens)),
    )
}

/// 用上一请求的真实输入与本地估算校准当前请求投影：
///
/// ```text
/// projected = previous_input + current_estimate - previous_estimate
/// ```
///
/// 保留 provider 对 system prompt、工具定义与消息格式的实际开销。
pub fn project_input(usage: Option<UsageSample>, current_estimate: u64) -> u64 {
    match usage {
        Some(sample) => sample
            .input_tokens
            .saturating_add(current_estimate)
            .saturating_sub(sample.estimated_request_tokens),
        None => current_estimate,
    }
}

// ── 活动历史 ────────────────────────────────────────────────────────

/// 注入活动历史的摘要 system 消息前缀。
const SUMMARY_MESSAGE_PREFIX: &str = "\
[Rolling summary of earlier messages in this session. \
The full transcript is preserved; treat this as context for the work so far.]\n\n";

/// 由完整历史构造模型可见历史：summary system 消息 + 未覆盖后缀。
pub fn build_active_history(
    checkpoint: &ContextCheckpoint,
    full_history: &[Message],
) -> Vec<Message> {
    let start = checkpoint.covered_messages.min(full_history.len());
    let mut out = Vec::with_capacity(full_history.len() - start + 1);
    if let Some(summary) = checkpoint.summary.as_deref().filter(|s| !s.is_empty()) {
        out.push(Message::system(format!(
            "{SUMMARY_MESSAGE_PREFIX}{summary}"
        )));
    }
    out.extend_from_slice(&full_history[start..]);
    out
}

// ── 安全切分 ────────────────────────────────────────────────────────

/// user 消息是否包含 tool result。
fn is_tool_result_message(msg: &Message) -> bool {
    matches!(msg, Message::User { content }
        if content.iter().any(|c| matches!(c, UserContent::ToolResult(_))))
}

/// 是否普通用户消息（全新 user turn 边界）。
fn is_plain_user_message(msg: &Message) -> bool {
    matches!(msg, Message::User { .. }) && !is_tool_result_message(msg)
}

/// 收集 assistant 消息中的 tool call 标识（id 与 provider call_id）。
fn collect_call_ids<'a>(msg: &'a Message, ids: &mut HashSet<&'a str>) {
    if let Message::Assistant { content, .. } = msg {
        for c in content.iter() {
            if let AssistantContent::ToolCall(tc) = c {
                ids.insert(tc.id.as_str());
                if let Some(call_id) = &tc.call_id {
                    ids.insert(call_id.as_str());
                }
            }
        }
    }
}

/// result 是否能在 id 集合中找到对应 call。
fn result_matched(result: &ToolResult, ids: &HashSet<&str>) -> bool {
    ids.contains(result.id.as_str())
        || result
            .call_id
            .as_deref()
            .is_some_and(|cid| ids.contains(cid))
}

/// 消息是否包含指定 tool result 对应的 call。
fn message_has_call(msg: &Message, result: &ToolResult) -> bool {
    let mut ids = HashSet::new();
    collect_call_ids(msg, &mut ids);
    result_matched(result, &ids)
}

/// prompt 中的 tool result 对应的 call 在历史中的下界：
/// 切点 k 必须 ≤ 该值，保证 call 留在活动历史内。
/// prompt 不含 tool result 时返回 `history.len()`（无约束）；
/// 任一 result 找不到对应 call（transcript 已损坏）返回 None。
fn prompt_call_floor(history: &[Message], prompt: &Message) -> Option<usize> {
    let Message::User { content } = prompt else {
        return Some(history.len());
    };
    let mut floor = history.len();
    for c in content.iter() {
        if let UserContent::ToolResult(result) = c {
            let idx = history.iter().position(|m| message_has_call(m, result))?;
            floor = floor.min(idx);
        }
    }
    Some(floor)
}

/// 校验切点：`history[k..]` + prompt 中的工具调用关系必须合法。
fn legal_cut(history: &[Message], k: usize, prompt: &Message) -> bool {
    let retained = &history[k.min(history.len())..];
    // 不允许活动历史从孤立的 ToolResult 开始。
    if retained.first().is_some_and(is_tool_result_message) {
        return false;
    }
    let mut ids = HashSet::new();
    for m in retained {
        collect_call_ids(m, &mut ids);
    }
    // retained 内每个 ToolResult（含并行批次）都要有对应 ToolCall。
    let all_results_matched = |msg: &Message| {
        let Message::User { content } = msg else {
            return true;
        };
        content.iter().all(|c| match c {
            UserContent::ToolResult(r) => result_matched(r, &ids),
            _ => true,
        })
    };
    retained.iter().all(all_results_matched) && all_results_matched(prompt)
}

/// 选择切点：在 `keep_recent_tokens` 预算内保留最多合法历史。
///
/// 优先级：完整 user turn 边界 > assistant 边界（split-turn）。
/// `min_covered` 为已覆盖前缀，切点只向前滚动。
pub fn plan_split(
    history: &[Message],
    prompt: &Message,
    policy: &ContextPolicy,
    min_covered: usize,
) -> Option<usize> {
    let len = history.len();
    let min_k = min_covered.min(len);
    let prompt_est = estimate_message(prompt);
    let k_floor = prompt_call_floor(history, prompt)?;
    if k_floor < min_k {
        return None;
    }
    // keep_recent 预算内可保留的最大后缀起点。
    let mut cost = prompt_est;
    let mut k_budget = len;
    while k_budget > min_k {
        let c = estimate_message(&history[k_budget - 1]);
        if cost + c > policy.keep_recent_tokens {
            break;
        }
        cost += c;
        k_budget -= 1;
    }
    let legal = (min_k..=len).filter(|&k| k <= k_floor && legal_cut(history, k, prompt));
    let in_budget: Vec<usize> = legal.clone().filter(|&k| k >= k_budget).collect();
    // 预算内：保留最多（最小 k），按边界质量偏好挑选。
    // 全部超预算（单个 turn 太大）：保留最少（最大 k），允许 split-turn。
    let chosen = if in_budget.is_empty() {
        legal.max()
    } else {
        in_budget
            .iter()
            .copied()
            .find(|&k| k < len && is_plain_user_message(&history[k]))
            .or_else(|| {
                in_budget
                    .iter()
                    .copied()
                    .find(|&k| k < len && matches!(&history[k], Message::Assistant { .. }))
            })
            .or_else(|| in_budget.first().copied())
    };
    let k = chosen?;
    // 单个不可拆片段仍超窗口（含摘要预留）→ 明确失败，不制造非法历史。
    let retained_est = estimate_history(&history[k..]) + prompt_est;
    if retained_est.saturating_add(policy.summary_max_tokens()) >= policy.trigger_tokens() {
        return None;
    }
    Some(k)
}

// ── 压缩决策 ────────────────────────────────────────────────────────

/// 每次 completion 前的纯决策：是否压缩、如何 patch。
///
/// `estimate` 为当前活动历史 + prompt 的本地估算（调用方计算并记账）。
pub fn decide(
    policy: &ContextPolicy,
    checkpoint: &ContextCheckpoint,
    usage: Option<UsageSample>,
    force: bool,
    history: &[Message],
    prompt: &Message,
    estimate: u64,
) -> Decision {
    if !force && project_input(usage, estimate) < policy.trigger_tokens() {
        return if checkpoint.is_empty() {
            Decision::None
        } else {
            Decision::PatchActive
        };
    }
    match plan_split(history, prompt, policy, checkpoint.covered_messages) {
        Some(k) if k > checkpoint.covered_messages => Decision::Compact(k),
        // 强制压缩时仍无合法切点（或已无进展）：终止；否则按现状发送，
        // 交给 provider overflow 恢复路径处理。
        _ if force => Decision::Terminate,
        _ => Decision::PatchActive,
    }
}

// ── 滚动摘要 ────────────────────────────────────────────────────────

/// 摘要输入中单个 ToolResult 的最大字符数。
const SUMMARY_TOOL_RESULT_MAX_CHARS: usize = 2000;

/// 摘要请求的系统提示词：固定输出结构，合并旧摘要与新淘汰历史。
pub const SUMMARY_PREAMBLE: &str = "\
You are maintaining a rolling summary of a longer conversation between a user \
and a coding assistant. You receive the previous summary and a new slice of \
conversation history. Produce an updated summary that merges both, keeping only \
information useful for continuing the work. Use exactly this markdown structure:

## Goal
## Constraints & Preferences
## Progress
### Done
### In Progress
### Blocked
## Key Decisions
## Next Steps
## Critical Context
## Relevant Files

Rules: be concise; drop chit-chat and superseded details; keep file paths, \
command outcomes, error messages and decisions verbatim when relevant; write in \
the same language as the conversation. Output only the summary markdown.";

/// 摘要生成失败。
#[derive(Debug, thiserror::Error)]
pub enum SummaryError {
    #[error("summary request failed: {0}")]
    Request(#[from] rig::completion::CompletionError),
    #[error("summary response was empty")]
    Empty,
}

/// 按字符数截断并附加省略标记。
fn truncate_chars(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((i, _)) => format!("{}…[truncated]", &text[..i]),
        None => text.to_string(),
    }
}

/// 将待淘汰消息序列化为摘要输入文本。
///
/// ToolResult 截断到 [`SUMMARY_TOOL_RESULT_MAX_CHARS`] 字符，保留工具名与
/// call ID；完整 transcript 中的 ToolResult 不受影响。
pub fn serialize_for_summary(messages: &[Message]) -> String {
    // 先扫描区间内的 tool call，建立 id → 工具名映射（含 provider call_id）。
    let mut names: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for msg in messages {
        if let Message::Assistant { content, .. } = msg {
            for c in content.iter() {
                if let AssistantContent::ToolCall(tc) = c {
                    names.insert(tc.id.as_str(), tc.function.name.as_str());
                    if let Some(call_id) = &tc.call_id {
                        names.insert(call_id.as_str(), tc.function.name.as_str());
                    }
                }
            }
        }
    }
    let tool_name = |r: &ToolResult| -> &str {
        names
            .get(r.id.as_str())
            .or_else(|| r.call_id.as_deref().and_then(|cid| names.get(cid)))
            .copied()
            .unwrap_or("unknown")
    };
    let mut out = String::new();
    for msg in messages {
        match msg {
            Message::System { content } => {
                out.push_str("[system]\n");
                out.push_str(content);
                out.push('\n');
            }
            Message::User { content } => {
                for c in content.iter() {
                    match c {
                        UserContent::Text(t) => {
                            out.push_str("[user]\n");
                            out.push_str(&t.text);
                            out.push('\n');
                        }
                        UserContent::ToolResult(r) => {
                            out.push_str(&format!(
                                "[tool result id={} name={}]\n",
                                r.id,
                                tool_name(r)
                            ));
                            for rc in r.content.iter() {
                                if let ToolResultContent::Text(t) = rc {
                                    out.push_str(&truncate_chars(
                                        &t.text,
                                        SUMMARY_TOOL_RESULT_MAX_CHARS,
                                    ));
                                }
                            }
                            out.push('\n');
                        }
                        _ => {}
                    }
                }
            }
            Message::Assistant { content, .. } => {
                for c in content.iter() {
                    match c {
                        AssistantContent::Text(t) => {
                            out.push_str("[assistant]\n");
                            out.push_str(&t.text);
                            out.push('\n');
                        }
                        AssistantContent::Reasoning(r) => {
                            out.push_str("[assistant reasoning]\n");
                            out.push_str(&r.display_text());
                            out.push('\n');
                        }
                        AssistantContent::ToolCall(tc) => {
                            out.push_str(&format!(
                                "[tool call id={} name={}]\n{}\n",
                                tc.id, tc.function.name, tc.function.arguments
                            ));
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    out
}

/// 构造摘要请求输入：旧摘要 + 本次新淘汰的消息区间。
pub fn build_summary_prompt(previous: Option<&str>, new_messages: &[Message]) -> String {
    format!(
        "<previous-summary>\n{}\n</previous-summary>\n\n<new-history>\n{}</new-history>\n",
        previous.unwrap_or("(none)"),
        serialize_for_summary(new_messages)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig::OneOrMany;
    use rig::message::{Reasoning, Text, ToolCall, ToolFunction};

    fn user(text: &str) -> Message {
        Message::User {
            content: OneOrMany::one(UserContent::text(text)),
        }
    }

    fn assistant(text: &str) -> Message {
        Message::Assistant {
            id: None,
            content: OneOrMany::one(AssistantContent::text(text)),
        }
    }

    fn tool_call_msg(id: &str, name: &str) -> Message {
        Message::Assistant {
            id: None,
            content: OneOrMany::one(AssistantContent::ToolCall(ToolCall::new(
                id.into(),
                ToolFunction::new(name.into(), serde_json::json!({"arg": 1})),
            ))),
        }
    }

    fn tool_result_msg(id: &str, text: &str) -> Message {
        Message::User {
            content: OneOrMany::one(UserContent::ToolResult(ToolResult {
                id: id.into(),
                call_id: None,
                content: OneOrMany::one(ToolResultContent::Text(Text::new(text))),
            })),
        }
    }

    fn parallel_results_msg(ids: &[&str]) -> Message {
        Message::User {
            content: OneOrMany::many(
                ids.iter()
                    .map(|id| {
                        UserContent::ToolResult(ToolResult {
                            id: (*id).into(),
                            call_id: None,
                            content: OneOrMany::one(ToolResultContent::Text(Text::new("ok"))),
                        })
                    })
                    .collect::<Vec<_>>(),
            )
            .unwrap(),
        }
    }

    fn parallel_calls_msg(ids: &[&str]) -> Message {
        Message::Assistant {
            id: None,
            content: OneOrMany::many(
                ids.iter()
                    .map(|id| {
                        AssistantContent::ToolCall(ToolCall::new(
                            (*id).into(),
                            ToolFunction::new("read".into(), serde_json::json!({})),
                        ))
                    })
                    .collect::<Vec<_>>(),
            )
            .unwrap(),
        }
    }

    fn policy(window: u64, reserve: u64, keep: u64) -> ContextPolicy {
        ContextPolicy {
            window_tokens: window,
            reserve_tokens: reserve,
            keep_recent_tokens: keep,
        }
    }

    // ── 估算 ────────────────────────────────────────────────────────

    #[test]
    fn estimate_ascii_four_chars_per_token() {
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn estimate_non_ascii_one_char_per_token() {
        assert_eq!(estimate_tokens("你好世界"), 4);
        // 混合：4 ASCII = 1 token + 2 非 ASCII = 2 token
        assert_eq!(estimate_tokens("abcd你好"), 3);
    }

    #[test]
    fn estimate_message_includes_tool_json_and_reasoning() {
        let call = tool_call_msg("c1", "shell");
        let est = estimate_message(&call);
        // {"arg": 1} 10 ASCII + shell 5 → 约 4 token + 开销
        assert!(est > MESSAGE_OVERHEAD_TOKENS);
        let reasoning = Message::Assistant {
            id: None,
            content: OneOrMany::one(AssistantContent::Reasoning(Reasoning::new("思考一下"))),
        };
        assert!(estimate_message(&reasoning) >= MESSAGE_OVERHEAD_TOKENS + 4);
        let result = tool_result_msg("c1", &"x".repeat(400));
        assert!(estimate_message(&result) >= 100);
    }

    // ── usage 与投影 ────────────────────────────────────────────────

    #[test]
    fn effective_input_prefers_input_tokens_then_total_minus_output() {
        let mut usage = Usage::new();
        assert_eq!(effective_input_tokens(&usage), None);
        usage.input_tokens = 100;
        usage.output_tokens = 20;
        usage.total_tokens = 150;
        assert_eq!(effective_input_tokens(&usage), Some(130));
        usage.input_tokens = 200;
        assert_eq!(effective_input_tokens(&usage), Some(200));
    }

    #[test]
    fn projection_corrects_estimate_with_provider_sample() {
        let sample = UsageSample {
            input_tokens: 1000,
            estimated_request_tokens: 800,
        };
        // 新增内容估算 900 → 1000 + 900 - 800 = 1100
        assert_eq!(project_input(Some(sample), 900), 1100);
        assert_eq!(project_input(None, 900), 900);
        // 校准后为负时饱和到 0
        let weird = UsageSample {
            input_tokens: 10,
            estimated_request_tokens: 5000,
        };
        assert_eq!(project_input(Some(weird), 100), 0);
    }

    // ── 活动历史 ────────────────────────────────────────────────────

    #[test]
    fn active_history_without_checkpoint_is_unchanged() {
        let history = vec![user("a"), assistant("b")];
        let active = build_active_history(&ContextCheckpoint::default(), &history);
        assert_eq!(active, history);
    }

    #[test]
    fn active_history_prepends_summary_and_keeps_suffix() {
        let history = vec![user("a"), assistant("b"), user("c"), assistant("d")];
        let checkpoint = ContextCheckpoint {
            summary: Some("摘要是这个".into()),
            covered_messages: 2,
        };
        let active = build_active_history(&checkpoint, &history);
        assert_eq!(active.len(), 3);
        assert!(
            matches!(&active[0], Message::System { content } if content.contains("摘要是这个"))
        );
        assert_eq!(active[1], user("c"));
        assert_eq!(active[2], assistant("d"));
    }

    // ── 切分 ────────────────────────────────────────────────────────

    #[test]
    fn split_prefers_plain_user_turn_boundary() {
        // 每个 user 消息约 30 token；keep 预算只够最后 2 条多一点。
        let history = vec![
            user(&"u".repeat(100)),
            assistant(&"a".repeat(100)),
            user(&"u".repeat(100)),
            assistant(&"a".repeat(100)),
            user(&"u".repeat(100)),
            assistant(&"a".repeat(100)),
        ];
        let p = policy(100_000, 1000, 90);
        let prompt = user("go");
        match plan_split(&history, &prompt, &p, 0) {
            Some(k) => {
                // 预算内的合法切点中保留最多：user 边界（index 4）
                assert_eq!(k, 4, "should cut before the last plain user message");
                assert!(is_plain_user_message(&history[k]));
            }
            None => panic!("expected a split"),
        }
    }

    #[test]
    fn split_never_starts_with_orphan_tool_result() {
        let history = vec![
            user("查一下"),
            tool_call_msg("c1", "read"),
            tool_result_msg("c1", &"r".repeat(400)),
            assistant("读完了"),
        ];
        // keep 预算极小，只能保留极少内容
        let p = policy(100_000, 1000, 40);
        let prompt = user("继续");
        match plan_split(&history, &prompt, &p, 0) {
            Some(k) => {
                assert!(!is_tool_result_message(&history[k]));
                assert_eq!(k, 3, "must skip past the tool result message");
            }
            None => panic!("expected a split"),
        }
    }

    #[test]
    fn split_keeps_parallel_call_and_results_together() {
        let history = vec![
            user("读两个文件"),
            parallel_calls_msg(&["c1", "c2"]),
            parallel_results_msg(&["c1", "c2"]),
            assistant("都读完了"),
        ];
        let p = policy(100_000, 1000, 30);
        let prompt = user("好");
        let Some(k) = plan_split(&history, &prompt, &p, 0) else {
            panic!("expected a split");
        };
        // 不能切在批量 result 处（孤立 result），只能保留最后的 assistant
        assert_eq!(k, 3);
    }

    #[test]
    fn split_with_tool_result_prompt_keeps_matching_call() {
        let history = vec![user("跑命令"), tool_call_msg("c1", "shell")];
        let p = policy(100_000, 1000, 20);
        let prompt = tool_result_msg("c1", "done");
        // keep 预算只够 prompt 本身，但 call 必须保留 → k = 1
        let Some(k) = plan_split(&history, &prompt, &p, 0) else {
            panic!("expected a split");
        };
        assert_eq!(k, 1, "cut must keep the assistant message with the call");
    }

    #[test]
    fn split_turn_when_single_turn_too_big() {
        // 一个巨大 user turn（user + 多个 assistant），预算内无法整 turn 保留，
        // 允许在 assistant 边界 split-turn。
        let history = vec![
            user(&"u".repeat(200)),
            assistant(&"a".repeat(200)),
            assistant(&"b".repeat(200)),
        ];
        let p = policy(100_000, 1000, 120);
        let prompt = user("继续");
        let Some(k) = plan_split(&history, &prompt, &p, 0) else {
            panic!("expected a split");
        };
        assert!(matches!(&history[k], Message::Assistant { .. }));
        assert_eq!(k, 2);
    }

    #[test]
    fn split_impossible_when_prompt_result_has_no_call() {
        let history = vec![user("hi"), assistant("hello")];
        let p = policy(100_000, 1000, 20);
        let prompt = tool_result_msg("missing", "done");
        assert_eq!(plan_split(&history, &prompt, &p, 0), None);
    }

    #[test]
    fn split_impossible_when_indivisible_segment_exceeds_window() {
        // 窗口极小，连 prompt + 一条消息都装不下。
        let history = vec![user(&"u".repeat(500))];
        let p = policy(250, 100, 50);
        let prompt = user(&"p".repeat(500));
        assert_eq!(plan_split(&history, &prompt, &p, 0), None);
    }

    #[test]
    fn split_respects_min_covered_rolls_forward_only() {
        let history = vec![
            user(&"u".repeat(100)),
            assistant(&"a".repeat(100)),
            user(&"u".repeat(100)),
            assistant(&"a".repeat(100)),
        ];
        let p = policy(100_000, 1000, 250);
        let prompt = user("go");
        // min_covered = 2：即使预算允许保留更多，也只能从 2 之后切
        let Some(k) = plan_split(&history, &prompt, &p, 2) else {
            panic!("expected a split");
        };
        assert!(k >= 2);
    }

    // ── 决策 ────────────────────────────────────────────────────────

    #[test]
    fn decide_none_when_under_budget_and_no_checkpoint() {
        let p = policy(100_000, 1000, 2000);
        let history = vec![user("hi")];
        let prompt = user("go");
        let est = estimate_history(&history) + estimate_message(&prompt);
        let d = decide(
            &p,
            &ContextCheckpoint::default(),
            None,
            false,
            &history,
            &prompt,
            est,
        );
        assert_eq!(d, Decision::None);
    }

    #[test]
    fn decide_patches_active_history_when_checkpoint_exists() {
        let p = policy(100_000, 1000, 2000);
        let history = vec![user("a"), assistant("b"), user("c")];
        let checkpoint = ContextCheckpoint {
            summary: Some("old".into()),
            covered_messages: 2,
        };
        let prompt = user("go");
        let active = build_active_history(&checkpoint, &history);
        let est = estimate_history(&active) + estimate_message(&prompt);
        let d = decide(&p, &checkpoint, None, false, &history, &prompt, est);
        assert_eq!(d, Decision::PatchActive);
    }

    #[test]
    fn decide_compacts_when_over_budget() {
        let p = policy(300, 100, 120);
        let history = vec![
            user(&"u".repeat(200)),
            assistant(&"a".repeat(200)),
            user(&"u".repeat(200)),
            assistant(&"a".repeat(200)),
        ];
        let prompt = user("go");
        let est = estimate_history(&history) + estimate_message(&prompt);
        assert!(est >= p.trigger_tokens());
        let d = decide(
            &p,
            &ContextCheckpoint::default(),
            None,
            false,
            &history,
            &prompt,
            est,
        );
        assert!(matches!(d, Decision::Compact(k) if k > 0));
    }

    #[test]
    fn decide_terminate_when_forced_and_no_progress() {
        // 历史为空（或已覆盖到顶），force 也无切点可推进。
        let p = policy(300, 100, 50);
        let history = vec![user(&"u".repeat(500))];
        let checkpoint = ContextCheckpoint {
            summary: Some("s".into()),
            covered_messages: 1,
        };
        let prompt = user(&"p".repeat(500));
        let active = build_active_history(&checkpoint, &history);
        let est = estimate_history(&active) + estimate_message(&prompt);
        let d = decide(&p, &checkpoint, None, true, &history, &prompt, est);
        assert_eq!(d, Decision::Terminate);
    }

    #[test]
    fn decide_repeat_compaction_only_covers_new_range() {
        // 已覆盖 2 条，再次压缩只能从 2 之后推进。
        let p = policy(300, 100, 120);
        let history = vec![
            user(&"u".repeat(400)),
            assistant(&"a".repeat(400)),
            user(&"u".repeat(400)),
            assistant(&"a".repeat(400)),
        ];
        let checkpoint = ContextCheckpoint {
            summary: Some("old".into()),
            covered_messages: 2,
        };
        let prompt = user("go");
        let active = build_active_history(&checkpoint, &history);
        let est = estimate_history(&active) + estimate_message(&prompt);
        let d = decide(&p, &checkpoint, None, false, &history, &prompt, est);
        assert!(matches!(d, Decision::Compact(k) if k >= 2));
    }

    // ── 摘要输入 ────────────────────────────────────────────────────

    #[test]
    fn summary_input_truncates_tool_result_only() {
        let long = "r".repeat(5000);
        let messages = vec![
            tool_call_msg("c1", "read"),
            tool_result_msg("c1", &long),
            assistant(&"a".repeat(3000)),
        ];
        let input = serialize_for_summary(&messages);
        assert!(input.contains("[tool result id=c1 name=read]"));
        assert!(input.contains("…[truncated]"));
        assert!(!input.contains(&long));
        // assistant 文本不截断
        assert!(input.contains(&"a".repeat(3000)));
        assert!(input.contains("[tool call id=c1 name=read]"));
    }

    #[test]
    fn summary_prompt_wraps_previous_summary_and_new_history() {
        let messages = vec![user("新问题")];
        let prompt = build_summary_prompt(Some("旧摘要"), &messages);
        assert!(prompt.contains("<previous-summary>\n旧摘要\n</previous-summary>"));
        assert!(prompt.contains("<new-history>\n[user]\n新问题\n</new-history>"));
        let fresh = build_summary_prompt(None, &messages);
        assert!(fresh.contains("(none)"));
    }

    #[test]
    fn summary_input_keeps_call_id_and_tool_name() {
        let messages = vec![
            parallel_calls_msg(&["c1", "c2"]),
            parallel_results_msg(&["c1", "c2"]),
        ];
        let input = serialize_for_summary(&messages);
        assert!(input.contains("id=c1 name=read"));
        assert!(input.contains("id=c2 name=read"));
        assert!(input.matches("[tool result").count() == 2);
    }
}
