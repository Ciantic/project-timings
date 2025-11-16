use futures::stream::StreamExt;
use zbus::{Connection, Result};

use timings_kde::*;

// Although we use `tokio` here, you can use any async runtime of choice.
#[tokio::main]
async fn main() -> Result<()> {
    let connection = Connection::session().await?;
    println!("Connected to D-Bus session bus.");

    // connection
    //     .object_server()
    //     .at("/org/zbus/Listener", Listener)
    //     .await?;

    // connection.request_name("org.zbus.Listener").await?;

    // `proxy` macro creates `VirtualDesktopManagerProxy` based on `VirtualDesktopManager` trait.
    let proxy = virtual_desktop_manager::VirtualDesktopManagerProxy::new(&connection).await?;
    println!("Proxy to VirtualDesktopManager created.");
    let count = proxy.count().await?;
    println!("Current virtual desktop count: {}", count);
    let current = proxy.current().await?;
    println!("Current virtual desktop: {}", current);
    let reply = proxy.desktops().await?;
    println!("Current virtual desktops: {:?}", reply);

    // println!("Setting current virtual desktop to the first one.");
    // proxy.set_current(&reply[0].1).await?;

    let mut property_stream = proxy.receive_current_changed_method().await?;

    while let Some(msg) = property_stream.next().await {
        let id = msg.args()?.id;
        let desktops = proxy.desktops().await?;
        // Iterate through the desktops to find the name of the current desktop
        for desktop in &desktops {
            if desktop.1 == id {
                println!("Current desktop changed to ID: {}, Name: {}", id, desktop.1);
            }
        }
        println!("Desktops changed: {:?}", id);
    }
    println!("End of stream.");
    Ok(())
}
