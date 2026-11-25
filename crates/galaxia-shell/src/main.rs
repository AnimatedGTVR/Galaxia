//! Galaxia shell: the panel.
//!
//! A floating bar along the top of the screen. The compositor draws frosted
//! glass behind it; the bar itself is transparent and only draws its content.
//!   left:   workspace pills (click to switch) + focused window title
//!   center: clock
//!   right:  launcher button
use std::time::Duration;

use galaxia_ipc::{Command, State as IpcState};
use iced::{
    border::Radius,
    widget::{container, mouse_area, row, space, text, Space},
    Alignment, Background, Border, Color, Element, Length, Subscription, Task,
};
use iced_layershell::{
    application,
    reexport::{Anchor, KeyboardInteractivity},
    settings::{LayerShellSettings, Settings, StartMode},
    to_layer_message,
};
use tokio::io::AsyncBufReadExt;

const BAR_HEIGHT: u32 = 40;

struct Panel {
    theme: galaxia_theme::Theme,
    state: IpcState,
    connected: bool,
    now: chrono::DateTime<chrono::Local>,
}

#[to_layer_message]
#[derive(Debug, Clone)]
enum Message {
    Tick,
    Ipc(Option<IpcState>),
    SwitchWorkspace(usize),
    Launch,
}

fn rgba(c: galaxia_theme::Color) -> Color {
    Color::from_rgba(c.0[0], c.0[1], c.0[2], c.0[3])
}

/// Send one command to the compositor on a short-lived connection.
fn send(cmd: Command) {
    use std::io::Write;
    let Some(path) = galaxia_ipc::socket_path() else { return };
    match std::os::unix::net::UnixStream::connect(&path) {
        Ok(mut s) => {
            let _ = s.write_all(galaxia_ipc::encode(&cmd).as_bytes());
        }
        Err(e) => eprintln!("galaxia-shell: cannot reach compositor: {e}"),
    }
}

/// Stream of compositor state. Reconnects if the compositor restarts.
fn ipc_stream() -> impl iced::futures::Stream<Item = Message> {
    iced::stream::channel(16, async |mut out: iced::futures::channel::mpsc::Sender<Message>| {
        use iced::futures::SinkExt;
        loop {
            let Some(path) = galaxia_ipc::socket_path() else {
                eprintln!("galaxia-shell: {} not set; is this running inside Galaxia?", galaxia_ipc::SOCKET_ENV);
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            };
            if let Ok(stream) = tokio::net::UnixStream::connect(&path).await {
                let mut lines = tokio::io::BufReader::new(stream).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Some(state) = galaxia_ipc::decode::<IpcState>(&line) {
                        let _ = out.send(Message::Ipc(Some(state))).await;
                    }
                }
            }
            let _ = out.send(Message::Ipc(None)).await;
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
}

fn namespace() -> String {
    "galaxia-panel".into()
}

fn update(panel: &mut Panel, message: Message) -> Task<Message> {
    match message {
        Message::Tick => panel.now = chrono::Local::now(),
        Message::Ipc(Some(s)) => {
            panel.state = s;
            panel.connected = true;
        }
        Message::Ipc(None) => panel.connected = false,
        Message::SwitchWorkspace(n) => send(Command::SwitchWorkspace(n)),
        Message::Launch => send(Command::Spawn(std::env::var("GALAXIA_TERM").unwrap_or_else(|_| "ghostty --gtk-single-instance=false".into()))),
        _ => {}
    }
    Task::none()
}

fn subscription(_: &Panel) -> Subscription<Message> {
    Subscription::batch([
        iced::time::every(Duration::from_secs(1)).map(|_| Message::Tick),
        Subscription::run(ipc_stream),
    ])
}

/// One workspace indicator: a wide accent pill when active, a dot when it has windows.
fn pill<'a>(panel: &Panel, index: usize, windows: usize) -> Element<'a, Message> {
    let active = index == panel.state.active;
    let accent = rgba(panel.theme.accent);
    let (w, color) = match (active, windows > 0) {
        (true, _) => (26.0, accent),
        (false, true) => (10.0, Color { a: 0.75, ..Color::WHITE }),
        (false, false) => (10.0, Color { a: 0.25, ..Color::WHITE }),
    };
    let dot = container(Space::new().width(w).height(10.0)).style(move |_| container::Style {
        background: Some(Background::Color(color)),
        border: Border { radius: Radius::from(5.0), ..Border::default() },
        ..container::Style::default()
    });
    mouse_area(container(dot).padding([0, 3])).on_press(Message::SwitchWorkspace(index)).into()
}

fn view(panel: &Panel) -> Element<'_, Message> {
    let fg = Color::WHITE;
    let dim = Color { a: 0.7, ..Color::WHITE };

    // Show workspaces up to the last one in use (plus the active one), at least one.
    let last = panel
        .state
        .workspaces
        .iter()
        .rposition(|w| w.windows > 0)
        .unwrap_or(0)
        .max(panel.state.active);
    let pills = row((0..=last).map(|i| {
        let windows = panel.state.workspaces.get(i).map(|w| w.windows).unwrap_or(0);
        pill(panel, i, windows)
    }))
    .align_y(Alignment::Center);

    let title: String = if panel.connected {
        let t = panel.state.focused_title.clone().or(panel.state.focused_app.clone()).unwrap_or_default();
        if t.chars().count() > 60 { t.chars().take(57).collect::<String>() + "…" } else { t }
    } else {
        "not connected".into()
    };

    let left = row![pills, text(title).size(13).color(dim)].spacing(14).align_y(Alignment::Center);
    let clock = row![
        text(panel.now.format("%a %b %-d").to_string()).size(13).color(dim),
        text(panel.now.format("%-I:%M %p").to_string()).size(14).color(fg),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let accent = rgba(panel.theme.accent);
    let launcher = mouse_area(
        container(text("✦").size(16).color(fg))
            .padding([2, 10])
            .style(move |_| container::Style {
                background: Some(Background::Color(Color { a: 0.35, ..accent })),
                border: Border { radius: Radius::from(10.0), ..Border::default() },
                ..container::Style::default()
            }),
    )
    .on_press(Message::Launch);

    row![
        container(left).width(Length::Fill).align_x(Alignment::Start),
        container(clock).width(Length::Shrink),
        container(row![space(), launcher]).width(Length::Fill).align_x(Alignment::End),
    ]
    .padding([0, 16])
    .align_y(Alignment::Center)
    .height(Length::Fill)
    .into()
}

fn style(_: &Panel, _theme: &iced::Theme) -> iced::theme::Style {
    // Transparent: the compositor's frosted glass shows through.
    iced::theme::Style { background_color: Color::TRANSPARENT, text_color: Color::WHITE }
}

fn main() -> Result<(), iced_layershell::Error> {
    let (theme, err) = galaxia_theme::Theme::load();
    if let Some(e) = err {
        eprintln!("galaxia-shell: theme problem, using defaults: {e}");
    }
    let gap = theme.gap.round() as i32;

    application(
        move || Panel { theme: theme.clone(), state: IpcState::default(), connected: false, now: chrono::Local::now() },
        namespace,
        update,
        view,
    )
    .style(style)
    .subscription(subscription)
    .settings(Settings {
        layer_settings: LayerShellSettings {
            size: Some((0, BAR_HEIGHT)),
            exclusive_zone: BAR_HEIGHT as i32,
            anchor: Anchor::Top | Anchor::Left | Anchor::Right,
            // Float off the screen edges, riced style: (top, right, bottom, left).
            margin: (gap, gap, 0, gap),
            // A panel never takes keyboard focus away from windows.
            keyboard_interactivity: KeyboardInteractivity::None,
            start_mode: StartMode::Active,
            ..Default::default()
        },
        ..Default::default()
    })
    .run()
}
