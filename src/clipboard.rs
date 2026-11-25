//! Copy text to the clipboard: the system clipboard when available (macOS,
//! Windows, X11/XWayland), else the OSC 52 escape sequence, which asks the
//! terminal to set the clipboard and also works over SSH.

use std::io::Write;

use base64::Engine;

#[derive(Default)]
pub struct Clipboard {
    /// Kept alive: on X11 the clipboard content is served by its owner.
    native: Option<arboard::Clipboard>,
    native_failed: bool,
}

impl Clipboard {
    /// Copy `text`; returns where it went, for the status message.
    pub fn copy(&mut self, text: &str) -> Result<&'static str, String> {
        if !self.native_failed {
            if self.native.is_none() {
                match arboard::Clipboard::new() {
                    Ok(c) => self.native = Some(c),
                    Err(_) => self.native_failed = true,
                }
            }
            if let Some(c) = &mut self.native
                && c.set_text(text).is_ok()
            {
                return Ok("clipboard");
            }
        }
        osc52(text).map_err(|e| format!("copy failed: {e}"))?;
        Ok("terminal clipboard (OSC 52)")
    }
}

fn osc52(text: &str) -> std::io::Result<()> {
    let mut out = std::io::stdout().lock();
    out.write_all(osc52_sequence(text).as_bytes())?;
    out.flush()
}

fn osc52_sequence(text: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    format!("\x1b]52;c;{encoded}\x07")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc52_encodes_base64() {
        assert_eq!(osc52_sequence("hi"), "\x1b]52;c;aGk=\x07");
    }
}
