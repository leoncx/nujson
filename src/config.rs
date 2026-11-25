//! Optional TOML config file. Command-line options take precedence.
//!
//! ```toml
//! color = true          # JSON syntax highlighting
//! wrap = true           # wrap long lines in the detail pane
//! list_width = 40       # width of the line list, in % of the screen
//! view = '.ts + " " + .msg'   # default jq view
//!
//! [theme]               # color names, "#rrggbb" or 0-255 indexes
//! key = "blue"
//! string = "green"
//! number = "cyan"
//! boolean = "yellow"
//! null = "magenta"
//! match = "yellow"      # background of text-filter matches
//! ```

use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, bail};
use ratatui::style::Color;
use serde::Deserialize;

use crate::render::pretty::Theme;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub color: Option<bool>,
    pub wrap: Option<bool>,
    pub list_width: Option<u16>,
    pub view: Option<String>,
    pub theme: ThemeConfig,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeConfig {
    key: Option<String>,
    string: Option<String>,
    number: Option<String>,
    boolean: Option<String>,
    null: Option<String>,
    #[serde(rename = "match")]
    matched: Option<String>,
}

impl Config {
    /// Load `explicit` (must exist) or the default file (may be missing).
    pub fn load(explicit: Option<&Path>) -> Result<Self> {
        let (path, required) = match explicit {
            Some(p) => (p.to_path_buf(), true),
            None => match default_path() {
                Some(p) => (p, false),
                None => return Ok(Self::default()),
            },
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if !required && e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
        };
        Self::parse(&text).with_context(|| format!("invalid config {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self> {
        let config: Self = toml::from_str(text)?;
        if let Some(w) = config.list_width
            && !(10..=90).contains(&w)
        {
            bail!("list_width must be between 10 and 90, got {w}");
        }
        config.theme()?; // report bad colors at startup
        Ok(config)
    }

    /// The colored theme with configured overrides.
    pub fn theme(&self) -> Result<Theme> {
        let mut theme = Theme::colored();
        let t = &self.theme;
        for (name, value, style) in [
            ("key", &t.key, &mut theme.key),
            ("string", &t.string, &mut theme.string),
            ("number", &t.number, &mut theme.number),
            ("boolean", &t.boolean, &mut theme.boolean),
            ("null", &t.null, &mut theme.null),
        ] {
            if let Some(v) = value {
                *style = style.fg(parse_color(name, v)?);
            }
        }
        if let Some(v) = &t.matched {
            theme.matched = theme.matched.bg(parse_color("match", v)?);
        }
        Ok(theme)
    }
}

fn parse_color(name: &str, value: &str) -> Result<Color> {
    Color::from_str(value).map_err(|_| {
        anyhow::anyhow!(
            "theme.{name}: unknown color {value:?} (use a name like \"blue\", \"#rrggbb\" or 0-255)"
        )
    })
}

/// `$NUJSON_CONFIG`, else `$XDG_CONFIG_HOME/nujson/config.toml` or
/// `~/.config/nujson/config.toml` (Linux, macOS), `%APPDATA%\nujson\config.toml`
/// (Windows).
pub fn default_path() -> Option<PathBuf> {
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(p) = env("NUJSON_CONFIG") {
        return Some(p);
    }
    let dir = if cfg!(windows) {
        env("APPDATA")?
    } else {
        env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|h| h.join(".config")))?
    };
    Some(dir.join("nujson").join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_is_default() {
        let c = Config::parse("").unwrap();
        assert!(c.color.is_none() && c.wrap.is_none() && c.view.is_none());
        assert_eq!(c.theme().unwrap(), Theme::colored());
    }

    #[test]
    fn full_config() {
        let c = Config::parse(
            r##"
            color = false
            wrap = false
            list_width = 30
            view = ".msg"
            [theme]
            key = "light-red"
            null = "#808080"
            match = "33"
            "##,
        )
        .unwrap();
        assert_eq!(c.color, Some(false));
        assert_eq!(c.list_width, Some(30));
        assert_eq!(c.view.as_deref(), Some(".msg"));
        let t = c.theme().unwrap();
        assert_eq!(t.key.fg, Some(Color::LightRed));
        assert_eq!(t.null.fg, Some(Color::Rgb(128, 128, 128)));
        assert_eq!(t.matched.bg, Some(Color::Indexed(33)));
        assert_eq!(t.string, Theme::colored().string);
    }

    #[test]
    fn errors_are_reported() {
        let err = |s: &str| format!("{:#}", Config::parse(s).unwrap_err());
        assert!(err("colour = true").contains("unknown field"));
        assert!(err("list_width = 95").contains("between 10 and 90"));
        assert!(err("[theme]\nkey = \"blurple\"").contains("theme.key"));
    }

    #[test]
    fn missing_default_file_is_fine_but_explicit_is_not() {
        assert!(Config::load(Some(Path::new("/nonexistent/nujson.toml"))).is_err());
    }
}
