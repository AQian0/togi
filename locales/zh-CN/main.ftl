# Chinese (Simplified) translations for togi UI

# ── CLI ──────────────────────────────────────────────────────────────
cli-about = 终端里的 AI 编程助手
cli-long-about = togi 是一个运行在终端里的 AI 编程助手，支持多模型切换、文件操作、Shell 命令执行等功能。

# ── Builtin Commands ─────────────────────────────────────────────────
builtins-help-title = 可用命令
builtins-shortcuts-title = 快捷键
builtins-shortcut-esc = Esc           取消当前回答 / 清空当前输入
builtins-shortcut-ctrl-c = Ctrl-C        退出
builtins-shortcut-page = PageUp/PageDown  滚动对话历史

builtins-help-desc = 显示这份帮助
builtins-clear-desc = 清空对话历史并重置屏幕
builtins-cwd-desc = 显示当前工作目录
builtins-exit-desc = 退出（也可用 exit / quit 或 Ctrl-C）

builtins-clear-done = 已清空对话历史（共 { $count } 条消息）。
builtins-cwd-display = 当前工作目录：{ $cwd }
builtins-unknown-command = 未知命令 `{ $command }`。输入 /help 查看可用命令。

# ── App ──────────────────────────────────────────────────────────────
app-cancelled = 已取消。
app-session-error = Session 错误：{ $error }
app-history-save-error = 无法保存输入历史：{ $error }

# ── Agent ────────────────────────────────────────────────────────────
agent-esc-interrupted = Esc 已中断当前回答。
agent-unknown-provider = 无法识别模型 "{ $model }" 对应的提供商。支持的提供商：{ $supported }
agent-missing-api-key = 未设置 { $env } 环境变量
agent-provider-init = 无法初始化 { $provider } 提供商：{ $source }
agent-stream-error = 模型流式响应失败：{ $source }

# ── Summarize ────────────────────────────────────────────────────────
summarize-write = 写入
summarize-write-binary = 写入 · 二进制
summarize-replace = 替换
summarize-edits = { $count } 处改动
summarize-readonly-result = （已读取 { $lines } 行，内容已省略）

# ── Common ───────────────────────────────────────────────────────────
common-no-changes = （无改动）
common-truncation-notice = （显示了 { $shown } / { $total } { $unit }；用 `shell` 工具的 `tail`、`head` 或 `sed` 命令查看其余部分）

# ── Config ───────────────────────────────────────────────────────────
config-loaded = [togi] 已加载配置文件：{ $path }

# ── Errors (user-facing) ────────────────────────────────────────────
error-not-found = 未找到文件：`{ $path }`。请检查路径后重试。
error-permission-denied = 权限不足，无法访问：`{ $path }`。
error-not-a-file = `{ $path }` 是目录，不是文件。请提供文件路径。
error-not-utf8 = `{ $path }` 不是有效的 UTF-8 文本，无法按字符串编辑。
error-default-model-init = 无法初始化默认模型 (deepseek-v4-pro)：{ $source }。请设置 DEEPSEEK_API_KEY 环境变量，或在 togi.toml 中通过 system.model 指定其他模型。
error-unsupported-theme = 不支持的主题 `{ $input }`，可用主题：Latte、Frappe、Macchiato、Mocha。
error-bad-working-dir = 工作目录 `{ $path }` 不存在或不是目录。请检查路径后重试。
error-shell-timeout = 命令在 { $secs } 秒内未完成，已被终止。请增大 `timeout_secs` 或运行更快的命令。
error-shell-spawn = 无法在 `{ $cwd }` 中启动命令：{ $error }
error-shell-io = 运行命令时发生 IO 错误：{ $error }
error-old-text-not-found = 在 `{ $path }` 中未找到要替换的文本。请检查 `old_text` 是否与文件内容完全一致。
error-old-text-not-unique = 在 `{ $path }` 中找到多处匹配 `old_text` 的文本。请提供更长的上下文使其唯一。
error-overlapping-edits = `{ $path }` 中有两处或更多编辑区域重叠。请确保每个 `old_text` 覆盖不同的区域。
error-file-too-large = `{ $path }` 大小为 { $size }，超过最大允许大小 { $max }。请使用 `shell` 工具的 `head`、`tail` 或 `sed` 命令查看。
error-read-io = 读取 `{ $path }` 时发生 IO 错误：{ $error }
error-modify-io = 修改 `{ $path }` 时发生 IO 错误：{ $error }
app-io-error = { $context }时发生 IO 错误：{ $error }
config-read-error = 无法读取配置文件 { $path }：{ $error }
config-parse-error = 配置文件 { $path } 解析失败：{ $error }

# ── Conversation ─────────────────────────────────────────────────────
conv-user-label = 用户
conv-reasoning-label = 思考过程
conv-answer-label = 回答
conv-empty-output = （无输出）
conv-folded-lines = … 其余 { $count } 行（已折叠）
conv-retryable =  · 可重试
conv-error-format = !! [{ $code } · { $kind }{ $retry }] { $message }

# ── Render ───────────────────────────────────────────────────────────
render-scroll-indicator = ── 回到底部 (PageDown) ──
