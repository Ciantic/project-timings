use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;

static DEBOUNCERS: OnceLock<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>> = OnceLock::new();

/// Schedules a callback to run after `delay`. If another call with the same
/// `id` is made before the delay elapses, the previous scheduled callback is
/// aborted.
fn run_debounced_task<F, S>(id: S, delay: Duration, callback: F)
where
    F: FnOnce() -> tokio::task::JoinHandle<()> + Send + 'static,
    S: Into<String>,
{
    let id = id.into();

    let map_mutex = DEBOUNCERS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = map_mutex.lock().unwrap();

    if let Some(handle) = map.remove(&id) {
        handle.abort();
    }

    let handle = tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        callback();
    });

    map.insert(id, handle);
}

/// Schedules a future to run after `delay`. If another call with the same
/// `id` is made before the delay elapses, the previous scheduled future is
/// aborted.
pub fn run_debounced_spawn<Fut, S>(id: S, delay: Duration, fut: Fut)
where
    Fut: std::future::Future<Output = ()> + Send + 'static,
    S: Into<String>,
{
    let id = id.into();
    run_debounced_task(id, delay, move || tokio::spawn(fut));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use tokio::time::Duration;
    use tokio::time::sleep;

    #[tokio::test]
    async fn run_debounced_task_example() {
        let counter = Arc::new(AtomicUsize::new(0));

        let c1 = counter.clone();
        run_debounced_task("debounce-task", Duration::from_millis(200), move || {
            let c = c1.clone();
            tokio::spawn(async move {
                c.fetch_add(1, Ordering::SeqCst);
            })
        });

        // Call again quickly; should cancel the first
        let c2 = counter.clone();
        run_debounced_task("debounce-task", Duration::from_millis(100), move || {
            let c = c2.clone();
            tokio::spawn(async move {
                c.fetch_add(1, Ordering::SeqCst);
            })
        });

        // Wait enough time for last scheduled task to run
        sleep(Duration::from_millis(300)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn run_debounced_spawn_example() {
        let counter = Arc::new(AtomicUsize::new(0));

        let c1 = counter.clone();
        run_debounced_spawn("debounce-spawn", Duration::from_millis(200), async move {
            c1.fetch_add(1, Ordering::SeqCst);
        });

        let c2 = counter.clone();
        run_debounced_spawn("debounce-spawn", Duration::from_millis(100), async move {
            c2.fetch_add(1, Ordering::SeqCst);
        });

        sleep(Duration::from_millis(300)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
}
