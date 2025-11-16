use futures::stream::StreamExt;
use virtual_desktops::*;

// Although we use `tokio` here, you can use any async runtime of choice.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a new KDE Virtual Desktop Controller
    let mut controller = KDEVirtualDesktopController::new().await?;
    println!("KDEVDController created successfully.");

    // Get the current desktop ID
    let current_desktop = controller.get_current_desktop().await?;

    // Get the current desktop name
    let current_name = controller.get_desktop_name(&current_desktop).await?;
    println!("Current desktop name: {}", current_name);

    // Update the desktop name
    let new_name = format!("{}-updated", current_name);
    println!("Updating desktop name to: {}", new_name);
    // controller.update_desktop_name(new_name.clone()).await?;
    // println!("Desktop name updated successfully.");

    // // Verify the name was updated
    // let updated_name = controller.get_desktop_name().await?;
    // println!("Verified desktop name: {}", updated_name);

    // Listen for desktop changes
    println!("\nListening for desktop changes...");
    println!("Switch to a different virtual desktop to see the change event.");

    let query_controller = controller.clone();
    let mut stream = controller.listen().await?;

    while let Some(msg) = stream.next().await {
        match msg {
            VirtualDesktopMessage::DesktopNameChanged(id, name) => {
                println!("Desktop name changed! New name: {} {}", id, name);
            }
            VirtualDesktopMessage::DesktopChange(id) => {
                let name = query_controller.get_desktop_name(&id).await?;
                println!("Desktop changed! New desktop ID: {} with name {}", id, name);
            }
            VirtualDesktopMessage::Idle => {
                println!("System went idle");
            }
            VirtualDesktopMessage::Active => {
                println!("System became active");
            }
        }
    }

    println!("Stream ended.");
    Ok(())
}
