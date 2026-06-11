use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "togi",
    version,
    about = "终端里的 AI 编程助手",
    long_about = "togi 是一个运行在终端里的 AI 编程助手，支持多模型切换、文件操作、Shell 命令执行等功能。"
)]
pub struct Args {
    #[arg(short = 'm', long = "model", value_name = "MODEL")]
    pub model: Option<String>,

    #[arg(short = 'k', long = "key", value_name = "KEY")]
    pub api_key: Option<String>,

    /// Catppuccin theme flavor: Latte, Frappe, Macchiato, Mocha
    #[arg(long = "theme", value_name = "FLAVOR")]
    pub theme: Option<String>,
}
impl Args {
    pub fn parse() -> Self {
        <Self as Parser>::parse()
    }
}
