use idle_monitor::run_idle_monitor;
use idle_monitor::IdleNotification;
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::time::Duration;

enum AppMessages {
    Something,
    IdleNotification(IdleNotification),
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Starting idle monitor example, avoid using your mouse and keyboard for 5 seconds...");

    // Create a channel for receiving idle notifications
    let (tx, rx) = channel::<AppMessages>();

    // Spawn the idle monitor in a background thread (5 second timeout)
    let monitor_thread = run_idle_monitor(
        move |i| {
            tx.send(AppMessages::IdleNotification(i)).unwrap();
        },
        Duration::from_secs(5),
    );

    // Listen for idle notifications
    for notification in rx {
        match notification {
            AppMessages::IdleNotification(IdleNotification::Idle) => {
                println!("💤 User idle detected!");
            }
            AppMessages::IdleNotification(IdleNotification::Resumed) => {
                println!("✅ User activity resumed!");
            }
            AppMessages::Something => {}
        }
    }

    // Wait for the monitor thread to finish
    match monitor_thread.join() {
        Ok(Ok(())) => {
            println!("Monitor completed successfully");
        }
        Ok(Err(e)) => {
            eprintln!("Monitor error: {}", e);
        }
        Err(_) => {
            eprintln!("Thread panic");
        }
    }

    Ok(())
}
