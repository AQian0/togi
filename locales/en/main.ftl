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
builtins-history-desc = Show current conversation history
builtins-sessions-desc = List all sessions
builtins-switch-desc = Switch to a session (number or ID)
builtins-new-desc = Create a new session
builtins-delete-desc = Delete a session
builtins-cwd-desc = Show current working directory
builtins-exit-desc = Exit (also via exit / quit or Ctrl-C)

builtins-clear-done = Cleared conversation history ({ $count } messages).
builtins-history-title = Conversation history ({ $count } messages)
builtins-history-empty = No conversation history yet.
builtins-cwd-display = Current working directory: { $cwd }
builtins-unknown-command = Unknown command `{ $command }`. Type /help to see available commands.
builtins-sessions-title = Sessions ({ $count })
builtins-sessions-empty = No sessions yet.
builtins-sessions-unavailable = Database unavailable, cannot manage sessions.
builtins-switch-usage = Usage: /switch <number|session-id>
builtins-switch-same = Already in the current session.
builtins-switch-done = Switched to session { $id } ({ $count } messages).
builtins-new-done = Created new session { $id } ({ $title }).
builtins-new-default-title = New session
builtins-delete-usage = Usage: /delete <number|session-id>
builtins-delete-current = Cannot delete the current session, switch to another session first.
builtins-delete-done = Deleted session { $id }.
builtins-session-not-found = Session `{ $input }` not found.
builtins-session-ambiguous = Session ID `{ $input }` matches multiple sessions, provide a longer prefix.

# ── App ──────────────────────────────────────────────────────────────
app-cancelled = Cancelled.
app-busy = Previous message is still being processed; this submission was ignored.
app-session-error = Session error: { $error }
app-history-save-error = Could not save input history: { $error }

# ── Agent ────────────────────────────────────────────────────────────
agent-esc-interrupted = Esc interrupted the current response.
agent-unknown-provider = Could not identify the provider for model "{ $model }". Supported providers: { $supported }
agent-missing-api-key = { $env } environment variable not set
agent-provider-init = Could not initialize { $provider } provider: { $source }
agent-stream-error = Model streaming failed: { $source }
agent-retrying = Request failed, retrying in { $delay }s (attempt { $attempt }/{ $max })…

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
error-unsupported-theme = Unsupported theme `{ $input }`, available themes: Latte, Frappe, Macchiato, Mocha.
error-bad-working-dir = The working directory `{ $path }` does not exist or is not a directory. Double-check the path, then retry.
error-shell-timeout = The command did not finish within { $secs } seconds and was killed. Increase `timeout_secs` or run a faster command.
error-shell-spawn = Failed to start the command in `{ $cwd }`: { $error }
error-shell-io = IO error while running the command: { $error }
error-old-text-not-found = Could not find the text to replace in `{ $path }`. Make sure `old_text` matches the file content exactly.
error-old-text-not-unique = Found multiple matches for `old_text` in `{ $path }`. Provide a longer context to make it unique.
error-overlapping-edits = Two or more edits target overlapping text in `{ $path }`. Make each `old_text` cover a distinct region.
error-file-too-large = `{ $path }` is { $size }, which exceeds the maximum allowed size of { $max }. Use the `shell` tool with `head`, `tail`, or `sed` to work with this file instead.
error-read-io = IO error while reading `{ $path }`: { $error }
error-modify-io = IO error while modifying `{ $path }`: { $error }
app-io-error = IO error during { $context }: { $error }
config-read-error = Could not read config file { $path }: { $error }
config-parse-error = Failed to parse config file { $path }: { $error }

# ── Conversation ─────────────────────────────────────────────────────
conv-user-label = You
conv-reasoning-label = Reasoning
conv-answer-label = Answer
conv-empty-output = (no output)
conv-folded-lines = … { $count } more lines (folded)
conv-retryable =  · retryable
conv-error-format = !! [{ $code } · { $kind }{ $retry }] { $message }

# ── Store ────────────────────────────────────────────────────────────
store-save-ok = Conversation saved ({ $count } messages).
store-save-error = Could not save conversation: { $error }
store-load-error = Could not load conversation history: { $error }
store-clear-error = Could not clear stored history: { $error }
store-open-error = Could not open database { $path }: { $error }
store-query-error = Database query failed: { $error }
store-deserialize-error = Could not deserialize stored message: { $error }

# ── Render ───────────────────────────────────────────────────────────
render-scroll-indicator = ── Back to bottom (PageDown) ──
