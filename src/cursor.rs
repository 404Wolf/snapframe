//! Hides Hyprland's cursor around still captures. With software cursors
//! (`cursor:no_hardware_cursors`) the compositor draws the cursor into every
//! frame, so `grim` would otherwise capture it. Recordings keep the cursor.

use std::process::Command;
use std::thread;
use std::time::Duration;

/// How long to let the compositor render a frame without the cursor.
const SETTLE: Duration = Duration::from_millis(150);

/// Restores the cursor's previous visibility when dropped, including on an
/// early return or a failed capture.
#[must_use = "the cursor reappears as soon as this is dropped"]
pub struct Hidden {
    /// The `cursor:invisible` value to put back, if we changed it.
    restore: Option<bool>,
}

/// Hides the cursor until the returned guard is dropped. Best effort: if
/// Hyprland can't be asked, the capture goes ahead with the cursor visible.
pub fn hide() -> Hidden {
    let Some(was_invisible) = invisible() else {
        return Hidden { restore: None };
    };
    if was_invisible || !set_invisible(true) {
        return Hidden { restore: None };
    }
    thread::sleep(SETTLE);
    Hidden {
        restore: Some(was_invisible),
    }
}

impl Drop for Hidden {
    fn drop(&mut self) {
        if let Some(value) = self.restore.take() {
            set_invisible(value);
        }
    }
}

fn invisible() -> Option<bool> {
    let out = Command::new("hyprctl")
        .args(["getoption", "cursor:invisible", "-j"])
        .output()
        .ok()
        .filter(|out| out.status.success())?;
    parse_option(&out.stdout)
}

/// Reads `{"option": "...", "bool": true, "set": ...}` from `hyprctl getoption -j`.
fn parse_option(json: &[u8]) -> Option<bool> {
    let value: serde_json::Value = serde_json::from_slice(json).ok()?;
    value
        .get("bool")
        .and_then(serde_json::Value::as_bool)
        .or_else(|| {
            value
                .get("int")
                .and_then(serde_json::Value::as_i64)
                .map(|v| v != 0)
        })
}

fn set_invisible(value: bool) -> bool {
    let lua = format!("hl.config({{ cursor = {{ invisible = {value} }} }})");
    Command::new("hyprctl")
        .args(["eval", &lua])
        .output()
        .is_ok_and(|out| out.status.success() && out.stdout.trim_ascii() == b"ok")
}

#[cfg(test)]
mod tests {
    use super::parse_option;

    #[test]
    fn reads_bool_and_int_options() {
        assert_eq!(
            parse_option(br#"{"option":"cursor:invisible","bool":true,"set":true}"#),
            Some(true)
        );
        assert_eq!(
            parse_option(br#"{"option":"cursor:invisible","bool":false,"set":false}"#),
            Some(false)
        );
        assert_eq!(
            parse_option(br#"{"option":"cursor:invisible","int":1}"#),
            Some(true)
        );
        assert_eq!(parse_option(b"not json"), None);
    }
}
