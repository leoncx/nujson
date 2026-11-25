mod app;
mod filter;
mod prompt;
mod render;
mod source;
mod ui;

use std::fs::File;
use std::io::IsTerminal;
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
    /// NDJSON file to open; reads stdin when omitted or `-`.
    file: Option<PathBuf>,

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

    let color = !cli.no_color && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());

    let (name, source) = load(cli.file)?;
    let mut app = App::new(name, source, color);
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

/// Open the input: a regular file is memory-mapped; stdin and other streams
/// (pipes, `<(cmd)` process substitution) are read fully into memory.
fn load(file: Option<PathBuf>) -> Result<(PathBuf, Source)> {
    match file {
        Some(path) if path.as_os_str() != "-" => {
            let meta = std::fs::metadata(&path)
                .with_context(|| format!("cannot open {}", path.display()))?;
            let source = if meta.is_file() {
                Source::open(&path)?
            } else if meta.is_dir() {
                bail!("{} is a directory", path.display());
            } else {
                let f =
                    File::open(&path).with_context(|| format!("cannot open {}", path.display()))?;
                Source::read_all(f)?
            };
            Ok((path, source))
        }
        _ => {
            let stdin = std::io::stdin();
            if stdin.is_terminal() {
                bail!("no input: pass a file or pipe data, e.g. `cat app.log | nujson`");
            }
            // Keys are then read from the controlling terminal (/dev/tty or
            // CONIN$) by crossterm, not from stdin.
            Ok((PathBuf::from("stdin"), Source::read_all(stdin.lock())?))
        }
    }
}
