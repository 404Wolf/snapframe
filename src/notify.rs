//! The "saved" notification, with actions to open the file or its folder.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::Result;

/// Shows the notification and waits for an action. Blocks; run detached.
pub fn saved(path: &Path, copied: bool) -> Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let body = if copied {
        format!("{name}\nCopied to the clipboard")
    } else {
        name
    };
    let mut notification = notify_rust::Notification::new();
    notification
        .appname("snapframe")
        .summary("Capture saved")
        .body(&body)
        .icon("camera-photo")
        .action("default", "Open")
        .action("open", "Open")
        .action("folder", "Show in folder");
    if path.extension().is_some_and(|e| e == "png") {
        notification.image_path(&path.to_string_lossy());
    }
    let handle = notification.show()?;
    let file = path.to_path_buf();
    handle.wait_for_action(move |action| {
        let target = match action {
            "default" | "open" => Some(file.clone()),
            "folder" => file.parent().map(Path::to_path_buf),
            _ => None,
        };
        if let Some(target) = target {
            let _ = Command::new("xdg-open")
                .arg(target)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
        }
    });
    Ok(())
}

/// A plain notification for progress and errors.
pub fn message(summary: &str, body: &str) {
    let _ = notify_rust::Notification::new()
        .appname("snapframe")
        .summary(summary)
        .body(body)
        .icon("camera-photo")
        .show();
}
