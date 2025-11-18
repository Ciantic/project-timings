use futures::stream::StreamExt;
use zbus::{Connection, Result};

use virtual_desktops::{
    screen_saver::ScreenSaverProxy, virtual_desktop_manager::VirtualDesktopManagerProxy,
};

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
    let proxy = ScreenSaverProxy::new(&connection).await?;
    let is_active = proxy.get_active().await?;
    println!("Screen saver active: {}", is_active);
    let active_time = proxy.get_active_time().await?;
    println!("Screen saver active time: {}", active_time);

    let mut property_stream = proxy.receive_active_changed().await?;

    while let Some(msg) = property_stream.next().await {
        let is_active = msg.args()?.arg_1;
        println!("Screen saver active changed: {}", is_active);
    }
    println!("End of stream.");
    Ok(())
}
