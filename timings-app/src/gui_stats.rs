use crate::AppMessage;
use crate::TimingsApp;
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
    surface_state: Option<EguiSurfaceState<Window>>,
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
        let surface_state = Some(EguiSurfaceState::new(app, window, 600, 400));
        Self {
            surface_state,
            pool,
        }
    }

    pub async fn handle_app_events(
        &mut self,
        parent: &mut TimingsApp,
        _app: &mut Application,
        event: &AppMessage,
    ) -> () {
        match event {
            // AppMessage::GuiStatsEvent(GuiStatsEvents::Close) => {
            //     parent.close_gui_stats();
            // }
            _ => {}
        }
    }

    pub async fn handle_wayland_events(
        &mut self,
        parent: &mut TimingsApp,
        app: &mut Application,
        events: &[WaylandEvent],
    ) -> () {
        if let Some(surface_state) = &mut self.surface_state {
            surface_state.handle_events(app, events, &mut |ctx| ());
        }
    }
}
