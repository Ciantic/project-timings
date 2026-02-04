#![allow(dead_code)]

use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::xdg::window::Window;
use smithay_client_toolkit::shell::xdg::window::WindowDecorations;
use sqlx::SqlitePool;
use wayapp::Application;
use wayapp::EguiSurfaceState;
use wayapp::WaylandEvent;

enum GuiStatsEvents {
    Close,
}

pub struct GuiStats {
    surface_state: EguiSurfaceState<Window>,
    pool: SqlitePool,
}

impl GuiStats {
    pub fn new(app: &Application, pool: SqlitePool) -> Self {
        let window = app.xdg_shell.create_window(
            app.compositor_state.create_surface(&app.qh),
            WindowDecorations::ServerDefault,
            &app.qh,
        );
        window.set_title("Example Window");
        window.set_app_id("io.github.ciantic.wayapp.ExampleWindow");
        window.commit();
        let surface_state = EguiSurfaceState::new(app, window, 600, 400);
        Self {
            surface_state,
            pool,
        }
    }

    pub fn handle_events(&mut self, app: &mut Application, events: &[WaylandEvent]) {
        self.surface_state.handle_events(app, events, &mut |_ctx| {
            // ctx.ui().label("GUI Stats Placeholder")
        });
    }
}
