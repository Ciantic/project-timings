use std::fmt;

use futures::Stream;

#[derive(Debug)]
pub enum Error {
    SysError(String),
    DesktopNotFound(DesktopId),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::SysError(err) => write!(f, "Sys error: {}", err),
            Error::DesktopNotFound(id) => write!(f, "Desktop not found: {}", id),
        }
    }
}

impl std::error::Error for Error {}

impl From<zbus::Error> for Error {
    fn from(err: zbus::Error) -> Self {
        Error::SysError(format!("zbus error: {}", err))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DesktopId(pub(crate) String);

impl std::fmt::Display for DesktopId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VirtualDesktopMessage {
    ScreenSaveInactive,
    ScreenSaverActive,
    DesktopChange(DesktopId),
    DesktopNameChanged(DesktopId, String),
}

pub trait VirtualDesktopController {
    async fn listen(&mut self) -> Result<impl Stream<Item = VirtualDesktopMessage>, Error>;

    /// Updates the name of the current virtual desktop.
    async fn update_desktop_name(&mut self, desktop_name: &str) -> Result<(), Error>;

    /// Gets the name of the current virtual desktop.
    async fn get_desktop_name(&self, desktop_id: &DesktopId) -> Result<String, Error>;

    /// Gets the current virtual desktop ID.
    async fn get_current_desktop(&self) -> Result<DesktopId, Error>;

    /// Get list of all virtual desktop IDs.
    async fn get_desktops(&self) -> Result<Vec<(DesktopId, String)>, Error>;
}
