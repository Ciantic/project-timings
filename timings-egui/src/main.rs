#![deny(clippy::all)]
#![forbid(unsafe_code)]

use chrono::{DateTime, Utc};
use eframe::egui::{self, Align, Margin};
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;
use tokio::runtime::Runtime;

struct ProjectTimingsEgui {
    // Sender/Receiver for async notifications.
    tx: Sender<u32>,
    rx: Receiver<u32>,

    client: String,
    project: String,
    start: Option<DateTime<Utc>>,
}

fn main() {
    // Run tokio runtime
    let rt = Runtime::new().unwrap();
    let _enter = rt.enter();
    std::thread::spawn(move || {
        rt.block_on(async {
            tokio::time::sleep(Duration::MAX).await;
        })
    });

    // Run the GUI in the main thread.
    let _ = eframe::run_native(
        "Hello egui + tokio",
        eframe::NativeOptions::default(),
        Box::new(|_cc| Ok(Box::new(ProjectTimingsEgui::default()))),
    );
}

impl Default for ProjectTimingsEgui {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();

        Self {
            tx,
            rx,
            client: String::new(),
            project: String::new(),
            start: Some(Utc::now()),
        }
    }
}

impl eframe::App for ProjectTimingsEgui {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Ensure the UI repaints at least once per second so the elapsed duration updates.
        ctx.request_repaint_after(Duration::from_secs(1));

        // Update the counter with the async response.
        // if let Ok(incr) = self.rx.try_recv() {
        //     self.count += incr;
        // }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label("Press the button to initiate an HTTP request.");
            ui.label("If successful, the count will increase by the following value.");
            // Show client and project names as text inputs.
            let client_response = ui
                .add(
                    egui::TextEdit::singleline(&mut self.client)
                        .hint_text("Client")
                        .margin(Margin::symmetric(10, 10))
                        .horizontal_align(Align::Center)
                        .vertical_align(Align::Center),
                )
                .labelled_by(ui.id().with("Client"));
            let project_response = ui
                .add(
                    egui::TextEdit::singleline(&mut self.project)
                        .hint_text("Project")
                        .margin(Margin::symmetric(10, 10))
                        .horizontal_align(Align::Center)
                        .vertical_align(Align::Center),
                )
                .labelled_by(ui.id().with("Project"));

            let status_label = if client_response.has_focus() || project_response.has_focus() {
                "Paused"
            } else {
                "Playing"
            };
            ui.label(status_label);

            // Show duration as a label (not editable).
            let duration = if let Some(start_time) = self.start {
                let now = Utc::now();
                // Floor to seconds
                now.signed_duration_since(start_time).num_milliseconds() / 1000
            } else {
                0
            };
            ui.label(format!("Duration: {} seconds", duration));
            println!("Update called {}", duration);
            // if ui.button(format!("Count: {}", self.count)).clicked() {
            //     send_req(self.value, self.tx.clone(), ctx.clone());
            // }
        });
    }
}

fn send_req(incr: u32, tx: Sender<u32>, ctx: egui::Context) {
    tokio::spawn(async move {
        let _ = tx.send(123);
        ctx.request_repaint();
    });
}
