use crate::Error;
use crate::TimingsMutations;
use crate::TimingsQueries;
use crate::TimingsRecorder;
use crate::Totals;
use crate::api::TimingsRecording;
use chrono::DateTime;
use chrono::NaiveDate;
use chrono::Utc;
use sqlx::SqliteConnection;
use std::sync::Arc;
use std::sync::Mutex;

#[derive(Clone)]
pub struct TimingsRecorderShared {
    pub recorder: Arc<Mutex<TimingsRecorder>>,
}

unsafe impl Send for TimingsRecorderShared {}
unsafe impl Sync for TimingsRecorderShared {}

impl TimingsRecorderShared {
    pub fn new(minimum_timing: chrono::Duration) -> Self {
        TimingsRecorderShared {
            recorder: Arc::new(Mutex::new(TimingsRecorder::new(minimum_timing))),
        }
    }

    pub fn is_running(&self) -> bool {
        self.recorder.lock().unwrap().is_running()
    }

    pub async fn get_totals<T: TimingsMutations + TimingsQueries>(
        &self,
        client: &str,
        project: &str,
        now: DateTime<Utc>,
        conn: &mut T,
    ) -> Result<Totals, Error> {
        let mut guard = self.recorder.lock().unwrap();
        let totals = guard.get_totals(client, project, now, conn).await;
        drop(guard);
        totals
    }

    pub fn set_running_changed_callback<F>(&self, callback: F)
    where
        F: Fn(bool) + Send + Sync + 'static,
    {
        self.recorder
            .lock()
            .unwrap()
            .set_running_changed_callback(callback)
    }

    pub fn get_summary_if_cached(
        &self,
        day: NaiveDate,
        client: &str,
        project: &str,
    ) -> Option<String> {
        self.recorder
            .lock()
            .unwrap()
            .get_summary_if_cached(day, client, project)
    }

    pub async fn get_summary(
        &mut self,
        day: NaiveDate,
        client: &str,
        project: &str,
        now: DateTime<Utc>,
        conn: &mut SqliteConnection,
    ) -> Result<String, Error> {
        self.recorder
            .lock()
            .unwrap()
            .get_summary(day, client, project, now, conn)
            .await
    }
}

impl TimingsRecording for TimingsRecorderShared {
    fn is_running(&self) -> bool {
        self.is_running()
    }

    fn start_timing(&mut self, client: String, project: String, now: DateTime<Utc>) -> bool {
        self.recorder
            .lock()
            .unwrap()
            .start_timing(client, project, now)
    }

    fn stop_timing(&mut self, now: DateTime<Utc>) {
        self.recorder.lock().unwrap().stop_timing(now)
    }

    fn keep_alive_timing(&mut self, now: DateTime<Utc>) {
        self.recorder.lock().unwrap().keep_alive_timing(now)
    }

    async fn write_timings(
        &mut self,
        conn: &mut impl TimingsMutations,
        now: DateTime<Utc>,
    ) -> Result<(), Error> {
        self.recorder.lock().unwrap().write_timings(conn, now).await
    }
}
