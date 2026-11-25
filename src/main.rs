mod app;
mod clipboard;
mod filter;
mod prompt;
mod render;
mod source;
mod ui;

use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;

use crate::app::App;
use crate::filter::{FilterKind, Spec};
use crate::source::{Source, Stream, spawn_reader};

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

    /// Keep reading the file as it grows, like `tail -f`; the selection sticks
    /// to the last line while it is on it.
    #[arg(short = 'F', long)]
    follow: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let color = !cli.no_color && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());

    let (name, source) = load(cli.file, cli.follow)?;
    let mut app = App::new(name, source, color);
    app.follow = cli.follow;
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

/// Open the input. A regular file is memory-mapped; stdin, pipes,
/// `<(cmd)` and followed files are read by a background thread, and lines show
/// up as they arrive.
fn load(file: Option<PathBuf>, follow: bool) -> Result<(PathBuf, Arc<Source>)> {
    let streaming = |name: PathBuf, stream: Stream| {
        let source = Arc::new(Source::streaming());
        spawn_reader(Arc::clone(&source), stream);
        (name, source)
    };
    match file {
        Some(path) if path.as_os_str() != "-" => {
            let meta = std::fs::metadata(&path)
                .with_context(|| format!("cannot open {}", path.display()))?;
            if meta.is_dir() {
                bail!("{} is a directory", path.display());
            }
            if follow {
                Ok(streaming(path.clone(), Stream::Follow(path)))
            } else if meta.is_file() {
                let source = Source::open(&path)?;
                Ok((path, Arc::new(source)))
            } else {
                Ok(streaming(path.clone(), Stream::File(path)))
            }
        }
        _ => {
            if follow {
                bail!("--follow needs a file; piped input is always streamed");
            }
            if std::io::stdin().is_terminal() {
                bail!("no input: pass a file or pipe data, e.g. `cat app.log | nujson`");
            }
            // Keys are then read from the controlling terminal (/dev/tty or
            // CONIN$) by crossterm, not from stdin.
            Ok(streaming(PathBuf::from("stdin"), Stream::Stdin))
        }
    }
}
