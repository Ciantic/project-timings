use chrono::Duration;
use clap::Parser;
use futures::StreamExt;
use idle_monitor::run_idle_monitor;
use log::trace;
use single_instance::only_single_instance;
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use std::path::PathBuf;
use std::str::FromStr;
use std::thread;
use timings::TimingsMutations;
use timings::TimingsRecording;
use tokio::sync::mpsc::UnboundedSender;
use trayicon::Icon;
use trayicon::MenuBuilder;
use trayicon::TrayIconBuilder;
use virtual_desktops::KDEVirtualDesktopController;
use virtual_desktops::VirtualDesktopController;
use virtual_desktops::VirtualDesktopMessage;

const DEFAULT_DATABASE: &str = "~/.config/timings/timings.db";
const ICON_GREEN: &[u8] = include_bytes!("../resources/green.ico");
const ICON_RED: &[u8] = include_bytes!("../resources/red.ico");

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
    minimum_timing: u64,

    /// Idle timeout in seconds (how long before user is considered idle)
    ///
    /// Set to 0 to disable idle monitoring.
    #[arg(short = 't', long, default_value_t = 180)]
    idle_timeout: u64,
}

#[derive(PartialEq, Clone)]
enum AppMessage {
    Exit,
    WriteTimings,
    KeepAlive,
    ShowDailyTotals,
    VirtualDesktop(VirtualDesktopMessage),
    VirtualDesktopThreadExited,
    StartedTiming(String, String),
    StoppedTiming,
    UserIdled,
    UserResumed,
    AnotherInstanceTriedToStart,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("timings_app=info,timings=trace"),
    )
    .init();
    let cli = Cli::parse();
    let database_path = handle_database_path(&cli.database).await?;
    let (appmsg_sender, mut appmsgs) = tokio::sync::mpsc::unbounded_channel::<AppMessage>();

    // Ensure only a single instance is running for this database path
    let sender_for_single_instance = appmsg_sender.clone();
    only_single_instance(&database_path, move || {
        let _ = sender_for_single_instance.send(AppMessage::AnotherInstanceTriedToStart);
    })?;

    // Start the virtual desktop timings recorder
    let mut vd_timings_recorder = VirtualDesktopTimingsRecorder::new(
        &database_path,
        Duration::seconds(cli.minimum_timing as i64),
        appmsg_sender.clone(),
    )
    .await?;

    let desktop_controller = KDEVirtualDesktopController::new().await?;
    let current_desktop = desktop_controller.get_current_desktop().await?;
    let current_desktop_name = desktop_controller
        .get_desktop_name(&current_desktop)
        .await
        .unwrap_or_else(|_| "Unknown".to_string());

    // Initialize timing for the current desktop
    vd_timings_recorder.start_timing_from_desktop_name(&current_desktop_name);

    let tray_icon_sender = appmsg_sender.clone();
    let green_icon = Icon::from_buffer(ICON_GREEN, None, None)?;
    let red_icon = Icon::from_buffer(ICON_RED, None, None)?;
    let mut tray_icon = TrayIconBuilder::new()
        .sender(move |m: &AppMessage| {
            let _ = tray_icon_sender.send(m.clone());
        })
        .icon(green_icon.clone())
        .tooltip(format!("Timings: {}", current_desktop_name).as_str())
        .menu(
            MenuBuilder::new()
                .item("Show daily totals", AppMessage::ShowDailyTotals)
                .item("Exit", AppMessage::Exit),
        )
        .build()?;

    spawn_idle_monitor_thread(appmsg_sender.clone(), cli.idle_timeout);
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
                    let _ = tray_icon.set_tooltip(format!("Timings: {}", name).as_str());
                }
                VirtualDesktopMessage::DesktopChange(id) => {
                    let name = desktop_controller
                        .get_desktop_name(&id)
                        .await
                        .unwrap_or_else(|_| "Unknown".to_string());
                    vd_timings_recorder.start_timing_from_desktop_name(&name);
                    let _ = tray_icon.set_tooltip(format!("Timings: {}", name).as_str());
                }
            },
            Some(AppMessage::UserIdled) => {
                log::trace!("User activity changed to idling");
                vd_timings_recorder.stop_timing();
            }
            Some(AppMessage::UserResumed) => {
                log::trace!("User activity changed to resumed");
                vd_timings_recorder.resume_timing();
            }
            Some(AppMessage::VirtualDesktopThreadExited) => {
                log::warn!(
                    "Virtual desktop listener thread has exited, this happens if the D-Bus \
                     connection is lost for instance when user closes the desktop but not the \
                     application."
                );
                break Err("Virtual desktop listener thread has exited".into());
            }
            Some(AppMessage::StartedTiming(client, project)) => {
                log::trace!("Started timing: client='{}', project='{}'", client, project);
                let _ = tray_icon.set_icon(&green_icon);
            }
            Some(AppMessage::StoppedTiming) => {
                log::trace!("Stopped timing");
                let _ = tray_icon.set_icon(&red_icon);
            }
            Some(AppMessage::AnotherInstanceTriedToStart) => {
                log::info!("Another instance tried to start");
            }
            None => {
                break Ok(());
            }
        }
    }
}

struct VirtualDesktopTimingsRecorder {
    client: Option<String>,
    project: Option<String>,
    timings_recorder: timings::TimingsRecorder,
    pool: SqlitePool,
    sender: UnboundedSender<AppMessage>,
}

impl VirtualDesktopTimingsRecorder {
    pub async fn new(
        database: &str,
        minimum_timing: Duration,
        sender: UnboundedSender<AppMessage>,
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
            sender,
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
                .start_timing(client.clone(), project.clone(), chrono::Utc::now());
            let _ = self.sender.send(AppMessage::StartedTiming(client, project));
            true
        } else {
            log::warn!(
                "Stopping timing: desktop name '{}' has no valid project",
                desktop_name
            );
            self.timings_recorder.stop_timing(chrono::Utc::now());
            let _ = self.sender.send(AppMessage::StoppedTiming);
            false
        }
    }

    /// Stops the current timing.
    pub fn stop_timing(&mut self) {
        log::info!("Stopping timing");
        self.timings_recorder.stop_timing(chrono::Utc::now());
        let _ = self.sender.send(AppMessage::StoppedTiming);
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
            let _ = self
                .sender
                .send(AppMessage::StartedTiming(client.clone(), project.clone()));
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
///
/// Canonicalizes the path to absolute path.
async fn handle_database_path(path: &str) -> Result<String, Box<dyn std::error::Error>> {
    if path.starts_with(":") || path == "sqlite::memory:" {
        // Special SQLite in-memory or URI path, return as is
        return Ok(path.to_string());
    }

    // Expand ~ to home directory
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

    // Expand path to absolute (std)
    let expanded = expanded.canonicalize()?;

    Ok(expanded.to_string_lossy().to_string())
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

        let _ = app_message_sender.send(AppMessage::VirtualDesktopThreadExited);
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

/// Spawns a thread that runs the idle monitor
fn spawn_idle_monitor_thread(
    app_message_sender: tokio::sync::mpsc::UnboundedSender<AppMessage>,
    idle_timeout: u64,
) {
    if idle_timeout == 0 {
        log::info!("Idle timeout is 0, not starting idle monitor");
        return;
    }

    thread::spawn(move || {
        let monitor_thread = run_idle_monitor(
            move |i| match i {
                idle_monitor::IdleNotification::Idle => {
                    let _ = app_message_sender.send(AppMessage::UserIdled);
                }
                idle_monitor::IdleNotification::Resumed => {
                    let _ = app_message_sender.send(AppMessage::UserResumed);
                }
            },
            std::time::Duration::from_secs(idle_timeout),
        );

        match monitor_thread.join() {
            Ok(Ok(())) => {
                log::info!("Idle monitor completed successfully");
            }
            Ok(Err(e)) => {
                log::error!("Idle monitor error: {}", e);
            }
            Err(_) => {
                log::error!("Idle monitor thread panic");
            }
        }
    });
}
