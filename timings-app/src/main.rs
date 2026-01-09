use chrono::Duration;
use clap::Parser;
use futures::StreamExt;
use ksni::TrayMethods;
use log::trace;
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use std::path::PathBuf;
use std::str::FromStr;
use std::thread;
use timings::TimingsMutations;
use timings::TimingsRecording;
use tokio::sync::mpsc::UnboundedSender;
use virtual_desktops::KDEVirtualDesktopController;
use virtual_desktops::VirtualDesktopController;
use virtual_desktops::VirtualDesktopMessage;

const DEFAULT_DATABASE: &str = "~/.config/timings/timings.db";

#[derive(Parser)]
#[command(name = "timings-app")]
#[command(about = "Virtual desktop timings tracker", long_about = None)]
struct Cli {
    /// Path to the SQLite database file (e.g., timings.db or sqlite::memory:
    /// for in-memory)
    #[cfg(debug_assertions)]
    #[arg(short, long, default_value = "sqlite::memory:")]
    database: String,

    #[cfg(not(debug_assertions))]
    #[arg(short, long, default_value = DEFAULT_DATABASE)]
    database: String,

    /// Minimum timing duration in seconds (timings shorter than this are
    /// ignored)
    #[arg(short, long, default_value_t = 3)]
    minimum_timing: i64,
}

struct TrayState {
    current_desktop_name: String,
    notifier: UnboundedSender<AppMessage>,
}

enum AppMessage {
    Exit,
    WriteTimings,
    KeepAlive,
    ShowDailyTotals,
    VirtualDesktop(VirtualDesktopMessage),
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
        println!("Open menu?");
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

struct VirtualDesktopTimingsRecorder {
    client: Option<String>,
    project: Option<String>,
    timings_recorder: timings::TimingsRecorder,
    pool: SqlitePool,
}

impl VirtualDesktopTimingsRecorder {
    pub async fn new(
        database: &str,
        minimum_timing: Duration,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let options = SqliteConnectOptions::from_str(database)?.create_if_missing(true);

        let pool = SqlitePool::connect_with(options).await?;
        let mut conn = pool.acquire().await?;
        conn.create_timings_database().await?;
        drop(conn);

        let timings_recorder = timings::TimingsRecorder::new(minimum_timing);

        Ok(Self {
            client: None,
            project: None,
            timings_recorder,
            pool,
        })
    }

    /// Starts timing from a desktop name.
    /// The desktop name is expected to be in the format "client: project".
    /// If no colon is present, the entire name is used as the client.
    /// Only starts timing if both client and project can be parsed.
    pub fn start_timing_from_desktop_name(&mut self, desktop_name: &str) -> bool {
        let (client, project) = Self::parse_desktop_name(desktop_name);
        let old_client = self.client.clone();
        let old_project = self.project.clone();
        self.client = client.clone();
        self.project = project.clone();

        if let (Some(client), Some(project)) = (client, project) {
            log::info!(
                "Starting timing: client='{}', project='{}' (previous: client={:?}, project={:?})",
                client,
                project,
                old_client,
                old_project
            );
            self.timings_recorder
                .start_timing(client, project, chrono::Utc::now());
            true
        } else {
            log::warn!(
                "Stopping timing: desktop name '{}' has no valid project",
                desktop_name
            );
            self.timings_recorder.stop_timing(chrono::Utc::now());
            false
        }
    }

    /// Stops the current timing.
    pub fn stop_timing(&mut self) {
        log::info!("Stopping timing");
        self.timings_recorder.stop_timing(chrono::Utc::now());
    }

    pub fn resume_timing(&mut self) {
        if let Some(client) = &self.client
            && let Some(project) = &self.project
        {
            log::info!(
                "Resuming timing: client='{}', project='{}'",
                client,
                project
            );

            self.timings_recorder
                .start_timing(client.clone(), project.clone(), chrono::Utc::now());
        }
    }

    /// Keeps the current timing alive.
    /// Must be called at least once a minute to prevent gaps in timing.
    pub fn keep_alive(&mut self) {
        self.timings_recorder.keep_alive_timing(chrono::Utc::now());
    }

    /// Writes accumulated timings to the database.
    pub async fn write_timings(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        log::info!("Writing timings to database");
        let mut conn = self.pool.acquire().await?;
        let now = chrono::Utc::now();
        self.timings_recorder.write_timings(&mut *conn, now).await?;
        log::info!("Successfully wrote timings to database");
        Ok(())
    }

    /// Shows daily totals from the past 6 months.
    pub async fn show_daily_totals(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        use timings::TimingsQueries;

        let mut conn = self.pool.acquire().await?;
        let end_date = chrono::Utc::now();
        let start_date = end_date - chrono::Duration::days(180);

        let mut totals = conn
            .get_timings_daily_totals(start_date, end_date, None, None)
            .await?;
        totals.reverse();

        if totals.is_empty() {
            println!("No timings found for the past 6 months.");
            return Ok(());
        }

        // Print table header
        println!(
            "\n{:<12} {:<20} {:<20} {:>10}",
            "Date", "Client", "Project", "Hours"
        );
        println!("{}", "-".repeat(64));

        // Print each row
        for total in totals {
            println!(
                "{:<12} {:<20} {:<20} {:>10.2}",
                total.day, total.client, total.project, total.hours
            );
        }
        println!();

        Ok(())
    }

    /// Parses a desktop name into client and project.
    /// Format: "client: project" or just "client"
    fn parse_desktop_name(desktop_name: &str) -> (Option<String>, Option<String>) {
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
}

/// Expands ~ to the home directory and ensures parent directories exist (only
/// for DEFAULT_DATABASE)
async fn expand_path(path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let expanded = if path.starts_with("~") {
        if let Some(home) = std::env::var_os("HOME") {
            PathBuf::from(home).join(path.strip_prefix("~/").unwrap_or(&path[1..]))
        } else {
            PathBuf::from(path)
        }
    } else {
        PathBuf::from(path)
    };

    // Create parent directories only if they don't exist and path matches
    // DEFAULT_DATABASE
    if path == DEFAULT_DATABASE {
        if let Some(parent) = expanded.parent() {
            trace!(
                "Creating parent directories for database path: {:?}",
                parent
            );
            tokio::fs::create_dir_all(parent).await?;
        }
    }

    Ok(expanded.to_string_lossy().to_string())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("timings_app=info,timings=trace"),
    )
    .init();
    let cli = Cli::parse();

    let database_path = expand_path(&cli.database).await?;

    let mut vd_timings_recorder =
        VirtualDesktopTimingsRecorder::new(&database_path, Duration::seconds(cli.minimum_timing))
            .await?;

    let desktop_controller = KDEVirtualDesktopController::new().await?;
    let current_desktop = desktop_controller.get_current_desktop().await?;
    let current_desktop_name = desktop_controller
        .get_desktop_name(&current_desktop)
        .await
        .unwrap_or_else(|_| "Unknown".to_string());

    // Initialize timing for the current desktop
    vd_timings_recorder.start_timing_from_desktop_name(&current_desktop_name);

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
    spawn_write_timings_thread(appmsg_sender.clone());
    spawn_keepalive_thread(appmsg_sender.clone());
    spawn_virtual_desktop_listener(desktop_controller.clone(), appmsg_sender.clone());

    loop {
        match appmsgs.recv().await {
            Some(AppMessage::Exit) => {
                break Ok(());
            }
            Some(AppMessage::WriteTimings) => {
                if let Err(e) = vd_timings_recorder.write_timings().await {
                    log::error!("Failed to write timings: {}", e);
                }
            }
            Some(AppMessage::KeepAlive) => {
                log::trace!("Keep alive timing");
                vd_timings_recorder.keep_alive();
            }
            Some(AppMessage::ShowDailyTotals) => {
                if let Err(e) = vd_timings_recorder.show_daily_totals().await {
                    log::error!("Failed to show daily totals: {}", e);
                }
            }
            Some(AppMessage::VirtualDesktop(vd_msg)) => match vd_msg {
                VirtualDesktopMessage::DesktopNameChanged(_id, name) => {
                    vd_timings_recorder.start_timing_from_desktop_name(&name);
                    tray_state
                        .update(|s| {
                            s.current_desktop_name = name;
                        })
                        .await;
                }
                VirtualDesktopMessage::DesktopChange(id) => {
                    let name = desktop_controller
                        .get_desktop_name(&id)
                        .await
                        .unwrap_or_else(|_| "Unknown".to_string());
                    vd_timings_recorder.start_timing_from_desktop_name(&name);
                    tray_state
                        .update(|s| {
                            s.current_desktop_name = name;
                        })
                        .await;
                }
                VirtualDesktopMessage::ScreenSaveInactive => {
                    log::trace!("Screen saver in-active");
                    vd_timings_recorder.resume_timing();
                }
                VirtualDesktopMessage::ScreenSaverActive => {
                    log::trace!("Screen saver active");
                    vd_timings_recorder.stop_timing();
                }
            },
            None => {
                break Ok(());
            }
        }
    }
}

/// Spawns a task that listens to virtual desktop messages and forwards them to
/// the app message channel
fn spawn_virtual_desktop_listener(
    desktop_controller: KDEVirtualDesktopController,
    app_message_sender: tokio::sync::mpsc::UnboundedSender<AppMessage>,
) {
    tokio::spawn(async move {
        let mut vd_controller_listener = desktop_controller;
        if let Ok(mut vd_stream) = vd_controller_listener.listen().await {
            while let Some(vd_msg) = vd_stream.next().await {
                if app_message_sender
                    .send(AppMessage::VirtualDesktop(vd_msg))
                    .is_err()
                {
                    // Main thread has exited, stop the loop
                    break;
                }
            }
        }
    });
}

/// Spawns a thread to read lines from stdin
fn spawn_stdin_reader(app_message_sender: tokio::sync::mpsc::UnboundedSender<AppMessage>) {
    fn print_info() {
        println!("Commands:");
        println!("Q: Exit");
        println!("1: Write timings to database");
        println!("2: Show daily totals from past 6 months");
        println!("Type command and press Enter: ");
    }
    // let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    thread::spawn(move || {
        print_info();
        for line in std::io::stdin().lines() {
            match line.unwrap().to_lowercase().as_str() {
                "q" => {
                    let _ = app_message_sender.send(AppMessage::Exit);
                    break;
                }
                "1" => {
                    let _ = app_message_sender.send(AppMessage::WriteTimings);
                }
                "2" => {
                    let _ = app_message_sender.send(AppMessage::ShowDailyTotals);
                }
                _ => {
                    print_info();
                }
            }
        }
    });
}

/// Spawns a thread that sends WriteTimings message every 3 minutes
fn spawn_write_timings_thread(app_message_sender: tokio::sync::mpsc::UnboundedSender<AppMessage>) {
    thread::spawn(move || {
        loop {
            thread::sleep(std::time::Duration::from_secs(3 * 60));
            if app_message_sender.send(AppMessage::WriteTimings).is_err() {
                // Main thread has exited, stop the loop
                break;
            }
        }
    });
}

/// Spawns a thread that sends KeepAlive message every 30 seconds
fn spawn_keepalive_thread(app_message_sender: tokio::sync::mpsc::UnboundedSender<AppMessage>) {
    thread::spawn(move || {
        loop {
            thread::sleep(std::time::Duration::from_secs(30));
            if app_message_sender.send(AppMessage::KeepAlive).is_err() {
                // Main thread has exited, stop the loop
                break;
            }
        }
    });
}
