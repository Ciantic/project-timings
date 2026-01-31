use chrono::Duration;
use chrono::Local;
use chrono::NaiveDate;
use clap::Parser;
use egui::CentralPanel;
use egui::Color32;
use egui::Context;
use egui::Pos2;
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
use timings::SummaryForDay;
use timings::TimingsMockdata;
use timings::TimingsMutations;
use timings::TimingsQueries;
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
use wayapp::EguiSurfaceState;
use wayapp::WaylandEvent;
mod utils;
use utils::*;

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
    UpdateTotalsTimer,
    ShowDailyTotals,
    TrayIconClicked,
    VirtualDesktop(VirtualDesktopMessage),
    VirtualDesktopThreadExited,
    HideLayerOverlay,
    UserIdled,
    RunningChanged(bool),
    UserResumed,
    AnotherInstanceTriedToStart,
    RequestRender,
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

    let tx = appmsg_sender.clone();
    let mut timings_recorder =
        timings::TimingsRecorder::new(Duration::seconds(cli.minimum_timing as i64));

    timings_recorder.set_running_changed_callback(move |running| {
        let _ = tx.send(AppMessage::RunningChanged(running));
    });

    // Start the timings app
    let mut timings_app = TimingsApp::new(
        &database_path,
        timings_recorder,
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
    spawn_update_totals_thread(appmsg_sender.clone());
    app.run_dispatcher();
    loop {
        // Other app events
        if let Some(event) = appmsgs.recv().await {
            match event {
                AppMessage::WaylandDispatch(token) => {
                    let events = app.dispatch_pending(token);
                    timings_app.handle_gui_events(&mut app, &events);
                }
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
                    timings_app.show_gui(&mut app);
                    timings_app.update_totals().await;
                    timings_app.update_summary().await;
                }
                AppMessage::VirtualDesktop(vd_msg) => match vd_msg {
                    VirtualDesktopMessage::DesktopNameChanged(id, name) => {
                        if id == timings_app.current_desktop {
                            timings_app.start_timing_from_desktop_name(&name);
                            timings_app.update_totals().await;
                            timings_app.update_summary().await;
                            timings_app.request_gui_frame();
                        }
                    }
                    VirtualDesktopMessage::DesktopChange(id) => {
                        let name = desktop_controller
                            .get_desktop_name(&id)
                            .await
                            .unwrap_or_else(|_| "Unknown".to_string());
                        timings_app.current_desktop = id;
                        timings_app.start_timing_from_desktop_name(&name);
                        timings_app.show_gui(&mut app);
                        timings_app.update_totals().await;
                        timings_app.update_summary().await;
                        timings_app.request_gui_frame();
                    }
                },
                AppMessage::UserIdled => {
                    log::trace!("User activity changed to idling");
                    timings_app.stop_timing();
                }
                AppMessage::UserResumed => {
                    log::trace!("User activity changed to resumed");
                    timings_app.resume_timing();
                    let _ = timings_app.update_totals().await;
                    timings_app.request_gui_frame();
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
                    timings_app.request_gui_frame();
                }
                AppMessage::UpdateTotalsTimer => {
                    if timings_app.timings_recorder.is_running() {
                        let _ = timings_app.update_totals().await;
                        timings_app.request_gui_frame();
                    }
                }
                AppMessage::RunningChanged(is_running) => {
                    log::info!("Timings recorder running state changed: {}", is_running);
                    let icon = if is_running {
                        &timings_app.green_icon
                    } else {
                        &timings_app.red_icon
                    };
                    timings_app.tray_icon.set_icon(icon).ok();
                    timings_app.request_gui_frame();
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

    // Current desktop, updated on desktop change
    current_desktop: DesktopId,

    // GUI fields
    gui_debug_mode: bool,
    gui_fps: f32,
    gui_client: String,
    gui_project: String,
    gui_totals: HashMap<(String, String), timings::Totals>,
    gui_summaries: HashMap<(NaiveDate, String, String), Option<String>>,
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
        timings_recorder: TimingsRecorder,
        sender: UnboundedSender<AppMessage>,
        desktop_controller: &KDEVirtualDesktopController,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let options = SqliteConnectOptions::from_str(database)?.create_if_missing(true);

        let pool = SqlitePool::connect_with(options).await?;
        let mut conn = pool.acquire().await?;
        conn.create_timings_database().await?;

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
            current_desktop,
            gui_debug_mode: false,
            gui_fps: 0.0,
            gui_totals: HashMap::new(),
            gui_client: String::new(),
            gui_project: String::new(),
            gui_summaries: HashMap::new(),
            has_keyboard_focus: false,
            egui_surface_state: None,
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
        let old_client = self.client.clone();
        let old_project = self.project.clone();
        self.client = client.clone().map(|s| s.trim().to_string());
        self.project = project.clone().map(|s| s.trim().to_string());
        if !compare_client_and_project_names(
            &self.gui_client,
            &self.gui_project,
            &self.client,
            &self.project,
        ) {
            self.gui_client = self.client.clone().unwrap_or_default();
            self.gui_project = self.project.clone().unwrap_or_default();
        }

        if self.has_keyboard_focus {
            log::info!(
                "Not starting timing from desktop name '{}' because GUI has focus",
                desktop_name
            );
            return false;
        }

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

    pub async fn update_summary(&mut self) {
        if let Some(client) = self.client.as_ref()
            && let Some(project) = self.project.as_ref()
        {
            let today = Local::now().date_naive();
            let key = (today, client.clone(), project.clone());

            // Check if summary is already cached
            if self.gui_summaries.get(&key).map(|s| s.is_some()) == Some(true) {
                log::trace!("Summary already cached for {}: {}", client, project);
                return;
            }

            log::info!("Updating summary cache for {}: {}", client, project);
            let mut conn = self.pool.acquire().await.unwrap();

            // Query the database for the summary
            if let Ok(summaries) = conn
                .get_timings_daily_summaries(
                    Local,
                    today,
                    today,
                    Some(client.clone()),
                    Some(project.clone()),
                )
                .await
            {
                let summary = summaries.first().map(|s| s.summary.clone());
                self.gui_summaries
                    .insert(key, Some(summary.unwrap_or_default()));
            } else {
                // Cache empty string, to allow editing in GUI
                self.gui_summaries.insert(key, Some(String::new()));
            }
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

    // GUI methods
    pub fn show_gui(&mut self, app: &mut Application) {
        hide_overlay_after_delay(self.sender.clone(), 3);
        if self.egui_surface_state.is_some() {
            return;
        }
        self.egui_surface_state = {
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
            Some(EguiSurfaceState::new(&app, layer_surface, 350, 200))
        };
        self.request_gui_frame();
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
            self.gui_fps = surface_state.get_fps();
            surface_state.handle_events(app, events, &mut |ctx| self.overlay_ui(ctx));
            self.egui_surface_state = Some(surface_state);
        }

        // Handle other Wayland events
        for event in events {
            match event {
                WaylandEvent::KeyboardEnter(_, ..) => {
                    trace!("Overlay keyboard enter");
                    self.has_keyboard_focus = true;
                    self.stop_timing();
                }
                WaylandEvent::KeyboardLeave(_, ..) => {
                    trace!("Overlay keyboard leave");
                    self.has_keyboard_focus = false;
                    self.resume_timing();
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

    pub fn request_gui_frame(&mut self) {
        if let Some(ref mut surface_state) = self.egui_surface_state {
            surface_state.request_frame();
        }
    }

    fn on_gui_client_or_project_changed(&mut self) {
        let client = self.gui_client.trim().to_string();
        let project = self.gui_project.trim().to_string();
        let current_desktop = self.current_desktop.clone();
        let mut controller = self.desktop_controller.clone();
        run_debounced_spawn(
            "update_client_or_project",
            std::time::Duration::from_millis(300),
            async move {
                let _ = controller
                    .update_desktop_name(current_desktop, &format!("{}: {}", client, project))
                    .await;
                // Test
            },
        );
    }

    fn on_gui_summary_changed(&mut self) {
        let today = Local::now().date_naive();
        let client = self.gui_client.trim().to_string();
        let project = self.gui_project.trim().to_string();
        let summary = self
            .gui_summaries
            .get(&(today, self.gui_client.clone(), self.gui_project.clone()))
            .and_then(|opt| opt.as_ref())
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let pool = self.pool.clone();
        run_debounced_spawn(
            "update_summary_database",
            std::time::Duration::from_millis(300),
            async move {
                let mut conn = pool.acquire().await.unwrap();
                conn.insert_timings_daily_summaries(
                    Local,
                    &[SummaryForDay {
                        day: Local::now().date_naive(),
                        client: client.clone(),
                        project: project.clone(),
                        summary: summary,
                        archived: false,
                    }],
                )
                .await
                .unwrap();
            },
        );
    }

    fn overlay_ui(&mut self, ctx: &Context) {
        ctx.set_visuals(egui::Visuals::light());
        let today = Local::now().date_naive();
        let bg_color = ctx.style().visuals.panel_fill;
        let client = self.gui_client.trim().to_string();
        let project = self.gui_project.trim().to_string();
        let is_running = self.timings_recorder.is_running();
        let totals = self
            .gui_totals
            .get(&(
                self.gui_client.trim().to_string(),
                self.gui_project.trim().to_string(),
            ))
            .cloned();
        // User is holding alt key:
        let debug_mode = self.gui_debug_mode || ctx.input(|i| i.modifiers.alt);

        // Toggle debug mode with ALT+D
        if ctx.input(|i| i.modifiers.alt && i.key_pressed(egui::Key::D)) {
            self.gui_debug_mode = !self.gui_debug_mode;
        }

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
                if debug_mode {
                    let painter = ui.painter();
                    let screen_rect = ctx.content_rect();

                    painter.text(
                        Pos2::new(screen_rect.right() - 5.0, screen_rect.top() + 5.0),
                        egui::Align2::RIGHT_TOP,
                        format!(
                            "ALT+D {:7.2} / {:>4}",
                            self.gui_fps,
                            ctx.cumulative_pass_nr()
                        ),
                        egui::FontId::new(10.0, egui::FontFamily::Monospace),
                        egui::Color32::GRAY,
                    );
                }
                ui.vertical(|ui| {
                    // Client text field
                    let client_input = ui.add(
                        egui::TextEdit::singleline(&mut self.gui_client)
                            .desired_width(f32::INFINITY)
                            .horizontal_align(egui::Align::Center)
                            .background_color(Color32::from_white_alpha(0))
                            .font(egui::FontId::new(20.0, egui::FontFamily::Proportional)),
                    );

                    ui.add_space(5.0);

                    // Project text field
                    let project_input = ui.add(
                        egui::TextEdit::singleline(&mut self.gui_project)
                            .desired_width(f32::INFINITY)
                            .horizontal_align(egui::Align::Center)
                            .background_color(Color32::from_white_alpha(0))
                            .font(egui::FontId::new(20.0, egui::FontFamily::Proportional)),
                    );

                    ui.add_space(5.0);

                    // Summary text field
                    let summary_value = self
                        .gui_summaries
                        .entry((today, client.to_string(), project.to_string()))
                        .or_default();
                    let mut empty_value = String::new();
                    let summary_input = ui.add_enabled(
                        summary_value.is_some(),
                        egui::TextEdit::singleline(match summary_value {
                            Some(v) => v,
                            None => &mut empty_value,
                        })
                        .desired_width(f32::INFINITY)
                        .horizontal_align(egui::Align::Center)
                        .background_color(Color32::from_white_alpha(0))
                        .font(egui::FontId::new(13.0, egui::FontFamily::Proportional)),
                    );

                    // When client or project changes, call on_gui_client_or_project_changed
                    if client_input.changed() || project_input.changed() {
                        self.on_gui_client_or_project_changed();
                    }

                    // When typing to summary, call update_summary_from_gui
                    if summary_input.changed() {
                        self.on_gui_summary_changed();
                    }
                });

                ui.vertical_centered(|ui| {
                    ui.set_max_width(150.0);
                    ui.set_max_height(45.0);
                    ui.horizontal_centered(|ui| {
                        let circle_color = if is_running {
                            egui::Color32::GREEN
                        } else {
                            egui::Color32::RED
                        };

                        let (response, painter) =
                            ui.allocate_painter(egui::Vec2::splat(30.0), egui::Sense::empty());
                        let center = response.rect.center();
                        painter.circle_filled(
                            center,
                            if is_running { 9.5 } else { 4.0 },
                            circle_color,
                        );
                        ui.label(
                            egui::RichText::new(
                                &totals
                                    .clone()
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
                            &totals
                                .clone()
                                .map(|t| duration_to_hours(&t.eight_weeks))
                                .unwrap_or_else(|| "N/A".to_string()),
                        );
                    });

                    // Last week column
                    cols[1].vertical_centered(|ui| {
                        ui.label("Last week");
                        ui.label(
                            &totals
                                .clone()
                                .map(|t| duration_to_hours(&t.last_week))
                                .unwrap_or_else(|| "N/A".to_string()),
                        );
                    });

                    // This week column
                    cols[2].vertical_centered(|ui| {
                        ui.label("This week");
                        ui.label(
                            &totals
                                .clone()
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
            if app_message_sender
                .send(AppMessage::UpdateTotalsTimer)
                .is_err()
            {
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

fn compare_client_and_project_names(
    gui_client: &str,
    gui_project: &str,
    client: &Option<String>,
    project: &Option<String>,
) -> bool {
    let client_match = match client {
        Some(c) => gui_client.trim() == c.trim(),
        None => gui_client.is_empty(),
    };
    let project_match = match project {
        Some(p) => gui_project.trim() == p.trim(),
        None => gui_project.is_empty(),
    };
    client_match && project_match
}
