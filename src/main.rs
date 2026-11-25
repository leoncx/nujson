mod app;
mod filter;
mod prompt;
mod render;
mod source;
mod ui;

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;

use crate::app::App;
use crate::filter::{FilterKind, Spec};
use crate::source::Source;

/// Terminal viewer for NDJSON files.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// NDJSON file to open.
    file: PathBuf,

    /// Disable JSON syntax highlighting (also honored via the NO_COLOR env var).
    #[arg(long)]
    no_color: bool,

    /// jq expression applied at startup; only lines where it yields a value
    /// other than false/null are listed (e.g. '.level == "error"').
    #[arg(short, long, value_name = "EXPR")]
    filter: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if !cli.file.is_file() {
        bail!("not a readable file: {}", cli.file.display());
    }
    let color = !cli.no_color && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());

    let source = Source::open(&cli.file)?;
    let mut app = App::new(cli.file, source, color);
    if let Some(expr) = cli.filter {
        app.start_filter(Spec::new(FilterKind::Jq, expr))
            .map_err(|e| anyhow!("invalid --filter: {e}"))?;
    }

    // `ratatui::init` enters raw mode + alternate screen and installs a panic
    // hook that restores the terminal before printing the panic message.
    let mut terminal = ratatui::init();
    let result = app.run(&mut terminal).context("application error");
    ratatui::restore();
    result
}
