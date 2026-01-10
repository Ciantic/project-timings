use tray_icon::status_notifier_watcher::StatusNotifierWatcherProxy;
use zbus::interface;
use zbus::names::OwnedWellKnownName;
use zbus::object_server::SignalEmitter;

// Minimal in-process implementation of `org.kde.StatusNotifierItem` to register
struct MyStatusNotifierItem {
    id: String,
}

#[interface(name = "org.kde.StatusNotifierItem")]
impl MyStatusNotifierItem {
    /// Activate method
    pub fn activate(&self, _x: i32, _y: i32) -> zbus::fdo::Result<()> {
        println!("Activate called");
        Ok(())
    }

    /// ContextMenu method
    pub fn context_menu(&self, _x: i32, _y: i32) -> zbus::fdo::Result<()> {
        println!("ContextMenu called");
        Ok(())
    }

    /// ProvideXdgActivationToken method
    pub fn provide_xdg_activation_token(&self, _token: &str) -> zbus::fdo::Result<()> {
        println!("ProvideXdgActivationToken called");
        Ok(())
    }

    /// Scroll method
    pub fn scroll(&self, _delta: i32, _orientation: &str) -> zbus::fdo::Result<()> {
        println!("Scroll called");
        Ok(())
    }

    /// SecondaryActivate method
    pub fn secondary_activate(&self, _x: i32, _y: i32) -> zbus::fdo::Result<()> {
        println!("SecondaryActivate called");
        Ok(())
    }

    /// AttentionIconName property
    #[zbus(property)]
    pub fn attention_icon_name(&self) -> zbus::fdo::Result<String> {
        Ok(String::new())
    }

    /// AttentionIconPixmap property
    #[zbus(property)]
    pub fn attention_icon_pixmap(&self) -> zbus::fdo::Result<Vec<(i32, i32, Vec<u8>)>> {
        Ok(vec![])
    }

    /// AttentionMovieName property
    #[zbus(property)]
    pub fn attention_movie_name(&self) -> zbus::fdo::Result<String> {
        Ok(String::new())
    }

    /// Category property
    #[zbus(property)]
    pub fn category(&self) -> zbus::fdo::Result<String> {
        Ok(String::from("ApplicationStatus"))
    }

    /// IconName property
    #[zbus(property)]
    pub fn icon_name(&self) -> zbus::fdo::Result<String> {
        Ok(String::from("application-x-executable"))
    }

    /// IconPixmap property
    #[zbus(property)]
    pub fn icon_pixmap(&self) -> zbus::fdo::Result<Vec<(i32, i32, Vec<u8>)>> {
        Ok(vec![])
    }

    /// IconThemePath property
    #[zbus(property)]
    pub fn icon_theme_path(&self) -> zbus::fdo::Result<String> {
        Ok(String::new())
    }

    /// Id property
    #[zbus(property)]
    pub fn id(&self) -> zbus::fdo::Result<String> {
        Ok(self.id.clone())
    }

    /// ItemIsMenu property
    #[zbus(property)]
    pub fn item_is_menu(&self) -> zbus::fdo::Result<bool> {
        Ok(false)
    }

    /// Menu property
    #[zbus(property)]
    pub fn menu(&self) -> zbus::fdo::Result<zbus::zvariant::OwnedObjectPath> {
        Ok(
            zbus::zvariant::OwnedObjectPath::try_from("/MenuBar").map_err(|_| {
                zbus::fdo::Error::UnknownProperty("Failed to create object path".to_string())
            })?,
        )
    }

    /// OverlayIconName property
    #[zbus(property)]
    pub fn overlay_icon_name(&self) -> zbus::fdo::Result<String> {
        Ok(String::from("help-about"))
    }

    /// OverlayIconPixmap property
    #[zbus(property)]
    pub fn overlay_icon_pixmap(&self) -> zbus::fdo::Result<Vec<(i32, i32, Vec<u8>)>> {
        Ok(vec![])
    }

    /// Status property
    #[zbus(property)]
    pub fn status(&self) -> zbus::fdo::Result<String> {
        Ok(String::from("Active"))
    }

    /// Title property
    #[zbus(property)]
    pub fn title(&self) -> zbus::fdo::Result<String> {
        Ok(String::from("Example App"))
    }

    /// ToolTip property
    #[zbus(property)]
    #[allow(clippy::type_complexity)]
    pub fn tool_tip(
        &self,
    ) -> zbus::fdo::Result<(String, Vec<(i32, i32, Vec<u8>)>, String, String)> {
        Ok((
            String::from("Tooltip"),
            vec![],
            String::new(),
            String::new(),
        ))
    }

    /// WindowId property
    #[zbus(property)]
    pub fn window_id(&self) -> zbus::fdo::Result<i32> {
        Ok(0)
    }

    // Signals -----------------------------------------------------------

    /// NewAttentionIcon signal
    #[zbus(signal)]
    pub async fn new_attention_icon(ctxt: &SignalEmitter<'_>) -> zbus::Result<()>;

    /// NewIcon signal
    #[zbus(signal)]
    pub async fn new_icon(ctxt: &SignalEmitter<'_>) -> zbus::Result<()>;

    /// NewMenu signal
    #[zbus(signal)]
    pub async fn new_menu(ctxt: &SignalEmitter<'_>) -> zbus::Result<()>;

    /// NewOverlayIcon signal
    #[zbus(signal)]
    pub async fn new_overlay_icon(ctxt: &SignalEmitter<'_>) -> zbus::Result<()>;

    /// NewStatus signal
    #[zbus(signal)]
    pub async fn new_status(ctxt: &SignalEmitter<'_>, _status: &str) -> zbus::Result<()>;

    /// NewTitle signal
    #[zbus(signal)]
    pub async fn new_title(ctxt: &SignalEmitter<'_>) -> zbus::Result<()>;

    /// NewToolTip signal
    #[zbus(signal)]
    pub async fn new_tool_tip(ctxt: &SignalEmitter<'_>) -> zbus::Result<()>;
}

// Although we use `tokio` here, you can use any async runtime of choice.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Connect to the session D-Bus
    let connection = zbus::Connection::session().await?;

    // Own a unique well-known name for our item (common pattern used by apps)
    let unique_name = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());
    // Request the name on the bus
    let owned_name = OwnedWellKnownName::try_from(unique_name.clone())?;
    let _ = connection.request_name(owned_name).await?;

    // Export our object at the conventional path
    let item = MyStatusNotifierItem {
        id: unique_name.clone(),
    };
    connection
        .object_server()
        .at("/StatusNotifierItem", item)
        .await?;

    // Create the StatusNotifierWatcher proxy and register our item
    let proxy = StatusNotifierWatcherProxy::builder(&connection)
        .destination("org.kde.StatusNotifierWatcher")?
        .path("/StatusNotifierWatcher")?
        .build()
        .await?;

    // The watcher expects a string of the form "SERVICE/OBJECT_PATH"
    let service_path = format!("{}/StatusNotifierItem", unique_name);
    proxy.register_status_notifier_item(&service_path).await?;

    println!("Registered as: {}", service_path);

    // Keep the program alive to respond to D-Bus calls (ctrl-c to quit)
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }
}
