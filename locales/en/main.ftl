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
builtins-history-tool-result = [tool result]

# ── App ──────────────────────────────────────────────────────────────
app-cancelled = Cancelled.
app-busy = Previous message is still being processed; this submission was ignored.
app-session-error = Session error: { $error }
app-history-save-error = Could not save input history: { $error }
app-context-get-cwd = get current working directory

# ── Agent ────────────────────────────────────────────────────────────
agent-esc-interrupted = Esc interrupted the current response.
agent-unknown-provider = Could not identify the provider for model "{ $model }". Supported providers: { $supported }
agent-missing-api-key = { $env } environment variable not set
agent-provider-init = Could not initialize { $provider } provider: { $source }
agent-stream-error = Model streaming failed: { $source }
agent-retrying = Request failed, retrying in { $delay }s (attempt { $attempt }/{ $max })…
agent-stalled = Model stream stalled: no data for { $secs }s; the connection may be dead.
agent-unhandled-output = received unhandled provider output: { $value }

# ── Summarize ────────────────────────────────────────────────────────
summarize-write = Write
summarize-write-binary = Write · Binary
summarize-replace = Replace
summarize-edits = { $count } edit(s)
summarize-readonly-result = ({ $lines } lines read, content omitted)

# ── Common ───────────────────────────────────────────────────────────
common-no-changes = (no changes)
common-unit-text = text
common-truncation-notice = (showing { $shown } of { $total } { $unit }; use the `shell` tool with `tail`, `head`, or `sed` to inspect beyond this range)

# ── Config ───────────────────────────────────────────────────────────
config-loaded = [togi] Loaded config file: { $path }

# ── Errors (user-facing) ────────────────────────────────────────────
error-empty-path = `path` must not be empty.
error-empty-command = `command` must not be empty.
error-empty-old-text = `old_text` must not be empty. Provide the exact text to replace.
error-no-instructions = no edit instructions provided. Pass `content` to write the whole file, or `old_text` (and optional `new_text`) / `edits` to replace text in an existing file.
error-conflicting-instructions = conflicting instructions: `content` cannot be combined with `old_text`, `new_text`, or `edits`. Pass `content` alone to overwrite the file, or use the edit fields alone.
error-conflicting-base64 = `content_base64` cannot be combined with `content`, `old_text`, `new_text`, or `edits`. For binary writes, pass `content_base64` alone.
error-invalid-base64 = `content_base64` is not valid base64: { $error }
error-invalid-encoding = unsupported encoding `{ $enc }`. Use `hex`, `base64`, or a text encoding label such as `utf-8`, `utf-16le`, `gbk`, `shift_jis`, or `windows-1252`.
error-invalid-text-encoding = unsupported text encoding `{ $enc }`. Use a label such as `utf-8`, `utf-16le`, `gbk`, `shift_jis`, or `windows-1252`.
error-encoding-decode = { $encoding } decoding failed because the input contains malformed byte sequences
error-encoding-encode = { $encoding } encoding failed because the text contains characters not representable in that encoding
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
error-old-text-not-unique = Found multiple matches for `old_text` in `{ $path }` at lines [{ $lines }]. Provide a longer context to make it unique.
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

# ── Modify tool output ───────────────────────────────────────────────
modify-action-overwrote = overwrote
modify-action-created = created
modify-action-overwrite = overwrite
modify-action-create = create
modify-summary = { $action } `{ $display }` ({ $bytes } bytes, { $lines } lines).
modify-summary-sized = { $action } `{ $display }` ({ $size }).
modify-dry-run = [dry run] would { $action } `{ $display }`
modify-dry-run-sized = [dry run] would { $action } `{ $display }` ({ $size }).
modify-dry-run-edits = [dry run] would apply { $count } edit(s) to `{ $display }`.
modify-edited = edited `{ $display }` ({ $details }).
modify-edited-no-changes = edited `{ $display }` { $marker }.
modify-replacements = { $count } replacement(s)
modify-deletions = { $count } deletion(s)
modify-binary-no-diff = (binary — no diff available)
modify-diff-skipped-large = diff skipped — existing file is too large ({ $size })
modify-diff-skipped-decode = diff skipped — could not decode existing file: { $error }
modify-diff-skipped-read = diff skipped — could not read existing file: { $error }
modify-diff-skipped-warning = (warning: { $note }; use `shell` with `diff` to compare)
modify-mtime-warning = warning: file was modified externally before write
modify-suggestion-line = did you mean line { $line }: `{ $text }` ({ $chars } char(s))?
modify-suggestion-lines = did you mean lines { $start }-{ $end } ({ $chars } char(s))?

# ── Read tool output ─────────────────────────────────────────────────
read-empty-file = (empty file)
read-header-offset = `{ $display }` — { $size } (reading from byte { $offset })
read-header-first = `{ $display }` — { $size } (first { $read_size })
read-binary-header-hex = (binary) `{ $display }` — { $size }{ $range }
read-binary-header-base64 = (binary) `{ $display }` — { $size }, base64{ $range }:
read-binary-from-byte = (from byte { $offset })
read-binary-hex-truncated = (showing { $shown } of { $total } bytes; use `shell` with `xxd`, `file`, or `hexdump` for full content)
read-binary-base64-truncated = (warning: only { $shown } of the remaining { $total } were base64-encoded; pass `offset_bytes`/`limit_bytes` or use `shell` with `base64` for more)

# ── Shell tool output ────────────────────────────────────────────────
shell-cwd-line = cwd: { $cwd }
shell-exit-code-line = exit code: { $code }
shell-output-truncated = (output truncated at { $shown } of { $total }; use shell redirects or `head`/`tail` to inspect full output)

# ── Pagination ───────────────────────────────────────────────────────
paginate-offset-one-based = `offset` is 1-based and must be at least 1.
paginate-bad-integer = `{ $key }` must be a non-negative integer.
paginate-past-end = (offset { $offset } is past end of output; output has { $total } lines)
paginate-showing = (showing lines { $start }-{ $end } of { $total })
paginate-more-lines = … ({ $count } more lines; call again with offset { $offset })

# ── Render ───────────────────────────────────────────────────────────
render-scroll-indicator = ── Back to bottom (PageDown) ──
