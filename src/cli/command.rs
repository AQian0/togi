use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "togi",
    version,
    about = cli_about(),
    long_about = cli_long_about()
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
fn cli_about() -> String {
    crate::t!("cli-about")
}

fn cli_long_about() -> String {
    crate::t!("cli-long-about")
}

