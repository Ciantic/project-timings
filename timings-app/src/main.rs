use chrono::Duration;
use chrono::Local;
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
use timings::TimingsMockdata;
use timings::TimingsMutations;
use timings::TimingsRecorder;
use timings::TimingsRecording;
use tokio::sync::mpsc::UnboundedSender;
use trayicon::Icon;
use trayicon::MenuBuilder;
use trayicon::TrayIconBuilder;
use virtual_desktops::DesktopId;
use virtual_desktops::KDEVirtualDesktopController;
use virtual_desktops::VirtualDesktopController;
use virtual_desktops::VirtualDesktopMessage;
use wayapp::Application;
use wayapp::DispatchToken;
mod gui_overlay;
mod gui_stats;
mod utils;
use crate::gui_overlay::GuiOverlay;
use crate::gui_overlay::GuiOverlayEvent;
use crate::utils::run_debounced_spawn;

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

#[derive(Debug, PartialEq, Clone)]
enum AppMessage {
    WaylandDispatch(DispatchToken),
    Exit,
    WriteTimings,
    KeepAlive,
    ShowStats,
    ShowDailyTotals,
    ShowDailySummaries,
    TrayIconClicked,
    VirtualDesktop(VirtualDesktopMessage),
    VirtualDesktopThreadExited,
    HideLayerOverlay,
    UserIdled,
    RunningChanged(bool),
    UserResumed,
    AnotherInstanceTriedToStart,
    RequestRender,
    GuiOverlayEvent(GuiOverlayEvent),
}

#[tokio::main(flavor = "current_thread")]
#[hotpath::main(percentiles = [100])]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(debug_assertions)]
    env_logger::Builder::from_env(
        env_logger::Env::default()
            .default_filter_or("timings_app=trace,timings=trace,wayapp=trace"),
    )
    .init();

    #[cfg(not(debug_assertions))]
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("timings_app=warn,timings=warn,wayapp=warn"),
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

    let desktop_controller = KDEVirtualDesktopController::new().await?;

    // Stats GUI
    // Start the timings app
    let mut timings_app = TimingsApp::new(
        cli.minimum_timing as i64,
        &database_path,
        appmsg_sender.clone(),
        &desktop_controller,
    )
    .await?;

    // Initialize timing for the current desktop
    timings_app.start_timing().await?;

    let appmsg_sender_ = appmsg_sender.clone();
    let mut app = Application::new(move |t| {
        let _ = appmsg_sender_.send(AppMessage::WaylandDispatch(t));
    });
    spawn_idle_monitor_thread(appmsg_sender.clone(), cli.idle_timeout);
    spawn_stdin_reader(appmsg_sender.clone());
    spawn_write_timings_thread(appmsg_sender.clone());
    spawn_keepalive_thread(appmsg_sender.clone());
    spawn_virtual_desktop_listener(desktop_controller.clone(), appmsg_sender.clone());
    app.run_dispatcher();
    loop {
        if let Some(event) = appmsgs.recv().await {
            match timings_app.handle_app_events(&mut app, &event).await {
                Ok(true) => break Ok(()),
                Err(e) => break Err(e),
                Ok(false) => {}
            }
        }
    }
}

struct TimingsApp {
    // Timing recording fields
    timings_recorder: timings::TimingsRecorder,
    pool: SqlitePool,
    sender: UnboundedSender<AppMessage>,
    desktop_controller: KDEVirtualDesktopController,

    // Current desktop, updated on desktop change
    current_desktop: DesktopId,

    // Gui state
    gui_overlay: Option<GuiOverlay>,

    // Tray icon
    tray_icon: trayicon::TrayIcon<AppMessage>,
    green_icon: Icon,
    red_icon: Icon,
}

impl TimingsApp {
    pub async fn new(
        minimum_timing: i64,
        database: &str,
        sender: UnboundedSender<AppMessage>,
        desktop_controller: &KDEVirtualDesktopController,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let options = SqliteConnectOptions::from_str(database)?.create_if_missing(true);

        let pool = SqlitePool::connect_with(options).await?;
        let mut conn = pool.acquire().await?;
        conn.create_timings_database().await?;

        let mut timings_recorder =
            TimingsRecorder::new(pool.clone(), Duration::seconds(minimum_timing));

        let sender_ = sender.clone();
        timings_recorder.set_running_changed_callback(move |running| {
            let _ = sender_.send(AppMessage::RunningChanged(running));
        });

        // Insert mockdata in debug mode with :memory:
        #[cfg(debug_assertions)]
        if database == "sqlite::memory:" {
            conn.insert_mockdata(chrono::Utc::now()).await?;
        }

        drop(conn);

        // Current desktop
        let current_desktop = desktop_controller.get_current_desktop().await?;

        // Build tray icon
        let green_icon = Icon::from_buffer(ICON_GREEN, None, None)?;
        let red_icon = Icon::from_buffer(ICON_RED, None, None)?;
        let tray_icon_sender = sender.clone();
        let tray_icon = TrayIconBuilder::new()
            .sender(move |m: &AppMessage| {
                let _ = tray_icon_sender.send(m.clone());
            })
            .on_click(AppMessage::TrayIconClicked)
            .icon(green_icon.clone())
            .tooltip(format!("Timings").as_str())
            .menu(
                MenuBuilder::new()
                    .item("Show stats", AppMessage::ShowStats)
                    .item("Exit", AppMessage::Exit),
            )
            .build()?;

        Ok(Self {
            timings_recorder,
            pool,
            sender,
            desktop_controller: desktop_controller.clone(),
            current_desktop,
            gui_overlay: None,
            tray_icon,
            green_icon,
            red_icon,
        })
    }

    /// Starts timing from a desktop name.
    /// The desktop name is expected to be in the format "client: project".
    /// If no colon is present, the entire name is used as the client.
    /// Only starts timing if both client and project can be parsed.
    fn start_timing_from_desktop_name(&mut self, desktop_name: &str) -> bool {
        let (client, project) = parse_desktop_name(desktop_name);

        if self
            .gui_overlay
            .as_ref()
            .map(|f| f.has_keyboard_focus())
            .unwrap_or(false)
        {
            log::info!(
                "Not starting timing from desktop name '{}' because GUI has focus",
                desktop_name
            );
            return false;
        }

        if let (Some(client), Some(project)) = (client, project) {
            trace!(
                "Starting timing: desktop name '{}' parsed to client '{}' and project '{}'",
                desktop_name, client, project
            );
            self.timings_recorder
                .start_timing(client.clone(), project.clone(), chrono::Utc::now());
            self.sender.send(AppMessage::RequestRender).ok();

            true
        } else {
            log::warn!(
                "Stopping timing: desktop name '{}' has no valid project",
                desktop_name
            );
            self.stop_timing();
            false
        }
    }

    pub async fn start_timing(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let current_desktop_name = self
            .desktop_controller
            .get_desktop_name(&self.current_desktop)
            .await
            .unwrap_or_else(|_| "Unknown".to_string());
        self.start_timing_from_desktop_name(&current_desktop_name);
        Ok(())
    }

    /// Stops the current timing.
    pub fn stop_timing(&mut self) {
        log::info!("Stopping timing");
        self.timings_recorder.stop_timing(chrono::Utc::now());
    }

    /// Keeps the current timing alive.
    /// Must be called at least once a minute to prevent gaps in timing.
    pub fn keep_alive(&mut self) {
        self.timings_recorder.keep_alive_timing(chrono::Utc::now());
    }

    /// Writes accumulated timings to the database.
    pub async fn write_timings(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        log::info!("Writing timings to database");
        let now = chrono::Utc::now();
        self.timings_recorder.write_timings(now).await?;
        log::info!("Successfully wrote timings to database");
        Ok(())
    }

    /// Shows daily totals from the past 6 months.
    pub async fn show_daily_totals(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        use timings::TimingsQueries;

        let mut conn = self.pool.acquire().await?;
        let end_date = chrono::Local::now().naive_local().date();
        let start_date = end_date - chrono::Duration::days(180);

        let mut totals = conn
            .get_timings_daily_totals(Local, start_date, end_date, None, None)
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

    pub async fn show_daily_summaries(&self) -> Result<(), Box<dyn std::error::Error>> {
        use timings::TimingsQueries;

        let mut conn = self.pool.acquire().await?;
        let end_date = chrono::Local::now().naive_local().date();
        let start_date = end_date - chrono::Duration::days(28);

        let mut summaries = conn
            .get_timings_daily_totals_and_summaries(Local, start_date, end_date, None, None)
            .await?;
        summaries.reverse();

        if summaries.is_empty() {
            println!("No timings found for the past 4 weeks.");
            return Ok(());
        }

        // Print table header
        println!(
            "\n{:<12} {:<20} {:<20} {:>10} {}",
            "Date", "Client", "Project", "Hours", "Summary"
        );
        println!("{}", "-".repeat(100));

        // Print each row
        for summary in summaries {
            println!(
                "{:<12} {:<20} {:<20} {:>10.2} {}",
                summary.day, summary.client, summary.project, summary.hours, summary.summary
            );
        }
        println!();

        Ok(())
    }

    // GUI methods
    pub fn show_gui(&mut self, app: &mut Application) {
        if self.gui_overlay.is_none() {
            log::trace!("Showing overlay GUI");
            let overlay = GuiOverlay::new(
                app,
                self,
                self.sender.clone(),
                self.desktop_controller.clone(),
            );
            self.gui_overlay = Some(overlay);
        }
        self.hide_gui_after_delay();
    }

    pub fn hide_gui(&mut self) {
        if let Some(ref overlay) = self.gui_overlay {
            if overlay.has_keyboard_focus() {
                log::trace!("Not hiding overlay, has keyboard focus");
                return;
            }
        }
        log::trace!("Hiding overlay GUI");

        self.gui_overlay.take();
    }

    pub fn hide_gui_after_delay(&mut self) {
        let tx = self.sender.clone();
        run_debounced_spawn(
            "hide_gui_after_delay",
            std::time::Duration::from_secs(3),
            async move {
                let _ = tx.send(AppMessage::HideLayerOverlay);
            },
        );
    }

    pub async fn handle_app_events(
        &mut self,
        app: &mut Application,
        event: &AppMessage,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        // Handle GUI overlay events first
        if let Some(mut overlay) = self.gui_overlay.take() {
            overlay.handle_app_events(self, app, event).await;
            self.gui_overlay = Some(overlay);
        }

        // Handle app events
        match event {
            AppMessage::WaylandDispatch(token) => {
                let events = app.dispatch_pending(*token);
                if let Some(mut overlay) = self.gui_overlay.take() {
                    overlay.handle_wayland_events(self, app, &events).await;
                    self.gui_overlay = Some(overlay);
                }
            }
            AppMessage::Exit => {
                return Ok(true);
            }
            AppMessage::WriteTimings => {
                if let Err(e) = self.write_timings().await {
                    log::error!("Failed to write timings: {}", e);
                }
            }
            AppMessage::KeepAlive => {
                log::trace!("Keep alive timing");
                self.keep_alive();
            }
            AppMessage::ShowStats => {
                // Execute bash script to show stats in a separate thread
                // /home/jarppa/projects/javascript/timings-stats/start.sh
                thread::spawn(
                    || match std::process::Command::new("./timings-stats").spawn() {
                        Ok(mut child) => {
                            if let Err(e) = child.wait() {
                                log::error!("Failed to wait for stats script: {}", e);
                            }
                        }
                        Err(e) => {
                            log::error!("Failed to start stats script: {}", e);
                        }
                    },
                );
            }
            AppMessage::ShowDailyTotals => {
                if let Err(e) = self.show_daily_totals().await {
                    log::error!("Failed to show daily totals: {}", e);
                }
            }
            AppMessage::ShowDailySummaries => {
                if let Err(e) = self.show_daily_summaries().await {
                    log::error!("Failed to show daily summaries: {}", e);
                }
            }
            AppMessage::TrayIconClicked => {
                self.show_gui(app);
            }
            AppMessage::VirtualDesktop(vd_msg) => match vd_msg {
                VirtualDesktopMessage::DesktopNameChanged(id, name) => {
                    if *id == self.current_desktop {
                        self.start_timing_from_desktop_name(name);
                    }
                }
                VirtualDesktopMessage::DesktopChange(id) => {
                    let name = self
                        .desktop_controller
                        .get_desktop_name(id)
                        .await
                        .unwrap_or_else(|_| "Unknown".to_string());
                    self.current_desktop = id.clone();
                    self.start_timing_from_desktop_name(&name);
                    self.show_gui(app);
                }
            },
            AppMessage::UserIdled => {
                log::trace!("User activity changed to idling");
                self.stop_timing();
            }
            AppMessage::UserResumed => {
                log::trace!("User activity changed to resumed");
                self.start_timing().await?;
            }
            AppMessage::VirtualDesktopThreadExited => {
                log::warn!(
                    "Virtual desktop listener thread has exited, this happens if the D-Bus \
                     connection is lost for instance when user closes the desktop but not the \
                     application."
                );
                return Err("Virtual desktop listener thread has exited".into());
            }
            AppMessage::AnotherInstanceTriedToStart => {
                log::info!("Another instance tried to start");
            }
            AppMessage::HideLayerOverlay => {
                self.hide_gui();
            }
            AppMessage::RequestRender => {
                // timings_app.request_gui_frame();
            }
            AppMessage::RunningChanged(is_running) => {
                log::info!("Timings recorder running state changed: {}", is_running);
                let icon = if *is_running {
                    &self.green_icon
                } else {
                    &self.red_icon
                };
                self.tray_icon.set_icon(icon).ok();
            }
            _ => {}
        }

        Ok(false)
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
        println!("3: Show daily summaries from past 4 weeks");
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
                "3" => {
                    let _ = app_message_sender.send(AppMessage::ShowDailySummaries);
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
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(3 * 60)).await;
            if app_message_sender.send(AppMessage::WriteTimings).is_err() {
                // Main thread has exited, stop the loop
                break;
            }
        }
    });
}

/// Spawns a keep alive thread for timings recorder
fn spawn_keepalive_thread(app_message_sender: tokio::sync::mpsc::UnboundedSender<AppMessage>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
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
