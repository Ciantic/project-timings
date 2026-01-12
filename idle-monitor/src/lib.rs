use smithay_client_toolkit::reexports::client::protocol::wl_seat::WlSeat;
use smithay_client_toolkit::reexports::client::Connection;
use smithay_client_toolkit::reexports::client::Dispatch;
use smithay_client_toolkit::reexports::client::QueueHandle;
use std::thread::JoinHandle;
use std::time::Duration;
use wayland_client::protocol::wl_registry;
use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notification_v1::ExtIdleNotificationV1;
use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notifier_v1::ExtIdleNotifierV1;

pub enum IdleNotification {
    Idle,
    Resumed,
}

pub fn run_idle_monitor(
    callback: impl Fn(IdleNotification) + Send + Sync + 'static,
    timeout: Duration,
) -> JoinHandle<Result<(), Box<dyn std::error::Error + Send + Sync>>> {
    std::thread::spawn(move || {
        let conn = Connection::connect_to_env()?;
        let mut event_queue = conn.new_event_queue();
        let qh = event_queue.handle();

        let _registry = conn.display().get_registry(&qh, ());

        let mut state = IdleMonitorState {
            idle_notifier: None,
            seat: None,
            idle_notification: None,
            callback: Box::new(callback),
            timeout,
        };

        // Main event loop
        loop {
            event_queue.blocking_dispatch(&mut state)?;
        }
    })
}

struct IdleMonitorState {
    idle_notifier: Option<ExtIdleNotifierV1>,
    seat: Option<WlSeat>,
    idle_notification: Option<ExtIdleNotificationV1>,
    callback: Box<dyn Fn(IdleNotification) + Send + Sync + 'static>,
    timeout: Duration,
}

impl Dispatch<wl_registry::WlRegistry, ()> for IdleMonitorState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            if interface == "wl_seat" {
                let seat = registry.bind::<WlSeat, _, _>(name, version, qh, ());
                state.seat = Some(seat);
            } else if interface == "ext_idle_notifier_v1" {
                let notifier = registry.bind::<ExtIdleNotifierV1, _, _>(name, version, qh, ());
                state.idle_notifier = Some(notifier);
            }
            // If we have both notifier and seat available, create a notification object
            if state.idle_notification.is_none() {
                if let (Some(notifier), Some(seat)) =
                    (state.idle_notifier.as_ref(), state.seat.as_ref())
                {
                    let timeout_ms = state.timeout.as_millis().min(u32::MAX as u128) as u32;
                    let notification =
                        notifier.get_input_idle_notification(timeout_ms, seat, qh, ());
                    state.idle_notification = Some(notification);
                }
            }
        }
    }
}

impl Dispatch<WlSeat, ()> for IdleMonitorState {
    fn event(
        _state: &mut Self,
        _proxy: &WlSeat,
        _event: <WlSeat as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtIdleNotifierV1, ()> for IdleMonitorState {
    fn event(
        _state: &mut Self,
        _proxy: &ExtIdleNotifierV1,
        _event: <ExtIdleNotifierV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtIdleNotificationV1, ()> for IdleMonitorState {
    fn event(
        state: &mut Self,
        _proxy: &ExtIdleNotificationV1,
        event: <ExtIdleNotificationV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notification_v1::Event;
        match event {
            Event::Idled => {
                (state.callback)(IdleNotification::Idle);
            }
            Event::Resumed => {
                (state.callback)(IdleNotification::Resumed);
            }
            _ => {}
        }
    }
}
