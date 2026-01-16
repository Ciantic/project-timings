use egui::CentralPanel;
use egui::Context;
use smithay_client_toolkit::reexports::client::Connection;
use smithay_client_toolkit::reexports::client::QueueHandle;
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::Anchor;
use smithay_client_toolkit::shell::wlr_layer::KeyboardInteractivity;
use smithay_client_toolkit::shell::wlr_layer::Layer;
use smithay_client_toolkit::shell::wlr_layer::LayerSurface;
use std::cell::RefCell;
use std::rc::Rc;
use virtual_desktops::KDEVirtualDesktopController;
use virtual_desktops::VirtualDesktopController;
use wayapp::Application;
use wayapp::EguiAppData;
use wayapp::RequestFrame;

pub struct ProjectTimingsGui {
    connection: Connection,
    queue_handle: QueueHandle<Application>,
    client: String,
    project: String,
    desktop_controller: KDEVirtualDesktopController,
    layer_surface: LayerSurface,
}

impl ProjectTimingsGui {
    pub fn new(
        connection: &Connection,
        queue_handle: &QueueHandle<Application>,
        layer_surface: &LayerSurface,
        desktop_controller: &KDEVirtualDesktopController,
    ) -> Self {
        Self {
            connection: connection.clone(),
            queue_handle: queue_handle.clone(),
            client: String::new(),
            project: String::new(),
            desktop_controller: desktop_controller.clone(),
            layer_surface: layer_surface.clone(),
        }
    }
}

impl EguiAppData for ProjectTimingsGui {
    fn ui(&mut self, ctx: &Context) {
        CentralPanel::default().show(ctx, |ui| {
            ui.heading("Project Timings");

            ui.separator();

            ui.horizontal(|ui| {
                ui.label("Client:");
                ui.text_edit_singleline(&mut self.client);
            });

            ui.horizontal(|ui| {
                ui.label("Project:");
                ui.text_edit_singleline(&mut self.project);
            });

            ui.separator();

            if ui.button("Update Desktop Name").clicked() {
                self.update_desktop_name();
            }
        });
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
        self.layer_surface.request_frame(&self.queue_handle);
        self.connection.flush().unwrap();
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
