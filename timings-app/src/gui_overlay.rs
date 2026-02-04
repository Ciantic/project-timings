use crate::AppMessage;
use crate::TimingsApp;
use crate::parse_desktop_name;
use crate::utils::run_debounced_spawn;
use chrono::Local;
use chrono::NaiveDate;
use chrono::Utc;
use egui::CentralPanel;
use egui::Color32;
use egui::Context;
use egui::Pos2;
use smithay_client_toolkit::seat::pointer::PointerEventKind;
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::Anchor;
use smithay_client_toolkit::shell::wlr_layer::KeyboardInteractivity;
use smithay_client_toolkit::shell::wlr_layer::Layer;
use smithay_client_toolkit::shell::wlr_layer::LayerSurface;
use sqlx::SqlitePool;
use std::collections::HashMap;
use timings::SummaryForDay;
use timings::TimingsMutations;
use timings::TimingsRecording;
use tokio::sync::mpsc::UnboundedSender;
use virtual_desktops::DesktopId;
use virtual_desktops::KDEVirtualDesktopController;
use virtual_desktops::VirtualDesktopController;
use virtual_desktops::VirtualDesktopMessage;
use wayapp::Application;
use wayapp::EguiSurfaceState;
use wayapp::WaylandEvent;

#[derive(Debug, PartialEq, Clone)]
pub enum GuiOverlayEvent {
    UpdateTotalsTimer,
    UpdateSummaryCache {
        day: NaiveDate,
        client: String,
        project: String,
    },
    UpdateSummary {
        day: NaiveDate,
        client: String,
        project: String,
        summary: String,
    },
}

pub struct GuiOverlay {
    surface_state: Option<EguiSurfaceState<LayerSurface>>,
    pool: SqlitePool,

    has_keyboard_focus: bool,

    current_desktop: DesktopId,
    desktop_controller: KDEVirtualDesktopController,

    gui_debug_mode: bool,
    gui_fps: f32,
    gui_client: String,
    gui_project: String,
    gui_summary: String,
    gui_totals: HashMap<(String, String), timings::Totals>,

    app_message_sender: UnboundedSender<AppMessage>,
    update_totals_thread: tokio::task::JoinHandle<()>,
}

impl GuiOverlay {
    pub fn new(
        app: &Application,
        parent: &mut TimingsApp,
        app_message_sender: UnboundedSender<AppMessage>,
        pool: SqlitePool,
        desktop_controller: KDEVirtualDesktopController,
    ) -> Self {
        let surface_state = {
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
        let current_desktop =
            futures::executor::block_on(desktop_controller.get_current_desktop()).unwrap();

        let current_desktop_name =
            futures::executor::block_on(desktop_controller.get_desktop_name(&current_desktop))
                .unwrap_or_default();

        let (gui_client, gui_project) = parse_desktop_name(&current_desktop_name);

        let gui_summary = parent.timings_recorder.get_summary_if_cached(
            Local::now().date_naive(),
            &gui_client.clone().unwrap_or_default(),
            &gui_project.clone().unwrap_or_default(),
        );

        Self {
            surface_state,
            pool,
            has_keyboard_focus: false,
            gui_debug_mode: false,
            gui_fps: 0.0,
            gui_client: gui_client.unwrap_or_default(),
            gui_project: gui_project.unwrap_or_default(),
            gui_summary: gui_summary.unwrap_or_default(),
            gui_totals: HashMap::new(),
            current_desktop,
            desktop_controller,
            app_message_sender: app_message_sender.clone(),
            update_totals_thread: spawn_update_totals_thread(app_message_sender.clone()),
        }
    }

    pub fn has_keyboard_focus(&self) -> bool {
        self.has_keyboard_focus
    }

    pub async fn update_totals(&mut self, parent: &mut TimingsApp) {
        let client = self.gui_client.trim().to_string();
        let project = self.gui_project.trim().to_string();
        log::trace!("Updating totals cache");
        let now = chrono::Utc::now();
        if let Some(totals) = parent
            .timings_recorder
            .get_totals(&client, &project, now)
            .await
            .ok()
        {
            self.gui_totals
                .insert((client.clone(), project.clone()), totals);
        }
    }

    fn on_gui_client_or_project_changed(&mut self, parent: &mut TimingsApp) {
        let client = self.gui_client.trim().to_string();
        let project = self.gui_project.trim().to_string();
        let current_desktop = self.current_desktop.clone();
        let mut controller = self.desktop_controller.clone();
        let app_message_sender = self.app_message_sender.clone();
        let today = Local::now().date_naive();
        self.update_gui_summary_from_cache(parent);

        run_debounced_spawn(
            "update_client_or_project",
            std::time::Duration::from_millis(300),
            async move {
                let _ = controller
                    .update_desktop_name(current_desktop, &format!("{}: {}", client, project))
                    .await;

                let _ = app_message_sender.send(AppMessage::GuiOverlayEvent(
                    GuiOverlayEvent::UpdateSummaryCache {
                        day: today,
                        client,
                        project,
                    },
                ));
            },
        );
    }

    fn update_gui_summary_from_cache(&mut self, parent: &mut TimingsApp) {
        let day = Local::now().date_naive();
        let client = self.gui_client.trim().to_string();
        let project = self.gui_project.trim().to_string();
        self.gui_summary = parent
            .timings_recorder
            .get_summary_if_cached(day, &client, &project)
            .unwrap_or_default();
    }

    fn on_gui_summary_changed(&mut self, parent: &mut TimingsApp) {
        let day = Local::now().date_naive();
        let client = self.gui_client.trim().to_string();
        let project = self.gui_project.trim().to_string();
        let summary = self.gui_summary.trim().to_string();
        let tx = self.app_message_sender.clone();
        run_debounced_spawn(
            "update_summary_database",
            std::time::Duration::from_millis(300),
            async move {
                tx.send(AppMessage::GuiOverlayEvent(
                    GuiOverlayEvent::UpdateSummary {
                        day,
                        client,
                        project,
                        summary,
                    },
                ))
                .ok();
            },
        );
    }

    fn overlay_ui(&mut self, ctx: &Context, parent: &mut TimingsApp) {
        ctx.set_visuals(egui::Visuals::light());
        let bg_color = ctx.style().visuals.panel_fill;
        let is_running = parent.timings_recorder.is_running();
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
                    let summary_input = ui.add_enabled(
                        true,
                        egui::TextEdit::singleline(&mut self.gui_summary)
                            .desired_width(f32::INFINITY)
                            .horizontal_align(egui::Align::Center)
                            .background_color(Color32::from_white_alpha(0))
                            .font(egui::FontId::new(13.0, egui::FontFamily::Proportional)),
                    );

                    // When client or project changes, call on_gui_client_or_project_changed
                    if client_input.changed() || project_input.changed() {
                        self.on_gui_client_or_project_changed(parent);
                    }

                    // When typing to summary, call update_summary_from_gui
                    if summary_input.changed() {
                        self.on_gui_summary_changed(parent);
                    }
                });

                ui.vertical_centered(|ui| {
                    ui.set_max_width(150.0);
                    ui.set_max_height(45.0);
                    ui.horizontal_centered(|ui| {
                        let circle_color = if parent.timings_recorder.is_running() {
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

    fn request_frame(&mut self) {
        if let Some(ref mut surface_state) = self.surface_state {
            surface_state.request_frame();
        }
    }

    pub fn handle_wayland_events(
        &mut self,
        parent: &mut TimingsApp,
        app: &mut Application,
        events: &[WaylandEvent],
    ) {
        if let Some(mut surface_state) = self.surface_state.take() {
            self.gui_fps = surface_state.get_fps();
            surface_state.handle_events(app, events, &mut |ctx| self.overlay_ui(ctx, parent));
            for event in events {
                if let Some(wl_surface) = event.get_wl_surface() {
                    if surface_state.get_content().wl_surface() != wl_surface {
                        continue;
                    }
                }

                match event {
                    WaylandEvent::KeyboardEnter(_, _, _) => {
                        self.has_keyboard_focus = true;
                        parent.stop_timing();
                        self.request_frame();
                    }
                    WaylandEvent::KeyboardLeave(_) => {
                        self.has_keyboard_focus = false;
                        futures::executor::block_on(parent.start_timing()).unwrap();
                        surface_state.set_keyboard_interactivity(KeyboardInteractivity::None);
                        self.request_frame();
                        parent.hide_gui_after_delay();
                    }
                    WaylandEvent::PointerEvent((_, _, PointerEventKind::Press { .. })) => {
                        surface_state.set_keyboard_interactivity(KeyboardInteractivity::OnDemand);
                    }
                    _ => {}
                }
            }
            self.surface_state = Some(surface_state);
        }
    }

    pub async fn handle_app_events(
        &mut self,
        parent: &mut TimingsApp,
        _app: &mut Application,
        event: &AppMessage,
    ) {
        match event {
            AppMessage::GuiOverlayEvent(gui_event) => {
                match gui_event {
                    GuiOverlayEvent::UpdateTotalsTimer => {
                        futures::executor::block_on(self.update_totals(parent));
                    }
                    GuiOverlayEvent::UpdateSummaryCache {
                        day,
                        client,
                        project,
                    } => {
                        parent
                            .timings_recorder
                            .update_summary_cache(
                                day.clone(),
                                &client.clone(),
                                &project.clone(),
                                Utc::now(),
                            )
                            .await
                            .ok();
                    }
                    GuiOverlayEvent::UpdateSummary {
                        day,
                        client,
                        project,
                        summary,
                    } => {
                        parent
                            .timings_recorder
                            .update_summary(*day, client, project, summary)
                            .await
                            .ok();
                    }
                }
                self.request_frame();
            }
            AppMessage::VirtualDesktop(vdm) => match vdm {
                VirtualDesktopMessage::DesktopChange(desktop_id) => {
                    self.current_desktop = desktop_id.clone();
                    let desktop_name = self
                        .desktop_controller
                        .get_desktop_name(desktop_id)
                        .await
                        .unwrap_or_default();
                    let (gui_client, gui_project) = parse_desktop_name(&desktop_name);
                    self.gui_summary = parent
                        .timings_recorder
                        .update_summary_cache(
                            Local::now().date_naive(),
                            &gui_client.clone().unwrap_or_default(),
                            &gui_project.clone().unwrap_or_default(),
                            Utc::now(),
                        )
                        .await
                        .unwrap_or_default();
                    self.gui_client = gui_client.unwrap_or_default();
                    self.gui_project = gui_project.unwrap_or_default();
                    self.request_frame();
                }
                VirtualDesktopMessage::DesktopNameChanged(desktop_id, desktop_name) => {
                    if *desktop_id == self.current_desktop {
                        let (gui_client, gui_project) = parse_desktop_name(&desktop_name);
                        self.gui_summary = parent
                            .timings_recorder
                            .update_summary_cache(
                                Local::now().date_naive(),
                                &gui_client.clone().unwrap_or_default(),
                                &gui_project.clone().unwrap_or_default(),
                                Utc::now(),
                            )
                            .await
                            .unwrap_or_default();
                        self.gui_client = gui_client.unwrap_or_default();
                        self.gui_project = gui_project.unwrap_or_default();
                        self.request_frame();
                    }
                }
            },
            AppMessage::RunningChanged(_) => {
                self.request_frame();
            }
            _ => {}
        }
    }
}

impl Drop for GuiOverlay {
    fn drop(&mut self) {
        self.update_totals_thread.abort();
    }
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

/// Spawns a thread that sends KeepAlive message every 30 seconds
fn spawn_update_totals_thread(
    app_message_sender: UnboundedSender<AppMessage>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if app_message_sender
                .send(AppMessage::GuiOverlayEvent(
                    GuiOverlayEvent::UpdateTotalsTimer,
                ))
                .is_err()
            {
                // Main thread has exited, stop the loop
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
    })
}
