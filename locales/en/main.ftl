# English translations for togi UI

# ── CLI ──────────────────────────────────────────────────────────────
cli-about = AI coding assistant in your terminal
cli-long-about = togi is an AI coding assistant that runs in your terminal, supporting multiple model providers, file operations, shell command execution and more.

# ── Builtin Commands ─────────────────────────────────────────────────
builtins-help-title = Available Commands
builtins-shortcuts-title = Shortcuts
builtins-shortcut-esc = Esc           Cancel current response / clear input
builtins-shortcut-ctrl-c = Ctrl-C        Quit
builtins-shortcut-page = PageUp/PageDown  Scroll conversation history

builtins-help-desc = Show this help
builtins-clear-desc = Clear conversation history and reset screen
builtins-cwd-desc = Show current working directory
builtins-exit-desc = Exit (also via exit / quit or Ctrl-C)

builtins-clear-done = Cleared conversation history ({ $count } messages).
builtins-cwd-display = Current working directory: { $cwd }
builtins-unknown-command = Unknown command `{ $command }`. Type /help to see available commands.

# ── App ──────────────────────────────────────────────────────────────
app-cancelled = Cancelled.
app-session-error = Session error: { $error }
app-history-save-error = Could not save input history: { $error }

# ── Agent ────────────────────────────────────────────────────────────
agent-esc-interrupted = Esc interrupted the current response.
agent-unknown-provider = Could not identify the provider for model "{ $model }". Supported providers: { $supported }
agent-missing-api-key = { $env } environment variable not set
agent-provider-init = Could not initialize { $provider } provider: { $source }
agent-stream-error = Model streaming failed: { $source }

# ── Summarize ────────────────────────────────────────────────────────
summarize-write = Write
summarize-write-binary = Write · Binary
summarize-replace = Replace
summarize-edits = { $count } edit(s)
summarize-readonly-result = ({ $lines } lines read, content omitted)

# ── Common ───────────────────────────────────────────────────────────
common-no-changes = (no changes)
common-truncation-notice = (showing { $shown } of { $total } { $unit }; use the `shell` tool with `tail`, `head`, or `sed` to inspect beyond this range)

# ── Config ───────────────────────────────────────────────────────────
config-loaded = [togi] Loaded config file: { $path }

# ── Errors (user-facing) ────────────────────────────────────────────
error-not-found = File not found: `{ $path }`. Double-check the path, then retry.
error-permission-denied = Permission denied: `{ $path }`.
error-not-a-file = `{ $path }` is a directory, not a file. Provide a file path.
error-not-utf8 = `{ $path }` is not valid UTF-8 text and cannot be edited as a string.
error-default-model-init = Could not initialize the default model (deepseek-v4-pro): { $source }. Set the DEEPSEEK_API_KEY environment variable, or specify a different model in togi.toml via system.model.

# ── Conversation ─────────────────────────────────────────────────────
conv-user-label = You
conv-reasoning-label = Reasoning
conv-answer-label = Answer
conv-empty-output = (no output)
conv-folded-lines = … { $count } more lines (folded)
conv-retryable =  · retryable
conv-error-format = !! [{ $code } · { $kind }{ $retry }] { $message }

# ── Render ───────────────────────────────────────────────────────────
render-scroll-indicator = ── Back to bottom (PageDown) ──
