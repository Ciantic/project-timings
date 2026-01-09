use futures::StreamExt;
use ksni::TrayMethods;
use sqlx::SqlitePool;
use std::thread;
use timings::TimingsMutations;
use tokio::sync::mpsc::UnboundedSender;
use virtual_desktops::KDEVirtualDesktopController;
use virtual_desktops::VirtualDesktopController;
use virtual_desktops::VirtualDesktopMessage;

struct TrayState {
    current_desktop_name: String,
    notifier: UnboundedSender<AppMessage>,
}

enum AppMessage {
    Exit,
}

impl ksni::Tray for TrayState {
    fn id(&self) -> String {
        env!("CARGO_PKG_NAME").into()
    }

    fn icon_name(&self) -> String {
        "help-about".into()
    }

    fn title(&self) -> String {
        "Timings".into()
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        vec![
            StandardItem {
                label: "Desktop: ".to_string() + &self.current_desktop_name,
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Exit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|this: &mut Self| {
                    this.notifier
                        .send(AppMessage::Exit)
                        .expect("Main thread is not listening");
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool = SqlitePool::connect("sqlite::memory:").await?;
    let mut conn = pool.acquire().await?;
    conn.create_timings_database().await?;

    let mut client: Option<String> = None;
    let mut project: Option<String> = None;

    let desktop_controller = KDEVirtualDesktopController::new().await?;
    let current_desktop = desktop_controller.get_current_desktop().await?;
    let current_desktop_name = desktop_controller
        .get_desktop_name(&current_desktop)
        .await
        .unwrap_or_else(|_| "Unknown".to_string());
    let (appmsg_sender, mut appmsgs) = tokio::sync::mpsc::unbounded_channel::<AppMessage>();

    let tray_state = TrayState {
        current_desktop_name: current_desktop_name.clone(),
        notifier: appmsg_sender.clone(),
        // app_state: Arc::clone(&state),
    };
    let tray_state = tray_state
        // .disable_dbus_name(ashpd::is_sandboxed().await) // For flatpak apps
        .spawn()
        .await?;

    spawn_stdin_reader(appmsg_sender.clone());
    let mut vd_controller_listener = desktop_controller.clone();
    let mut vd_stream = vd_controller_listener.listen().await?;
    loop {
        tokio::select! {
            Some(msg2) = vd_stream.next() => {
                match msg2 {
                    VirtualDesktopMessage::DesktopNameChanged(id, name) => {
                        println!("Desktop name changed: {} -> {}", id, name);
                        let (new_client, new_project) = parse_desktop_name(&name);
                        if client != new_client || project != new_project {
                            client = new_client;
                            project = new_project;
                        }
                        tray_state.update(|s| {
                            s.current_desktop_name = name;
                        }).await;
                    }
                    VirtualDesktopMessage::DesktopChange(id) => {
                        println!("Desktop changed: {}", id);
                        let name = desktop_controller
                            .get_desktop_name(&id)
                            .await
                            .unwrap_or_else(|_| "Unknown".to_string());
                        let (new_client, new_project) = parse_desktop_name(&name);
                        if client != new_client || project != new_project {
                            client = new_client;
                            project = new_project;
                        }
                        tray_state.update(|s| {
                            s.current_desktop_name = name;
                        }).await;
                    }
                    VirtualDesktopMessage::ScreenSaveInactive => {
                        println!("Screen saver inactive");
                    }
                    VirtualDesktopMessage::ScreenSaverActive => {
                        println!("Screen saver active");
                    }
                }
            }
            Some(msg) = appmsgs.recv() => {
                match msg {
                    AppMessage::Exit => {
                        break Ok(());
                    }
                }
            }
        }
    }
}

/// Spawns a thread to read lines from stdin
fn spawn_stdin_reader(app_message_sender: tokio::sync::mpsc::UnboundedSender<AppMessage>) {
    fn print_info() {
        println!("Commands:");
        println!("0: Exit");
        println!("Type command: ");
    }
    // let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    thread::spawn(move || {
        print_info();
        for line in std::io::stdin().lines() {
            if line.unwrap() == "0" {
                let _ = app_message_sender.send(AppMessage::Exit);
                break;
            }
            print_info();
        }
    });
}

fn parse_desktop_name(desktop_name: &str) -> (Option<String>, Option<String>) {
    // Split desktop name by ":" into client and project
    let parts: Vec<&str> = desktop_name.splitn(2, ':').collect();
    if parts.len() == 2 {
        (
            Some(parts[0].trim().to_string()),
            Some(parts[1].trim().to_string()),
        )
    } else {
        (Some(desktop_name.trim().to_string()), None)
    }
}
