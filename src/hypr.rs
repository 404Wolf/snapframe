//! Window geometry from Hyprland, for click-to-select. Optional: without
//! Hyprland the overlay still selects regions and outputs.

use std::process::Command;

use serde::Deserialize;

use crate::geom::Rect;

#[derive(Debug, Deserialize)]
struct Workspace {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Client {
    #[serde(default = "yes")]
    mapped: bool,
    #[serde(default)]
    hidden: bool,
    at: [f64; 2],
    size: [f64; 2],
    workspace: Workspace,
    #[serde(default)]
    floating: bool,
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    focus_history_id: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Monitor {
    active_workspace: Workspace,
    special_workspace: Option<Workspace>,
}

fn yes() -> bool {
    true
}

/// Visible windows, topmost first, or none when Hyprland isn't reachable.
pub fn visible_windows() -> Vec<Rect> {
    let run = |what: &str| -> Option<Vec<u8>> {
        let out = Command::new("hyprctl").args([what, "-j"]).output().ok()?;
        out.status.success().then_some(out.stdout)
    };
    match (run("clients"), run("monitors")) {
        (Some(clients), Some(monitors)) => parse(&clients, &monitors).unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn parse(clients: &[u8], monitors: &[u8]) -> Option<Vec<Rect>> {
    let clients: Vec<Client> = serde_json::from_slice(clients).ok()?;
    let monitors: Vec<Monitor> = serde_json::from_slice(monitors).ok()?;
    let special: Vec<&str> = monitors
        .iter()
        .filter_map(|m| m.special_workspace.as_ref())
        .map(|w| w.name.as_str())
        .filter(|n| !n.is_empty())
        .collect();
    let active: Vec<&str> = monitors
        .iter()
        .map(|m| m.active_workspace.name.as_str())
        .collect();

    let mut visible: Vec<(u8, i64, Rect)> = clients
        .iter()
        .filter(|c| c.mapped && !c.hidden && c.size[0] > 0.0 && c.size[1] > 0.0)
        .filter_map(|c| {
            let ws = c.workspace.name.as_str();
            // Lower ranks draw above higher ones.
            let rank = if special.contains(&ws) {
                0
            } else if c.pinned || (active.contains(&ws) && c.floating) {
                1
            } else if active.contains(&ws) {
                2
            } else {
                return None;
            };
            let rect = Rect::new(c.at[0], c.at[1], c.size[0], c.size[1]);
            Some((rank, c.focus_history_id, rect))
        })
        .collect();
    visible.sort_by_key(|(rank, focus, _)| (*rank, *focus));
    Some(visible.into_iter().map(|(_, _, r)| r).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONITORS: &str = r#"[{"activeWorkspace":{"id":1,"name":"1"},
        "specialWorkspace":{"id":0,"name":""}}]"#;

    #[test]
    fn floating_and_recent_windows_come_first_hidden_ones_never() {
        let clients = r#"[
          {"mapped":true,"hidden":false,"at":[0,0],"size":[100,100],"workspace":{"name":"1"},"floating":false,"focusHistoryID":0},
          {"mapped":true,"hidden":false,"at":[10,10],"size":[50,50],"workspace":{"name":"1"},"floating":true,"focusHistoryID":3},
          {"mapped":true,"hidden":false,"at":[20,20],"size":[50,50],"workspace":{"name":"2"},"floating":false,"focusHistoryID":1},
          {"mapped":true,"hidden":true,"at":[30,30],"size":[50,50],"workspace":{"name":"1"},"floating":false,"focusHistoryID":2}
        ]"#;
        let got = parse(clients.as_bytes(), MONITORS.as_bytes()).unwrap();
        assert_eq!(
            got,
            vec![
                Rect::new(10.0, 10.0, 50.0, 50.0),
                Rect::new(0.0, 0.0, 100.0, 100.0)
            ]
        );
    }

    #[test]
    fn an_open_special_workspace_is_on_top() {
        let monitors =
            r#"[{"activeWorkspace":{"name":"1"},"specialWorkspace":{"name":"special:term"}}]"#;
        let clients = r#"[
          {"at":[0,0],"size":[100,100],"workspace":{"name":"1"},"floating":true,"focusHistoryID":0},
          {"at":[5,5],"size":[10,10],"workspace":{"name":"special:term"},"focusHistoryID":4}
        ]"#;
        let got = parse(clients.as_bytes(), monitors.as_bytes()).unwrap();
        assert_eq!(got[0], Rect::new(5.0, 5.0, 10.0, 10.0));
    }

    #[test]
    fn garbage_yields_nothing() {
        assert!(parse(b"nope", MONITORS.as_bytes()).is_none());
    }
}
