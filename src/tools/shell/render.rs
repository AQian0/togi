use super::capture::{StreamCapture, StreamChunk};
use super::process;
use crate::common::format_size;
use crate::constants;
use std::fmt::Write;
use std::path::Path;
use std::process::ExitStatus;

pub(super) fn render_separated(
    status: ExitStatus,
    stdout: &StreamCapture,
    stderr: &StreamCapture,
    cwd: &Path,
) -> String {
    let stdout_str = String::from_utf8_lossy(&stdout.data);
    let stderr_str = String::from_utf8_lossy(&stderr.data);
    let code = process::exit_description(status);
    let total_output_len = stdout.total + stderr.total;
    let stored_output_len = stdout.data.len() + stderr.data.len();
    let truncated = total_output_len > stored_output_len;

    let stdout_display = stdout_str.trim_end_matches('\n');
    let stderr_display = stderr_str.trim_end_matches('\n');
    let estimated = stdout_display.len() + stderr_display.len() + code.len() + 256;
    let mut out = String::with_capacity(estimated);

    write!(out, "cwd: {}\nexit code: {}\n", cwd.display(), code).unwrap();

    if stdout_display.is_empty() && stderr_display.is_empty() {
        out.push_str("(no output)");
        return out;
    }
    if !stdout_display.is_empty() {
        out.push_str(constants::STDOUT_SECTION_HEADER);
        out.push_str(stdout_display);
        out.push('\n');
    }
    if !stderr_display.is_empty() {
        out.push_str(constants::STDERR_SECTION_HEADER);
        out.push_str(stderr_display);
        out.push('\n');
    }

    if truncated {
        out.push_str(&format!(
            "(output truncated at {} of {}; use shell redirects or `head`/`tail` to \
             inspect full output)",
            format_size(stored_output_len as u64),
            format_size(total_output_len as u64),
        ));
    }

    out.truncate(out.trim_end_matches('\n').len());
    out
}

pub(super) fn render_interleaved(
    status: ExitStatus,
    chunks: &[StreamChunk],
    total_bytes: usize,
    stored_bytes: usize,
    cwd: &Path,
) -> String {
    let code = process::exit_description(status);
    let mut out = String::with_capacity(
        (stored_bytes.min(constants::SHELL_MAX_OUTPUT_BYTES + 512)) + (chunks.len() * 8) + 256,
    );

    write!(out, "cwd: {}\nexit code: {}\n", cwd.display(), code).unwrap();

    if chunks.is_empty() {
        out.push_str("(no output)");
        return out;
    }

    let truncated = total_bytes > stored_bytes;

    let mut last_source: Option<bool> = None;
    for (is_stdout, data) in chunks {
        let display_text = String::from_utf8_lossy(data);
        if display_text.is_empty() {
            continue;
        }

        if last_source == Some(*is_stdout) {
            out.push_str(&display_text);
        } else {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            let header = if *is_stdout {
                constants::STDOUT_SECTION_HEADER
            } else {
                constants::STDERR_SECTION_HEADER
            };
            out.push_str(header);
            out.push_str(&display_text);
            last_source = Some(*is_stdout);
        }
    }

    if truncated {
        out.push_str(&format!(
            "\n(output truncated at {} of {}; use shell redirects or `head`/`tail` to \
             inspect full output)",
            format_size(stored_bytes as u64),
            format_size(total_bytes as u64),
        ));
    }

    out.truncate(out.trim_end_matches('\n').len());
    out
}
