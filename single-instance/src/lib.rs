use futures::executor::block_on;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread::JoinHandle;
use zbus::interface;
use zbus::Connection;

/// Errors that can occur when starting the single instance monitor
#[derive(Debug)]
pub enum Error {
    AlreadyRunning,
    DBus(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::AlreadyRunning => write!(f, "Another instance is already running"),
            Error::DBus(e) => write!(f, "D-Bus error: {}", e),
        }
    }
}

impl std::error::Error for Error {}

impl From<zbus::Error> for Error {
    fn from(e: zbus::Error) -> Self {
        Error::DBus(e.to_string())
    }
}

impl From<zbus::fdo::Error> for Error {
    fn from(e: zbus::fdo::Error) -> Self {
        Error::DBus(e.to_string())
    }
}

/// Make unique D-Bus compatible bus name from arbitrary string
fn sanitize_bus_name(input: &str) -> String {
    let mut hasher = DefaultHasher::new();
    input.hash(&mut hasher);
    let hash = hasher.finish();

    format!("org.example.SingleInstance{:x}", hash)
}

/// Runs the single instance checker
///
/// - `unique_name`: Unique name to identify the instance (e.g. database path)
/// - `callback`: The callback to invoke when a secondary instance tries to
///   start (this is used in primary instance only)
pub fn only_single_instance(
    unique_name: &str,
    callback: impl Fn() + Send + Sync + 'static,
) -> Result<JoinHandle<()>, Error> {
    let bus_name = sanitize_bus_name(unique_name);
    // First check if we can acquire the name
    let can_acquire = block_on(async {
        let connection = Connection::session().await?;
        let reply = zbus::fdo::DBusProxy::new(&connection)
            .await?
            .request_name(
                zbus::names::WellKnownName::from_string_unchecked(bus_name.clone()),
                zbus::fdo::RequestNameFlags::DoNotQueue.into(),
            )
            .await?;

        match reply {
            zbus::fdo::RequestNameReply::PrimaryOwner => {
                // Release the name so the thread can acquire it
                zbus::fdo::DBusProxy::new(&connection)
                    .await?
                    .release_name(zbus::names::WellKnownName::from_string_unchecked(
                        bus_name.clone(),
                    ))
                    .await?;
                Ok(true)
            }
            zbus::fdo::RequestNameReply::Exists => Ok(false),
            _ => Err(Error::DBus(
                "Unexpected reply when requesting name".to_string(),
            )),
        }
    })?;

    if !can_acquire {
        // Signal the primary instance
        signal_primary_instance(bus_name)?;
        return Err(Error::AlreadyRunning);
    }

    // Spawn the monitoring thread
    let handle = std::thread::spawn(move || {
        block_on(async {
            let connection = Connection::session().await.unwrap();

            // Acquire the D-Bus name (should succeed since we just checked)
            zbus::fdo::DBusProxy::new(&connection)
                .await
                .unwrap()
                .request_name(
                    zbus::names::WellKnownName::from_string_unchecked(bus_name.clone()),
                    zbus::fdo::RequestNameFlags::DoNotQueue.into(),
                )
                .await
                .unwrap();

            // Register the D-Bus service
            let service = SingleInstanceService {
                callback: Arc::new(Mutex::new(callback)),
            };

            connection
                .object_server()
                .at("/org/example/SingleInstance", service)
                .await
                .unwrap();

            // Keep the connection alive
            futures::future::pending::<()>().await;
        })
    });

    Ok(handle)
}

fn signal_primary_instance(bus_name: impl Into<String>) -> Result<(), Error> {
    let bus_name = bus_name.into();

    block_on(async {
        let connection = Connection::session().await?;

        let proxy = zbus::Proxy::new(
            &connection,
            bus_name,
            "/org/example/SingleInstance",
            "org.example.SingleInstance",
        )
        .await?;

        proxy.call_method("Activate", &()).await?;

        Ok(())
    })
}

struct SingleInstanceService {
    callback: Arc<Mutex<dyn Fn() + Send + Sync + 'static>>,
}

#[interface(name = "org.example.SingleInstance")]
impl SingleInstanceService {
    /// Called when a secondary instance tries to start
    fn activate(&self) {
        let callback = self.callback.lock().unwrap();
        callback();
    }
}
