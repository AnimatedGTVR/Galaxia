//! IPC server: tells clients (the panel) what's going on, and takes their commands.
//! Protocol and types live in the `galaxia-ipc` crate.
use std::{
    io::{Read, Write},
    os::{
        fd::{AsFd, BorrowedFd},
        unix::net::{UnixListener, UnixStream},
    },
    path::PathBuf,
};

use galaxia_ipc::{Command, State as IpcState, Workspace as IpcWorkspace};
use smithay::{
    reexports::calloop::{generic::Generic, EventLoop, Interest, Mode, PostAction},
    wayland::{compositor::with_states, shell::xdg::XdgToplevelSurfaceData},
};

use crate::{state::spawn, CalloopData, Galaxia};

/// One connected client: its stream plus any half-received line.
struct Conn {
    stream: UnixStream,
    buf: Vec<u8>,
}

impl AsFd for Conn {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.stream.as_fd()
    }
}

#[derive(Default)]
pub struct Ipc {
    pub path: Option<PathBuf>,
    /// Write halves of connected clients.
    clients: Vec<UnixStream>,
    last: Option<IpcState>,
}

impl Drop for Ipc {
    fn drop(&mut self) {
        if let Some(p) = &self.path {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Start listening, and point programs we launch at the socket.
pub fn init(event_loop: &mut EventLoop<CalloopData>, state: &mut Galaxia) {
    let path = galaxia_ipc::socket_path_for(&state.socket_name.to_string_lossy());
    let _ = std::fs::remove_file(&path); // stale socket from a crash
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!("IPC disabled, could not bind {}: {e}", path.display());
            return;
        }
    };
    listener.set_nonblocking(true).ok();
    std::env::set_var(galaxia_ipc::SOCKET_ENV, &path);
    tracing::info!("IPC listening on {}", path.display());
    state.ipc.path = Some(path);

    let handle = event_loop.handle();
    handle
        .insert_source(Generic::new(listener, Interest::READ, Mode::Level), move |_, listener, data| {
            while let Ok((stream, _)) = listener.accept() {
                stream.set_nonblocking(true).ok();
                let Ok(writer) = stream.try_clone() else { continue };
                data.state.ipc_add_client(writer);
                let conn = Conn { stream, buf: Vec::new() };
                let _ = data.state.loop_handle.insert_source(Generic::new(conn, Interest::READ, Mode::Level), |_, conn, data| {
                    // Safety: we never replace the stream inside the source.
                    let conn = unsafe { conn.get_mut() };
                    let mut chunk = [0u8; 1024];
                    loop {
                        match conn.stream.read(&mut chunk) {
                            Ok(0) => return Ok(PostAction::Remove), // client hung up
                            Ok(n) => conn.buf.extend_from_slice(&chunk[..n]),
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                            Err(_) => return Ok(PostAction::Remove),
                        }
                    }
                    while let Some(nl) = conn.buf.iter().position(|b| *b == b'\n') {
                        let line: Vec<u8> = conn.buf.drain(..=nl).collect();
                        match galaxia_ipc::decode::<Command>(&String::from_utf8_lossy(&line)) {
                            Some(cmd) => data.state.ipc_command(cmd),
                            None => tracing::debug!("ignoring bad IPC line"),
                        }
                    }
                    Ok(PostAction::Continue)
                });
            }
            Ok(PostAction::Continue)
        })
        .expect("failed to add IPC listener");
}

impl Galaxia {
    fn ipc_add_client(&mut self, mut writer: UnixStream) {
        // New clients get the current state straight away.
        let snapshot = self.ipc_snapshot();
        if writer.write_all(galaxia_ipc::encode(&snapshot).as_bytes()).is_ok() {
            self.ipc.clients.push(writer);
        }
    }

    fn ipc_command(&mut self, cmd: Command) {
        match cmd {
            Command::SwitchWorkspace(n) => self.switch_workspace(n),
            Command::Spawn(c) => spawn(&c),
        }
    }

    fn ipc_snapshot(&self) -> IpcState {
        let focused = self.focused_id().and_then(|id| self.window(id)).and_then(|w| w.toplevel().cloned());
        let (focused_title, focused_app) = focused
            .map(|t| {
                with_states(t.wl_surface(), |s| {
                    let d = s.data_map.get::<XdgToplevelSurfaceData>().map(|d| d.lock().unwrap());
                    d.map(|d| (d.title.clone(), d.app_id.clone())).unwrap_or_default()
                })
            })
            .unwrap_or_default();
        IpcState {
            active: self.active,
            workspaces: self
                .workspaces
                .iter()
                .map(|ws| IpcWorkspace { windows: ws.columns.iter().map(|c| c.windows.len()).sum::<usize>() + ws.floating.len() })
                .collect(),
            focused_title,
            focused_app,
        }
    }

    /// Send the state to every client if it changed. Cheap; called once per frame.
    pub fn ipc_broadcast(&mut self) {
        if self.ipc.clients.is_empty() {
            return;
        }
        let snapshot = self.ipc_snapshot();
        if self.ipc.last.as_ref() == Some(&snapshot) {
            return;
        }
        let line = galaxia_ipc::encode(&snapshot);
        // Drop clients that went away (or are too slow to keep up).
        self.ipc.clients.retain_mut(|c| c.write_all(line.as_bytes()).is_ok());
        self.ipc.last = Some(snapshot);
    }
}
