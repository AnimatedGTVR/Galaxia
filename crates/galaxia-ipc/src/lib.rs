//! IPC between the Galaxia compositor and clients like the panel.
//!
//! A Unix socket at `$GALAXIA_SOCKET` carries newline-delimited JSON.
//! - The compositor sends a [`State`] on connect and whenever it changes.
//! - Clients send [`Command`]s, one per line.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const SOCKET_ENV: &str = "GALAXIA_SOCKET";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Workspace {
    /// Number of windows (tiled + floating) on this workspace.
    pub windows: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct State {
    /// Index of the workspace in view (0-based).
    pub active: usize,
    pub workspaces: Vec<Workspace>,
    /// Title of the window with keyboard focus.
    pub focused_title: Option<String>,
    /// App id of the window with keyboard focus.
    pub focused_app: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    SwitchWorkspace(usize),
    Spawn(String),
}

/// Where the compositor listens: `$XDG_RUNTIME_DIR/galaxia-<wayland display>.sock`.
pub fn socket_path_for(wayland_display: &str) -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    dir.join(format!("galaxia-{wayland_display}.sock"))
}

/// The socket a client should use: `$GALAXIA_SOCKET`, set by the compositor for programs it starts.
pub fn socket_path() -> Option<PathBuf> {
    std::env::var_os(SOCKET_ENV).map(PathBuf::from)
}

pub fn encode<T: Serialize>(msg: &T) -> String {
    let mut s = serde_json::to_string(msg).expect("IPC types always serialize");
    s.push('\n');
    s
}

pub fn decode<'a, T: Deserialize<'a>>(line: &'a str) -> Option<T> {
    serde_json::from_str(line.trim()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let s = State { active: 2, workspaces: vec![Workspace { windows: 1 }; 3], focused_title: Some("hi".into()), focused_app: None };
        assert_eq!(decode::<State>(&encode(&s)), Some(s));
        let c = Command::SwitchWorkspace(4);
        assert_eq!(encode(&c), "{\"switch_workspace\":4}\n");
        assert_eq!(decode::<Command>("{\"spawn\":\"foot\"}"), Some(Command::Spawn("foot".into())));
        assert_eq!(decode::<Command>("garbage"), None);
    }
}
