pub mod screen_saver;
pub mod virtual_desktop_manager;
use std::pin::Pin;

use futures::{FutureExt, Stream, StreamExt};
use zbus::Connection;

use crate::api::*;

#[derive(Debug, Clone)]
pub struct KDEVirtualDesktopController {
    connection: Connection,
}

impl KDEVirtualDesktopController {
    pub async fn new() -> Result<Self, Error> {
        let connection = Connection::session().await?;
        Ok(Self { connection })
    }
}

// async fn get_desktop_name_from_id(
//     proxy: &virtual_desktop_manager::VirtualDesktopManagerProxy<'_>,
//     id: &DesktopId,
// ) -> Result<String, Error> {
//     let desktops = proxy.desktops().await?;

//     // Find the name of the current desktop
//     for desktop in &desktops {
//         if desktop.1 == id.0 {
//             return Ok(desktop.2.clone());
//         }
//     }

//     Err(Error::DesktopNotFound(id.clone()))
// }

impl VirtualDesktopController for KDEVirtualDesktopController {
    async fn listen(&mut self) -> Result<impl Stream<Item = VirtualDesktopMessage>, Error> {
        let vdproxy =
            virtual_desktop_manager::VirtualDesktopManagerProxy::new(&self.connection).await?;
        let screen_saver_proxy = screen_saver::ScreenSaverProxy::new(&self.connection).await?;

        let current_changed_stream = vdproxy.receive_current_changed_method().await?;
        let desktop_data_changed_stream = vdproxy.receive_desktop_data_changed().await?;
        let active_changed_stream = screen_saver_proxy.receive_active_changed().await?;

        let desktop_change_stream = futures::stream::unfold(
            (current_changed_stream, vdproxy.clone()),
            |(mut stream, proxy)| async move {
                while let Some(msg) = stream.next().await {
                    if let Ok(args) = msg.args() {
                        return Some((
                            VirtualDesktopMessage::DesktopChange(DesktopId(args.id.to_string())),
                            (stream, proxy),
                        ));
                    }
                }
                None
            },
        );

        let desktop_name_changed_stream = futures::stream::unfold(
            (desktop_data_changed_stream, vdproxy),
            |(mut stream, proxy)| async move {
                while let Some(msg) = stream.next().await {
                    if let Ok(args) = msg.args() {
                        let desktop_data = args.desktop_data;
                        let id = desktop_data.1.to_string();
                        let name = desktop_data.2.to_string();
                        return Some((
                            VirtualDesktopMessage::DesktopNameChanged(DesktopId(id), name),
                            (stream, proxy),
                        ));
                    }
                }
                None
            },
        );

        let idle_active_stream =
            futures::stream::unfold((active_changed_stream), |mut stream| async move {
                while let Some(msg) = stream.next().await {
                    if let Ok(args) = msg.args() {
                        let message = if args.arg_1 {
                            VirtualDesktopMessage::ScreenSaverActive
                        } else {
                            VirtualDesktopMessage::ScreenSaveInactive
                        };
                        return Some((message, stream));
                    }
                }
                None
            });

        use futures::stream::select_all;
        let streams: Vec<Pin<Box<dyn Stream<Item = VirtualDesktopMessage> + Send>>> = vec![
            Box::pin(desktop_change_stream),
            Box::pin(desktop_name_changed_stream),
            Box::pin(idle_active_stream),
        ];
        let combined_stream = select_all(streams);

        Ok(Box::pin(combined_stream))
    }

    async fn update_desktop_name(&mut self, desktop_name: &str) -> Result<(), Error> {
        let proxy =
            virtual_desktop_manager::VirtualDesktopManagerProxy::new(&self.connection).await?;

        let current_id = proxy.current().await?;

        proxy.set_desktop_name(&current_id, &desktop_name).await?;

        Ok(())
    }

    async fn get_desktop_name(&self, desktop_id: &DesktopId) -> Result<String, Error> {
        let proxy =
            virtual_desktop_manager::VirtualDesktopManagerProxy::new(&self.connection).await?;

        let current_id = DesktopId(proxy.current().await?);
        let desktops = proxy.desktops().await?;

        // Find the name of the current desktop
        for desktop in &desktops {
            if desktop.1 == desktop_id.0 {
                return Ok(desktop.2.clone());
            }
        }

        Err(Error::DesktopNotFound(current_id))
    }

    async fn get_current_desktop(&self) -> Result<DesktopId, Error> {
        let proxy =
            virtual_desktop_manager::VirtualDesktopManagerProxy::new(&self.connection).await?;

        let current_id = proxy.current().await?;

        Ok(DesktopId(current_id))
    }
}
