use super::capture::StreamCapture;
use super::process;
use crate::shared::constants;
use crate::shared::util::format_size;
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

    write!(
        out,
        "{}\n{}\n",
        crate::t!("shell-cwd-line", cwd = cwd.display().to_string()),
        crate::t!("shell-exit-code-line", code = code)
    )
    .unwrap();

    if stdout_display.is_empty() && stderr_display.is_empty() {
        out.push_str(&crate::t!("conv-empty-output"));
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
        let _ = write!(
            out,
            "{}",
            crate::t!(
                "shell-output-truncated",
                shown = format_size(stored_output_len as u64),
                total = format_size(total_output_len as u64)
            ),
        );
    }

    out.truncate(out.trim_end_matches('\n').len());
    out
}
