#[tokio::main]
async fn main() {
    init_logging();
    // 致命错误走本地化的 user_message（Termination 默认只打印 Debug 结构）。
    if let Err(err) = togi::app::run().await {
        use togi::error::TogiError;
        tracing::error!(code = err.code(), error = %err, "fatal error");
        eprintln!("{}", err.user_message());
        std::process::exit(1);
    }
}

/// 调试日志写文件（TUI 占用终端，不能写 stderr）。
/// `TOGI_LOG` 控制过滤（默认 `togi=debug`），`TOGI_LOG_FILE` 控制路径
/// （默认 `<临时目录>/togi.log`）。
fn init_logging() {
    let path = std::env::var_os("TOGI_LOG_FILE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("togi.log"));
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    let filter = tracing_subscriber::EnvFilter::try_from_env("TOGI_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("togi=debug"));
    if tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::sync::Mutex::new(file))
        .with_ansi(false)
        .try_init()
        .is_ok()
    {
        tracing::debug!(path = %path.display(), "logging initialized");
    }
}
