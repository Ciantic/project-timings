use tray_icon::DbusMenu;
use tray_icon::StatusNotifierItemImpl;
use tray_icon::StatusNotifierWatcherProxy;
use zbus::names::OwnedWellKnownName;

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
    let item = StatusNotifierItemImpl {
        id: unique_name.clone(),
    };
    connection
        .object_server()
        .at("/StatusNotifierItem", item)
        .await?;

    // Register the dbus menu at the conventional path
    let menu = DbusMenu::new();
    connection.object_server().at("/MenuBar", menu).await?;

    // Create the StatusNotifierWatcher proxy and register our item
    let proxy = StatusNotifierWatcherProxy::builder(&connection)
        .destination("org.kde.StatusNotifierWatcher")?
        .path("/StatusNotifierWatcher")?
        .build()
        .await?;

    println!("Connected to StatusNotifierWatcher");

    // Check if there's a StatusNotifierHost registered
    match proxy.is_status_notifier_host_registered().await {
        Ok(registered) => println!("StatusNotifierHost registered: {}", registered),
        Err(e) => println!("Failed to check host registration: {:?}", e),
    }

    match proxy.register_status_notifier_item(&unique_name).await {
        Ok(_) => println!("Successfully registered as: {}", unique_name),
        Err(e) => println!("Failed to register: {:?}", e),
    }

    // Give it a moment for the watcher to process the registration
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Check if we're in the list
    match proxy.registered_status_notifier_items().await {
        Ok(items) => {
            println!("Registered items after wait: {:?}", items);
            if items.contains(&unique_name) {
                println!("✓ Our item is in the registered list!");
            } else {
                println!("✗ Our item is NOT in the registered list yet");
            }
        }
        Err(e) => println!("Failed to get registered items: {:?}", e),
    }

    // Get the object from the server and emit the NewIcon signal
    // This tells the tray host that our icon is ready
    if let Ok(obj) = connection
        .object_server()
        .interface::<_, StatusNotifierItemImpl>("/StatusNotifierItem")
        .await
    {
        println!("Emitting NewIcon signal to notify tray of icon availability");
        let emitter = obj.signal_emitter();
        if let Err(e) = StatusNotifierItemImpl::new_icon(&emitter).await {
            println!("Failed to emit NewIcon signal: {:?}", e);
        }
    }

    println!("Waiting for tray icon requests...");
    println!("Note: The tray must query the properties to display the icon");

    // Keep the program alive to respond to D-Bus calls (ctrl-c to quit)
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }
}
