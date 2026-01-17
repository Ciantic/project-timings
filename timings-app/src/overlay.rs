use crate::AppMessage;
use egui::CentralPanel;
use egui::Context;
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::Anchor;
use smithay_client_toolkit::shell::wlr_layer::KeyboardInteractivity;
use smithay_client_toolkit::shell::wlr_layer::Layer;
use smithay_client_toolkit::shell::wlr_layer::LayerSurface;
use tokio::sync::mpsc::UnboundedSender;
use virtual_desktops::KDEVirtualDesktopController;
use virtual_desktops::VirtualDesktopController;
use wayapp::Application;
use wayapp::EguiAppData;
use wayapp::EguiSurfaceState;
use wayapp::WaylandEvent;

pub struct ProjectTimingsGui {
    sender: UnboundedSender<AppMessage>,
    client: String,
    project: String,
    desktop_controller: KDEVirtualDesktopController,
    has_keyboard_focus: bool,
    central_panel_has_focus: bool,
    egui_surface_state: Option<EguiSurfaceState<LayerSurface>>,
}

impl ProjectTimingsGui {
    pub fn new(
        sender: UnboundedSender<AppMessage>,
        desktop_controller: &KDEVirtualDesktopController,
    ) -> Self {
        Self {
            sender,
            client: String::new(),
            project: String::new(),
            desktop_controller: desktop_controller.clone(),
            has_keyboard_focus: false,
            central_panel_has_focus: false,
            egui_surface_state: None,
        }
    }

    pub fn show(&mut self, app: &mut Application) {
        if self.egui_surface_state.is_some() {
            return;
        }
        self.egui_surface_state = Some(make_layer_surface(app));
    }

    pub fn hide(&mut self) {
        if self.has_keyboard_focus {
            log::info!("Not hiding overlay, has keyboard focus");
            return;
        }
        self.egui_surface_state = None;
    }

    pub fn handle_events(&mut self, app: &mut Application, events: &[WaylandEvent]) {
        if let Some(mut surface_state) = self.egui_surface_state.take() {
            surface_state.handle_events(app, events, self);
            self.egui_surface_state = Some(surface_state);
        }
    }

    pub fn request_frame(&mut self, app: &mut Application) {
        if let Some(surface_state) = &mut self.egui_surface_state {
            surface_state.request_frame();
            let _ = app.conn.flush();
        }
    }
}

impl EguiAppData for ProjectTimingsGui {
    fn ui(&mut self, ctx: &Context) {
        ctx.set_visuals(egui::Visuals::light());
        let bg_color = ctx.style().visuals.panel_fill;

        self.has_keyboard_focus = false;

        let foo = CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(bg_color)
                    .stroke(egui::Stroke::new(1.0, egui::Color32::BLACK))
                    .inner_margin(10.0),
            )
            .show(ctx, |ui| {
                ui.heading("Project Timings");

                ui.separator();

                ui.horizontal(|ui| {
                    ui.label("Client:");
                    let client_response = ui.text_edit_singleline(&mut self.client);
                    if client_response.has_focus() {
                        self.has_keyboard_focus = true;
                    }
                });

                ui.horizontal(|ui| {
                    ui.label("Project:");
                    let project_response = ui.text_edit_singleline(&mut self.project);
                    if project_response.has_focus() {
                        self.has_keyboard_focus = true;
                    }
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                    if ui.button("Update name").clicked() {
                        self.update_desktop_name();
                    }
                });

                // Show label (focused) if has keyboard focus
                if self.has_keyboard_focus {
                    ui.label("Keyboard focused");
                }
                if self.central_panel_has_focus {
                    ui.label("Central panel focused");
                }

                ui.separator();

                ui.horizontal(|ui| {
                    ui.label("Current timing:");
                    ui.label("01:01:01");
                });
            });
        self.central_panel_has_focus = foo.response.has_focus();
    }
}

impl ProjectTimingsGui {
    /// Updates the client and project fields from a desktop name
    pub fn update_from_desktop_name(&mut self, desktop_name: &str) {
        let (client, project) = Self::parse_desktop_name(desktop_name);
        self.client = client.unwrap_or_default();
        self.project = project.unwrap_or_default();
        log::info!(
            "Updated overlay: client='{}', project='{}'",
            self.client,
            self.project
        );
        self.sender.send(AppMessage::RequestRender).unwrap();
    }

    fn update_desktop_name(&mut self) {
        if self.client.is_empty() || self.project.is_empty() {
            log::warn!("Client or Project is empty, not updating desktop name");
            return;
        }

        let desktop_name = format!("{}: {}", self.client, self.project);
        log::info!("Updating desktop name to: {}", desktop_name);
        if let Err(e) =
            futures::executor::block_on(self.desktop_controller.update_desktop_name(&desktop_name))
        {
            log::error!("Failed to update desktop name: {}", e);
        }

        // let mut controller = self.desktop_controller.clone();
        // tokio::spawn(async move {
        //     if let Err(e) =
        // controller.update_desktop_name(&desktop_name).await {
        //         log::error!("Failed to update desktop name: {}", e);
        //     } else {
        //         log::info!("Successfully updated desktop name");
        //     }
        // });
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
    layer_surface.set_anchor(Anchor::BOTTOM | Anchor::LEFT);
    layer_surface.set_margin(0, 0, 20, 20);
    layer_surface.set_size(320, 160);
    layer_surface.commit();
    EguiSurfaceState::new(&app, layer_surface, 320, 160)
}
