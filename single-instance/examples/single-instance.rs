use single_instance::*;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bus_name = "org.example.MyApp";

    match only_single_instance(bus_name, || {
        println!("\n⚡ Activation signal received from secondary instance!");
        println!("   (This is where you could bring your window to front, etc.)");
    }) {
        Ok(_) => {
            println!("✓ This is the primary instance");
            println!("  Press Ctrl+C to exit.\n");
            std::thread::sleep(Duration::from_secs(99999999));
        }
        Err(Error::AlreadyRunning) => {
            println!("✗ Another instance is already running");
            println!("  Signaling the primary instance...");
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            return Err(e.into());
        }
    }

    Ok(())
}
