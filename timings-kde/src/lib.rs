use futures::{self, Stream, StreamExt};
use timings::Error;
pub mod virtual_desktop_manager;

pub enum VDMessage {
    Idle,
    Active,
    DesktopChange(String),
}

pub trait VDController {
    async fn listen(&mut self) -> Result<impl Stream<Item = VDMessage> + Unpin, Error>;

    /// Updates the name of the current virtual desktop.
    async fn update_desktop_name(&mut self, desktop_name: String) -> Result<(), Error>;

    /// Gets the name of the current virtual desktop.
    async fn get_desktop_name(&self) -> Result<String, Error>;
}

struct KDEVDController {}

impl VDController for KDEVDController {
    async fn listen(&mut self) -> Result<impl Stream<Item = VDMessage> + Unpin, Error> {
        Ok(futures::stream::iter(vec![]))
    }

    async fn update_desktop_name(&mut self, desktop_name: String) -> Result<(), timings::Error> {
        todo!()
    }

    async fn get_desktop_name(&self) -> Result<String, timings::Error> {
        todo!()
    }
}
