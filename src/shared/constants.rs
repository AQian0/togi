//! 项目常量集中定义。
//!
//! 将分散在各模块中的魔法数字、字符串常量统一收敛到此文件，
//! 提高可读性并消除重复。

use std::time::Duration;

// ── 分页 ──────────────────────────────────────────────────────────

/// 工具输出默认每页行数。
pub(crate) const DEFAULT_PAGE_LINES: usize = 400;

// ── Agent ─────────────────────────────────────────────────────────

/// Agent 流式对话中允许的最大多轮工具调用循环次数。
pub(crate) const MAX_MULTI_TURN_ITERATIONS: u32 = 10;

/// 模型流停滞超时：超过此时间未收到任何流式内容即判定连接已死。
///  reasoning 模型的首 token 可能较慢，但流式增量应持续到达。
pub(crate) const STREAM_STALL_TIMEOUT: Duration = Duration::from_secs(120);

/// 等待工具结果的停滞宽限：工具自身超时上限加余量，余量覆盖结果收尾开销。
pub(crate) const TOOL_RESULT_GRACE: Duration =
    Duration::from_secs(MAX_TIMEOUT_SECS + TOOL_RESULT_GRACE_MARGIN_SECS);

/// [`TOOL_RESULT_GRACE`] 在工具超时上限之上的余量（秒）。
const TOOL_RESULT_GRACE_MARGIN_SECS: u64 = 30;

/// 系统提示词默认值。
pub(crate) const DEFAULT_PREAMBLE: &str = "\
你是一个运行在终端里的中文编程助手，回答要简洁、准确。\
你可以使用提供的工具来读取文件、修改文件、执行命令——\
具体的参数和用法见各工具自带的说明。\
所有工具都会自动接收运行时注入的必要信息（例如工作目录），\
不要臆造或手动填写隐藏参数；相对路径直接按当前工作目录解析。\
工具输出按行分页，需要查看更多内容时用工具的 offset / limit 参数翻页。";

// ── 上下文窗口管理 ──────────────────────────────────────────────────

/// `[context] reserve_tokens` 默认值。
pub(crate) const DEFAULT_RESERVE_TOKENS: u64 = 16_384;

/// `[context] keep_recent_tokens` 默认值。
pub(crate) const DEFAULT_KEEP_RECENT_TOKENS: u64 = 20_000;

// ── 时间间隔 ──────────────────────────────────────────────────────

/// 双击 Ctrl-C 退出窗口期。
pub(crate) const DOUBLE_PRESS_WINDOW: Duration = Duration::from_millis(500);

/// 输出渲染节流间隔（约 30 fps）。
pub(crate) const OUTPUT_RENDER_INTERVAL: Duration = Duration::from_millis(33);

/// 事件轮询间隔（毫秒），约 60 fps。
pub(crate) const POLL_INTERVAL_MS: u64 = 16;

// ── 终端 / UI 布局 ────────────────────────────────────────────────

/// 终端最小渲染宽度。
pub(crate) const MIN_TERMINAL_WIDTH: u16 = 6;

/// 终端最小渲染高度。
pub(crate) const MIN_TERMINAL_HEIGHT: u16 = 4;

/// 用户气泡的最大内容宽度相对终端宽度预留的左侧空间（字符数）。
///
/// 用于限制气泡宽度，使较长的用户消息折行成右侧的一列，而非铺满整行。
pub(crate) const USER_MARGIN: usize = 20;

/// 用户气泡距终端右边缘的外边距（字符数）。
pub(crate) const USER_EDGE_MARGIN: usize = 2;

/// 左侧块竖条 + 内边距宽度。
pub(crate) const GUTTER_W: usize = 2;

/// 块右侧内边距（字符数）：正文在此宽度处提前折行，背景色仍铺满到右边缘。
pub(crate) const BLOCK_RIGHT_PAD: usize = 2;

/// Markdown 水平线宽度。
pub(crate) const HORIZONTAL_RULE_WIDTH: usize = 60;

// ── 对话 / 编辑器 ─────────────────────────────────────────────────

/// 工具结果在对话中最多展示的行数。
pub(crate) const TOOL_RESULT_MAX_LINES: usize = 12;

/// 输入区最多展示的文本行数。
pub(crate) const MAX_TEXT_ROWS: usize = 10;

/// Tab 展开为的空格数。
pub(crate) const TAB_WIDTH: usize = 2;

// ── 工具调用摘要 ──────────────────────────────────────────────────

/// 摘要行内最大字符数。
pub(crate) const SUMMARY_MAX_INLINE_CHARS: usize = 72;

// ── Shell 工具 ────────────────────────────────────────────────────

/// Shell 命令默认超时（秒）。
pub(crate) const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// Shell 命令最大超时（秒）。
pub(crate) const MAX_TIMEOUT_SECS: u64 = 600;

/// Shell 输出总缓冲区上限（字节）。
pub(crate) const SHELL_MAX_OUTPUT_BYTES: usize = 256 * 1024;

/// Shell 读取缓冲区大小（字节）。
pub(crate) const SHELL_READ_BUFFER_SIZE: usize = 4096;

/// 交错模式 channel 容量。
pub(crate) const INTERLEAVED_CHANNEL_CAPACITY: usize = 64;

// ── 文件 IO ───────────────────────────────────────────────────────

/// 二进制检测采样大小（字节）。
pub(crate) const BINARY_DETECTION_SAMPLE_SIZE: usize = 8192;

/// 二进制判定：不可打印字符占比阈值。
pub(crate) const BINARY_NON_PRINTABLE_RATIO: f64 = 0.30;

/// UTF-8 边界对齐额外读取字节数。
pub(crate) const UTF8_ALIGNMENT_BUFFER: usize = 4;

/// Hexdump 每行字节数。
pub(crate) const HEXDUMP_BYTES_PER_ROW: usize = 16;

/// Hexdump 最大输出字节数。
pub(crate) const HEXDUMP_MAX_BYTES: usize = 512;

/// 超过此大小的文件视为"大文件"，触发流式读取和截断提示。
pub(crate) const LARGE_FILE_THRESHOLD: u64 = 10 * 1024 * 1024;

/// 工具允许加载的最大文件大小。超过此值直接拒绝。
pub(crate) const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024;

/// 默认最多返回的文本字节数（适用流式读取）。
pub(crate) const DEFAULT_MAX_READ_BYTES: u64 = 50 * 1024;

// ── 编辑 / Diff ────────────────────────────────────────────────────

/// Diff 输出上下文行数。
pub(crate) const DIFF_CONTEXT: usize = 3;

// ── 历史 ──────────────────────────────────────────────────────────

/// 持久化输入历史的最大条目数。
pub(crate) const MAX_HISTORY_ENTRIES: usize = 1000;

// ── 配置 / 环境 ──────────────────────────────────────────────────

/// 本地配置文件名。
pub(crate) const CONFIG_FILENAME: &str = "togi.toml";

/// Windows 配置文件名。
pub(crate) const WINDOWS_CONFIG_FILENAME: &str = "config.toml";

/// Windows 配置目录名。
pub(crate) const APP_DIR_NAME: &str = "togi";

/// 控制配置加载日志的环境变量。
pub(crate) const ENV_LOG_CONFIG: &str = "TOGI_LOG_CONFIG";

/// 自定义历史文件路径的环境变量。
pub(crate) const ENV_HISTORY_PATH: &str = "TOGI_HISTORY";

/// 自定义 Turso 数据库文件路径的环境变量。
pub(crate) const ENV_DB_PATH: &str = "TOGI_DB";

/// 数据库文件名。
pub(crate) const DB_FILENAME: &str = "togi.db";

/// 默认会话 ID。暂不支持多会话管理，全部对话归属此会话。
pub(crate) const DEFAULT_SESSION_ID: &str = "default";

// ── 临时文件 ──────────────────────────────────────────────────────

/// 原子写入临时文件后缀格式。
pub(crate) const TEMP_FILE_SUFFIX: &str = "togi";

/// 无文件名时的回退临时文件名。
pub(crate) const TEMP_FILE_FALLBACK_NAME: &str = "togi-tmp";

// ── Shell 渲染 ────────────────────────────────────────────────────

/// stdout 区段标题。
pub(crate) const STDOUT_SECTION_HEADER: &str = "--- stdout ---\n";

/// stderr 区段标题。
pub(crate) const STDERR_SECTION_HEADER: &str = "--- stderr ---\n";
