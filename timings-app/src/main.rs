use chrono::Duration;
use clap::Parser;
use egui::CentralPanel;
use egui::Color32;
use egui::Context;
use futures::StreamExt;
use idle_monitor::run_idle_monitor;
use log::trace;
use single_instance::only_single_instance;
use smithay_client_toolkit::seat::pointer::PointerEventKind;
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::Anchor;
use smithay_client_toolkit::shell::wlr_layer::KeyboardInteractivity;
use smithay_client_toolkit::shell::wlr_layer::Layer;
use smithay_client_toolkit::shell::wlr_layer::LayerSurface;
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Mutex;
use std::thread;
use timings::TimingsMutations;
use timings::TimingsRecording;
use tokio::select;
use tokio::sync::mpsc::UnboundedSender;
use trayicon::Icon;
use trayicon::MenuBuilder;
use trayicon::TrayIconBuilder;
use virtual_desktops::KDEVirtualDesktopController;
use virtual_desktops::VirtualDesktopController;
use virtual_desktops::VirtualDesktopMessage;
use wayapp::Application;
use wayapp::EguiSurfaceState;
use wayapp::WaylandEvent;

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
    Exit,
    WriteTimings,
    KeepAlive,
    UpdateTotals,
    ShowDailyTotals,
    TrayIconClicked,
    VirtualDesktop(VirtualDesktopMessage),
    VirtualDesktopThreadExited,
    HideLayerOverlay,
    UserIdled,
    UserResumed,
    AnotherInstanceTriedToStart,
    RequestRender,
}

#[tokio::main(flavor = "current_thread")]
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

    // Start the timings app
    let mut timings_app = TimingsApp::new(
        &database_path,
        Duration::seconds(cli.minimum_timing as i64),
        appmsg_sender.clone(),
        &desktop_controller,
    )
    .await?;

    // Initialize timing for the current desktop
    timings_app.start_timing().await?;

    spawn_idle_monitor_thread(appmsg_sender.clone(), cli.idle_timeout);
    spawn_stdin_reader(appmsg_sender.clone());
    spawn_write_timings_thread(appmsg_sender.clone());
    spawn_keepalive_thread(appmsg_sender.clone());
    spawn_virtual_desktop_listener(desktop_controller.clone(), appmsg_sender.clone());
    spawn_update_totals_thread(appmsg_sender.clone());

    let mut app = Application::new();
    let mut event_queue = app.event_queue.take().unwrap();
    loop {
        select! {
            // Wait for Wayland events in a blocking task, then dispatch them
            _ = tokio::task::spawn_blocking({
                let conn = app.conn.clone();
                move || {
                    if let Some(guard) = conn.prepare_read() {
                        guard.read_without_dispatch().unwrap();
                    }
                }
            }) => {
                let _ = event_queue.dispatch_pending(&mut app);
                let events = app.take_wayland_events();
                timings_app.handle_gui_events(&mut app, &events);
            }

            // Other app events
            Some(event) = appmsgs.recv() => {
                match event {
                    AppMessage::Exit => {
                        break Ok(());
                    }
                    AppMessage::WriteTimings => {
                        if let Err(e) = timings_app.write_timings().await {
                            log::error!("Failed to write timings: {}", e);
                        }
                    }
                    AppMessage::KeepAlive => {
                        log::trace!("Keep alive timing");
                        timings_app.keep_alive();
                    }
                    AppMessage::ShowDailyTotals => {
                        if let Err(e) = timings_app.show_daily_totals().await {
                            log::error!("Failed to show daily totals: {}", e);
                        }
                    }
                    AppMessage::TrayIconClicked => {
                        timings_app.update_totals().await;
                        timings_app.show_gui(&mut app);
                    }
                    AppMessage::VirtualDesktop(vd_msg) => match vd_msg {
                        VirtualDesktopMessage::DesktopNameChanged(id, name) => {
                            let current_desktop = desktop_controller.get_current_desktop().await?;
                            if id == current_desktop {
                                timings_app.start_timing_from_desktop_name(&name);
                                timings_app.update_totals().await;
                            }
                        }
                        VirtualDesktopMessage::DesktopChange(id) => {
                            let name = desktop_controller
                                .get_desktop_name(&id)
                                .await
                                .unwrap_or_else(|_| "Unknown".to_string());
                            timings_app.start_timing_from_desktop_name(&name);
                            timings_app.update_totals().await;
                            timings_app.show_gui(&mut app);
                        }
                    },
                    AppMessage::UserIdled => {
                        log::trace!("User activity changed to idling");
                        timings_app.stop_timing();
                    }
                    AppMessage::UserResumed => {
                        log::trace!("User activity changed to resumed");
                        let _ = timings_app.update_totals().await;
                        timings_app.resume_timing();
                    }
                    AppMessage::VirtualDesktopThreadExited => {
                        log::warn!(
                            "Virtual desktop listener thread has exited, this happens if the D-Bus \
                            connection is lost for instance when user closes the desktop but not the \
                            application."
                        );
                        break Err("Virtual desktop listener thread has exited".into());
                    }
                    AppMessage::AnotherInstanceTriedToStart => {
                        log::info!("Another instance tried to start");
                    }
                    AppMessage::HideLayerOverlay => {
                        timings_app.hide_gui();
                    }
                    AppMessage::RequestRender => {
                        timings_app.request_gui_frame(&mut app);
                    }
                    AppMessage::UpdateTotals => {
                        let _ = timings_app.update_totals().await;
                        timings_app.request_gui_frame(&mut app);
                    }
                }
            }
        }
    }
}

struct TimingsApp {
    // Timing recording fields
    client: Option<String>,
    project: Option<String>,
    timings_recorder: timings::TimingsRecorder,
    pool: SqlitePool,
    sender: UnboundedSender<AppMessage>,
    desktop_controller: KDEVirtualDesktopController,
    is_running: bool,

    // GUI fields
    gui_client: String,
    gui_project: String,
    gui_totals: HashMap<(String, String), timings::Totals>,
    has_keyboard_focus: bool,
    egui_surface_state: Option<EguiSurfaceState<LayerSurface>>,

    // Tray icon
    tray_icon: trayicon::TrayIcon<AppMessage>,
    green_icon: Icon,
    red_icon: Icon,
}

impl TimingsApp {
    pub async fn new(
        database: &str,
        minimum_timing: Duration,
        sender: UnboundedSender<AppMessage>,
        desktop_controller: &KDEVirtualDesktopController,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let options = SqliteConnectOptions::from_str(database)?.create_if_missing(true);

        let pool = SqlitePool::connect_with(options).await?;
        let mut conn = pool.acquire().await?;
        conn.create_timings_database().await?;
        drop(conn);

        let timings_recorder = timings::TimingsRecorder::new(minimum_timing);

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
                    .item("Show daily totals", AppMessage::ShowDailyTotals)
                    .item("Exit", AppMessage::Exit),
            )
            .build()?;

        Ok(Self {
            client: None,
            project: None,
            timings_recorder,
            pool,
            sender,
            desktop_controller: desktop_controller.clone(),
            gui_totals: HashMap::new(),
            gui_client: String::new(),
            gui_project: String::new(),
            has_keyboard_focus: false,
            egui_surface_state: None,
            is_running: false,
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
        let (client, project) = Self::parse_desktop_name(desktop_name);
        let old_client = self.client.clone();
        let old_project = self.project.clone();
        self.client = client.clone();
        self.project = project.clone();
        self.gui_client = client.clone().unwrap_or_default();
        self.gui_project = project.clone().unwrap_or_default();

        if let (Some(client), Some(project)) = (client, project) {
            log::info!(
                "Starting timing: client='{}', project='{}' (previous: client={:?}, project={:?})",
                client,
                project,
                old_client,
                old_project
            );
            if self.timings_recorder.start_timing(
                client.clone(),
                project.clone(),
                chrono::Utc::now(),
            ) {
                self.is_running = true;
                self.tray_icon.set_icon(&self.green_icon).ok();
            } else {
                self.is_running = false;
                self.tray_icon.set_icon(&self.red_icon).ok();
            }
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
        let current_desktop = self.desktop_controller.get_current_desktop().await?;
        let current_desktop_name = self
            .desktop_controller
            .get_desktop_name(&current_desktop)
            .await
            .unwrap_or_else(|_| "Unknown".to_string());
        self.start_timing_from_desktop_name(&current_desktop_name);
        Ok(())
    }

    /// Stops the current timing.
    pub fn stop_timing(&mut self) {
        log::info!("Stopping timing");
        self.timings_recorder.stop_timing(chrono::Utc::now());
        self.tray_icon.set_icon(&self.red_icon).ok();
        self.is_running = false;
        self.sender.send(AppMessage::RequestRender).ok();
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
            self.is_running = true;
            self.tray_icon.set_icon(&self.green_icon).ok();
        }
    }

    /// Updates the totals cache.
    pub async fn update_totals(&mut self) {
        if let Some(client) = self.client.as_ref()
            && let Some(project) = self.project.as_ref()
            && self.egui_surface_state.is_some()
        {
            log::info!("Updating totals cache");
            let mut conn = self.pool.acquire().await.unwrap();
            let now = chrono::Utc::now();
            if let Some(totals) = self
                .timings_recorder
                .get_totals(client, project, now, &mut *conn)
                .await
                .ok()
            {
                self.gui_totals
                    .insert((client.clone(), project.clone()), totals);
            }
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

    // GUI methods
    pub fn show_gui(&mut self, app: &mut Application) {
        hide_overlay_after_delay(self.sender.clone(), 3);
        if self.egui_surface_state.is_some() {
            return;
        }
        self.egui_surface_state = Some(make_layer_surface(app));
        self.request_gui_frame(app);
    }

    pub fn hide_gui(&mut self) {
        if self.has_keyboard_focus {
            log::info!("Not hiding overlay, has keyboard focus");
            return;
        }
        self.egui_surface_state = None;
    }

    pub fn handle_gui_events(&mut self, app: &mut Application, events: &[WaylandEvent]) {
        // Handle egui surface events
        if let Some(mut surface_state) = self.egui_surface_state.take() {
            surface_state.handle_events(app, events, &mut |ctx| self.overlay_ui(ctx));
            self.egui_surface_state = Some(surface_state);
        }

        // Handle other Wayland events
        for event in events {
            match event {
                WaylandEvent::KeyboardEnter(_, ..) => {
                    trace!("Overlay keyboard enter");
                    self.has_keyboard_focus = true;
                }
                WaylandEvent::KeyboardLeave(_, ..) => {
                    trace!("Overlay keyboard leave");
                    self.has_keyboard_focus = false;
                    hide_overlay_after_delay(self.sender.clone(), 3);
                    self.egui_surface_state.as_ref().map(|s| {
                        s.set_keyboard_interactivity(KeyboardInteractivity::None);
                    });
                }
                WaylandEvent::PointerEvent((_, _, PointerEventKind::Press { .. })) => {
                    self.egui_surface_state.as_ref().map(|s| {
                        s.set_keyboard_interactivity(KeyboardInteractivity::OnDemand);
                    });
                }
                _ => {}
            }
        }
    }

    pub fn request_gui_frame(&mut self, app: &mut Application) {
        if let Some(ref mut surface_state) = self.egui_surface_state {
            surface_state.request_frame();
            let _ = app.conn.flush();
        }
    }

    fn update_desktop_name_from_gui(&mut self) {
        let desktop_name = format!("{}: {}", self.gui_client, self.gui_project);
        log::info!("Updating desktop name to: {}", desktop_name);
        if let Err(e) =
            futures::executor::block_on(self.desktop_controller.update_desktop_name(&desktop_name))
        {
            log::error!("Failed to update desktop name: {}", e);
        }
    }

    fn overlay_ui(&mut self, ctx: &Context) {
        ctx.set_visuals(egui::Visuals::light());
        let bg_color = ctx.style().visuals.panel_fill;

        CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(bg_color)
                    .stroke(egui::Stroke::new(
                        2.0,
                        if self.has_keyboard_focus {
                            egui::Color32::LIGHT_BLUE
                        } else {
                            egui::Color32::GRAY
                        },
                    ))
                    .inner_margin(10.0),
            )
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.gui_client)
                            .desired_width(f32::INFINITY)
                            .horizontal_align(egui::Align::Center)
                            .background_color(Color32::from_white_alpha(0))
                            .font(egui::FontId::new(20.0, egui::FontFamily::Proportional)),
                    );
                    ui.add_space(5.0);
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.gui_project)
                            .desired_width(f32::INFINITY)
                            .horizontal_align(egui::Align::Center)
                            .background_color(Color32::from_white_alpha(0))
                            .font(egui::FontId::new(20.0, egui::FontFamily::Proportional)),
                    );

                    if resp.lost_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                        // println!("Updating desktop name from GUI");
                        self.update_desktop_name_from_gui();
                    }
                });

                // ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                //     if ui.button("Update name").clicked() {
                //         self.update_desktop_name_from_gui();
                //     }
                // });

                // Show label (focused) if has keyboard focus
                // if self.has_keyboard_focus {
                //     ui.label("Keyboard focused");
                // }

                ui.vertical_centered(|ui| {
                    ui.set_max_width(150.0);
                    ui.set_max_height(65.0);
                    ui.horizontal_centered(|ui| {
                        let circle_color = if self.is_running {
                            egui::Color32::GREEN
                        } else {
                            egui::Color32::RED
                        };

                        let (response, painter) =
                            ui.allocate_painter(egui::Vec2::splat(30.0), egui::Sense::empty());
                        let center = response.rect.center();
                        painter.circle_filled(
                            center,
                            if self.is_running { 9.5 } else { 4.0 },
                            circle_color,
                        );
                        ui.label(
                            egui::RichText::new(
                                self.gui_totals
                                    .get(&(self.gui_client.clone(), self.gui_project.clone()))
                                    .as_ref()
                                    .map(|t| duration_to_hh_mm_ss(&t.today))
                                    // .map(|t| format!("{:.5} hours", t.today.num_seconds() as f64
                                    // / 3600.0))
                                    .unwrap_or_else(|| "00:00:00".to_string()),
                            )
                            .size(20.0),
                        );
                    });
                });

                ui.columns(3, |cols| {
                    // Last 8 weeks column
                    cols[0].vertical_centered(|ui| {
                        ui.label("Eight weeks");
                        ui.label(
                            self.gui_totals
                                .get(&(self.gui_client.clone(), self.gui_project.clone()))
                                .as_ref()
                                .map(|t| duration_to_hours(&t.eight_weeks))
                                .unwrap_or_else(|| "N/A".to_string()),
                        );
                    });

                    // Last week column
                    cols[1].vertical_centered(|ui| {
                        ui.label("Last week");
                        ui.label(
                            self.gui_totals
                                .get(&(self.gui_client.clone(), self.gui_project.clone()))
                                .as_ref()
                                .map(|t| duration_to_hours(&t.last_week))
                                .unwrap_or_else(|| "N/A".to_string()),
                        );
                    });

                    // This week column
                    cols[2].vertical_centered(|ui| {
                        ui.label("This week");
                        ui.label(
                            self.gui_totals
                                .get(&(self.gui_client.clone(), self.gui_project.clone()))
                                .as_ref()
                                .map(|t| duration_to_hours(&t.this_week))
                                .unwrap_or_else(|| "N/A".to_string()),
                        );
                    });
                });
            });
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

/// Spawns a thread that sends a tick message every second
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
/// Spawns a thread that sends KeepAlive message every 30 seconds
fn spawn_update_totals_thread(app_message_sender: tokio::sync::mpsc::UnboundedSender<AppMessage>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            if app_message_sender.send(AppMessage::UpdateTotals).is_err() {
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

static HIDE_OVERLAY_TASK: Mutex<Option<tokio::task::JoinHandle<()>>> = Mutex::new(None);

fn hide_overlay_after_delay(
    sender: tokio::sync::mpsc::UnboundedSender<AppMessage>,
    delay_secs: u64,
) {
    let mut task = HIDE_OVERLAY_TASK.lock().unwrap();

    // Cancel existing task if any
    if let Some(handle) = task.take() {
        handle.abort();
    }

    // Start new task
    *task = Some(tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_secs(delay_secs)).await;
        let _ = sender.send(AppMessage::HideLayerOverlay);
    }));
}

pub fn make_layer_surface(app: &mut Application) -> EguiSurfaceState<LayerSurface> {
    let first_monitor = app
        .output_state
        .outputs()
        .collect::<Vec<_>>()
        .get(0)
        .cloned();
    let layer_surface = app.layer_shell.create_layer_surface(
        &app.qh,
        app.compositor_state.create_surface(&app.qh),
        Layer::Top,
        Some("ProjectTimings"),
        first_monitor.as_ref(),
    );
    layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);
    #[cfg(debug_assertions)]
    layer_surface.set_anchor(Anchor::BOTTOM | Anchor::RIGHT);
    #[cfg(not(debug_assertions))]
    layer_surface.set_anchor(Anchor::BOTTOM | Anchor::LEFT);

    layer_surface.set_margin(0, 20, 20, 20);
    layer_surface.set_size(350, 200);
    layer_surface.commit();
    EguiSurfaceState::new(&app, layer_surface, 350, 200)
}

fn duration_to_hh_mm_ss(duration: &chrono::Duration) -> String {
    let total_seconds = duration.num_seconds();
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
}

fn duration_to_hours(duration: &chrono::Duration) -> String {
    format!("{:.2}", duration.num_seconds() as f64 / 3600.0)
}
