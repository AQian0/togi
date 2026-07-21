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
builtins-history-desc = 显示当前对话历史
builtins-sessions-desc = 列出所有会话
builtins-switch-desc = 切换到指定会话（编号或 ID）
builtins-new-desc = 创建新会话
builtins-delete-desc = 删除指定会话
builtins-cwd-desc = 显示当前工作目录
builtins-exit-desc = 退出（也可用 exit / quit 或 Ctrl-C）

builtins-clear-done = 已清空对话历史（共 { $count } 条消息）。
builtins-history-title = 对话历史（共 { $count } 条消息）
builtins-history-empty = 暂无对话历史。
builtins-cwd-display = 当前工作目录：{ $cwd }
builtins-unknown-command = 未知命令 `{ $command }`。输入 /help 查看可用命令。
builtins-sessions-title = 会话列表（共 { $count } 个）
builtins-sessions-empty = 暂无会话。
builtins-sessions-unavailable = 数据库不可用，无法管理会话。
builtins-switch-usage = 用法：/switch <编号|会话ID>
builtins-switch-same = 已在当前会话中。
builtins-switch-done = 已切换到会话 { $id }（共 { $count } 条消息）。
builtins-new-done = 已创建新会话 { $id }（{ $title }）。
builtins-new-default-title = 新会话
builtins-delete-usage = 用法：/delete <编号|会话ID>
builtins-delete-current = 无法删除当前会话，请先切换到其他会话。
builtins-delete-done = 已删除会话 { $id }。
builtins-session-not-found = 未找到会话 `{ $input }`。
builtins-session-ambiguous = 会话 ID `{ $input }` 匹配到多个会话，请提供更长的前缀。
builtins-history-tool-result = [工具结果]

# ── App ──────────────────────────────────────────────────────────────
app-cancelled = 已取消。
app-busy = 上一条消息仍在处理中，本次提交已被忽略。
app-session-error = Session 错误：{ $error }
app-history-save-error = 无法保存输入历史：{ $error }
app-context-get-cwd = 获取当前工作目录

# ── Agent ────────────────────────────────────────────────────────────
agent-esc-interrupted = Esc 已中断当前回答。
agent-unknown-provider = 无法识别模型 "{ $model }" 对应的提供商。支持的提供商：{ $supported }
agent-missing-api-key = 未设置 { $env } 环境变量
agent-provider-init = 无法初始化 { $provider } 提供商：{ $source }
agent-stream-error = 模型流式响应失败：{ $source }
agent-retrying = 请求失败，{ $delay } 秒后重试（第 { $attempt }/{ $max } 次）…
agent-stalled = 模型响应停滞：{ $secs } 秒未收到任何数据，连接可能已中断。
agent-unhandled-output = 收到未处理的提供商输出：{ $value }

# ── Summarize ────────────────────────────────────────────────────────
summarize-write = 写入
summarize-write-binary = 写入 · 二进制
summarize-replace = 替换
summarize-edits = { $count } 处改动
summarize-readonly-result = （已读取 { $lines } 行，内容已省略）

# ── Common ───────────────────────────────────────────────────────────
common-no-changes = （无改动）
common-unit-text = 文本
common-truncation-notice = （显示了 { $shown } / { $total } { $unit }；用 `shell` 工具的 `tail`、`head` 或 `sed` 命令查看其余部分）

# ── Config ───────────────────────────────────────────────────────────
config-loaded = [togi] 已加载配置文件：{ $path }

# ── Errors (user-facing) ────────────────────────────────────────────
error-empty-path = `path` 不能为空。
error-empty-command = `command` 不能为空。
error-empty-old-text = `old_text` 不能为空。请提供要替换的原始文本。
error-no-instructions = 未提供编辑指令。传入 `content` 写入整个文件，或传入 `old_text`（及可选的 `new_text`）/ `edits` 替换已有文件中的文本。
error-conflicting-instructions = 指令冲突：`content` 不能与 `old_text`、`new_text` 或 `edits` 同时使用。请单独传 `content` 覆盖文件，或只使用编辑字段。
error-conflicting-base64 = `content_base64` 不能与 `content`、`old_text`、`new_text` 或 `edits` 同时使用。二进制写入请单独传 `content_base64`。
error-invalid-base64 = `content_base64` 不是有效的 base64：{ $error }
error-invalid-encoding = 不支持的编码 `{ $enc }`。可用 `hex`、`base64`，或 `utf-8`、`utf-16le`、`gbk`、`shift_jis`、`windows-1252` 等文本编码标签。
error-invalid-text-encoding = 不支持的文本编码 `{ $enc }`。可用 `utf-8`、`utf-16le`、`gbk`、`shift_jis`、`windows-1252` 等标签。
error-encoding-decode = { $encoding } 解码失败：输入包含损坏的字节序列
error-encoding-encode = { $encoding } 编码失败：文本包含该编码无法表示的字符
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
error-old-text-not-unique = 在 `{ $path }` 中找到多处匹配 `old_text` 的文本（第 { $lines } 行）。请提供更长的上下文使其唯一。
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

# ── Store ────────────────────────────────────────────────────────────
store-save-ok = 对话已保存（共 { $count } 条消息）。
store-save-error = 无法保存对话：{ $error }
store-load-error = 无法加载对话历史：{ $error }
store-clear-error = 无法清除已存储的历史：{ $error }
store-open-error = 无法打开数据库 { $path }：{ $error }
store-query-error = 数据库查询失败：{ $error }
store-deserialize-error = 无法反序列化存储的消息：{ $error }

# ── Modify tool output ───────────────────────────────────────────────
modify-action-overwrote = 已覆盖
modify-action-created = 已创建
modify-action-overwrite = 覆盖
modify-action-create = 创建
modify-summary = { $action } `{ $display }`（{ $bytes } 字节，{ $lines } 行）。
modify-summary-sized = { $action } `{ $display }`（{ $size }）。
modify-dry-run = [dry run] 将{ $action } `{ $display }`
modify-dry-run-sized = [dry run] 将{ $action } `{ $display }`（{ $size }）。
modify-dry-run-edits = [dry run] 将对 `{ $display }` 应用 { $count } 处编辑。
modify-edited = 已编辑 `{ $display }`（{ $details }）。
modify-edited-no-changes = 已编辑 `{ $display }`{ $marker }。
modify-replacements = { $count } 处替换
modify-deletions = { $count } 处删除
modify-binary-no-diff = （二进制 — 无 diff 可用）
modify-diff-skipped-large = 跳过 diff — 现有文件过大（{ $size }）
modify-diff-skipped-decode = 跳过 diff — 无法解码现有文件：{ $error }
modify-diff-skipped-read = 跳过 diff — 无法读取现有文件：{ $error }
modify-diff-skipped-warning = （警告：{ $note }；可用 `shell` 的 `diff` 命令对比）
modify-mtime-warning = 警告：文件在写入前被外部修改
modify-suggestion-line = 是否指第 { $line } 行：`{ $text }`（相差 { $chars } 个字符）？
modify-suggestion-lines = 是否指第 { $start }-{ $end } 行（相差 { $chars } 个字符）？

# ── Read tool output ─────────────────────────────────────────────────
read-empty-file = （空文件）
read-header-offset = `{ $display }` — { $size }（从字节 { $offset } 开始读取）
read-header-first = `{ $display }` — { $size }（前 { $read_size }）
read-binary-header-hex = （二进制）`{ $display }` — { $size }{ $range }
read-binary-header-base64 = （二进制）`{ $display }` — { $size }，base64{ $range }：
read-binary-from-byte = （从字节 { $offset } 开始）
read-binary-hex-truncated = （显示 { $shown } / { $total } 字节；用 `shell` 的 `xxd`、`file` 或 `hexdump` 查看完整内容）
read-binary-base64-truncated = （警告：仅对其余 { $total } 中的 { $shown } 做了 base64 编码；传 `offset_bytes`/`limit_bytes` 或用 `shell` 的 `base64` 获取更多）

# ── Shell tool output ────────────────────────────────────────────────
shell-cwd-line = cwd：{ $cwd }
shell-exit-code-line = 退出码：{ $code }
shell-output-truncated = （输出已在 { $total } 中的 { $shown } 处截断；用 shell 重定向或 `head`/`tail` 查看完整输出）

# ── Pagination ───────────────────────────────────────────────────────
paginate-offset-one-based = `offset` 从 1 开始，必须至少为 1。
paginate-bad-integer = `{ $key }` 必须是非负整数。
paginate-past-end = （offset { $offset } 超出输出末尾；输出共 { $total } 行）
paginate-showing = （显示第 { $start }-{ $end } 行，共 { $total } 行）
paginate-more-lines = …（还有 { $count } 行；用 offset { $offset } 再次调用）

# ── Render ───────────────────────────────────────────────────────────
render-scroll-indicator = ── 回到底部 (PageDown) ──
