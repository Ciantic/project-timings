use egui::CentralPanel;
use egui::Context;
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::Anchor;
use smithay_client_toolkit::shell::wlr_layer::KeyboardInteractivity;
use smithay_client_toolkit::shell::wlr_layer::Layer;
use std::cell::RefCell;
use std::rc::Rc;
use wayapp::Application;
use wayapp::EguiAppData;
use wayapp::EguiLayerSurface;

pub struct ProjectTimingsGui {
    counter: i32,
    text: String,
}

impl Default for ProjectTimingsGui {
    fn default() -> Self {
        Self {
            counter: 0,
            text: "Hello from EGUI!".into(),
        }
    }
}

impl EguiAppData for ProjectTimingsGui {
    fn ui(&mut self, ctx: &Context) {
        CentralPanel::default().show(ctx, |ui| {
            ui.heading("Egui WGPU / Smithay - Async Multi-Source");

            ui.separator();

            ui.label(format!("Counter: {}", self.counter));
            if ui.button("Increment").clicked() {
                self.counter += 1;
            }
            if ui.button("Decrement").clicked() {
                self.counter -= 1;
            }

            ui.separator();

            ui.horizontal(|ui| {
                ui.label("Text input:");
                ui.text_edit_singleline(&mut self.text);
            });

            ui.label(format!("You wrote: {}", self.text));

            ui.separator();

            ui.label("This demonstrates async multi-source event handling!");
        });
    }
}

pub fn init_project_timings_gui(app: &mut Application) -> Rc<RefCell<ProjectTimingsGui>> {
    let shared_surface = app.compositor_state.create_surface(&app.qh);
    let layer_surface = app.layer_shell.create_layer_surface(
        &app.qh,
        shared_surface.clone(),
        Layer::Top,
        Some("AsyncExample"),
        None,
    );
    layer_surface.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
    layer_surface.set_anchor(Anchor::BOTTOM);
    layer_surface.set_margin(0, 0, 20, 20);
    layer_surface.set_size(256, 256);
    layer_surface.commit();

    let project_timings_gui = Rc::new(RefCell::new(ProjectTimingsGui::default()));
    let egui_layer_surface =
        EguiLayerSurface::new(layer_surface, project_timings_gui.clone(), 256, 256);
    app.push_layer_surface(egui_layer_surface);
    project_timings_gui
}
