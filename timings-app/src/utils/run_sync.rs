use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;

static UNIQUE_TASKS: OnceLock<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>> =
    OnceLock::new();

/// Runs a task uniquely identified by `id`. If a task with the same `id` is
/// already running, it will be aborted before starting the new one.
fn run_sync_task<F, S>(id: S, callback: F)
where
    F: FnOnce() -> tokio::task::JoinHandle<()>,
    S: Into<String>,
{
    let id = id.into();

    let map_mutex = UNIQUE_TASKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = map_mutex.lock().unwrap();

    if let Some(handle) = map.remove(&id) {
        handle.abort();
    }
    let handle = callback();
    map.insert(id, handle);
}

/// Runs a future uniquely identified by `id`. If a future with the same `id` is
/// already running, it will be aborted before starting the new one.
pub fn run_sync_spawn<Fut, S>(id: S, fut: Fut)
where
    Fut: std::future::Future<Output = ()> + Send + 'static,
    S: Into<String>,
{
    let id = id.into();
    run_sync_task(id, move || tokio::spawn(fut));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use tokio::time::Duration;
    use tokio::time::sleep;

    // Example test demonstrating how to use `run_sync_task`.
    // The first spawned task sleeps before incrementing the counter and
    // should be aborted by the second call, so only one increment happens.
    #[tokio::test]
    async fn run_sync_task_example() {
        let counter = Arc::new(AtomicUsize::new(0));

        // First task: will sleep and then increment (expected to be aborted)
        let c1 = counter.clone();
        run_sync_task("unique-task", move || {
            let c = c1.clone();
            tokio::spawn(async move {
                sleep(Duration::from_millis(200)).await;
                c.fetch_add(1, Ordering::SeqCst);
            })
        });

        // Second task: replaces the first and increments immediately
        let c2 = counter.clone();
        run_sync_task("unique-task", move || {
            let c = c2.clone();
            tokio::spawn(async move {
                c.fetch_add(1, Ordering::SeqCst);
            })
        });

        // Wait enough time for tasks to run if they were not aborted
        sleep(Duration::from_millis(300)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn run_sync_spawn_example() {
        let counter = Arc::new(AtomicUsize::new(0));

        // First spawned future: sleeps then increments (expected to be aborted)
        let c1 = counter.clone();
        run_sync_spawn("unique-task-spawn", async move {
            sleep(Duration::from_millis(200)).await;
            c1.fetch_add(1, Ordering::SeqCst);
        });

        // Second spawned future: replaces the first and increments immediately
        let c2 = counter.clone();
        run_sync_spawn("unique-task-spawn", async move {
            c2.fetch_add(1, Ordering::SeqCst);
        });

        sleep(Duration::from_millis(300)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
}
