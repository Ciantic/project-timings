use crate::Error;
use crate::Timing;
use crate::TimingsMutations;
use crate::TimingsQueries;
use crate::Totals;
use crate::TotalsCache;
use crate::api::TimingsRecording;
use chrono::DateTime;
use chrono::Duration;
use chrono::NaiveDate;
use chrono::Utc;
use std::collections::HashMap;

// This implementation exists in older TypeScript codebase:
// https://github.com/Ciantic/winvd-monitoring/blob/b9e27d84a8412b0e97285f0dd869f56a57b3df4b/ui/TimingRecorder.ts#L14

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct CurrentTiming {
    pub start: DateTime<Utc>,
    pub project: String,
    pub client: String,
}

pub struct TimingsRecorder {
    unwritten_timings: Vec<Timing>,
    current_timing: Option<CurrentTiming>,
    last_keep_alive: Option<DateTime<Utc>>,
    minimum_timing: Duration,
    totals_cache: TotalsCache,
    summary_cache: HashMap<(NaiveDate, String, String), String>,
    running_changed: Option<Box<dyn Fn(bool) + Send + Sync>>,
}

impl TimingsRecorder {
    pub fn new(minimum_timing: Duration) -> Self {
        let min = if minimum_timing < Duration::zero() {
            Duration::zero()
        } else {
            minimum_timing
        };
        TimingsRecorder {
            unwritten_timings: Vec::new(),
            current_timing: None,
            last_keep_alive: None,
            minimum_timing: min,
            totals_cache: TotalsCache::new(),
            summary_cache: HashMap::new(),
            running_changed: None,
        }
    }

    pub fn set_running_changed_callback<F>(&mut self, callback: F)
    where
        F: Fn(bool) + Send + Sync + 'static,
    {
        self.running_changed = Some(Box::new(callback));
    }

    /// Get totals for a client/project, either from cache or by calculating
    /// from database.
    pub async fn get_totals<T: TimingsQueries + TimingsMutations>(
        &mut self,
        client: &str,
        project: &str,
        now: DateTime<Utc>,
        conn: &mut T,
    ) -> Result<Totals, Error> {
        let current_timing_start = if !self.totals_cache.has_cached_totals(client, project) {
            // Writing timings before getting totals to ensure up-to-date data for uncached
            // totals. `write_timings` writes the current timing as well thus it should be
            // empty.
            self.write_timings(conn, now).await?;
            None
        } else {
            self.current_timing.as_ref().and_then(|ct| {
                if ct.client == client && ct.project == project {
                    Some(ct.start)
                } else {
                    None
                }
            })
        };

        self.totals_cache
            .get_totals(client, project, now, conn, current_timing_start)
            .await
    }

    pub fn get_summary_if_cached(
        &self,
        day: NaiveDate,
        client: &str,
        project: &str,
    ) -> Option<String> {
        self.summary_cache
            .get(&(day, client.to_string(), project.to_string()))
            .cloned()
    }

    pub async fn get_summary<T: TimingsQueries + TimingsMutations>(
        &mut self,
        day: NaiveDate,
        client: &str,
        project: &str,
        now: DateTime<Utc>,
        conn: &mut T,
    ) -> Result<String, Error> {
        if let Some(cached) =
            self.summary_cache
                .get(&(day, client.to_string(), project.to_string()))
        {
            return Ok(cached.clone());
        }

        // Ensure timings are written before fetching summary
        self.write_timings(conn, now).await?;

        let summaries = conn
            .get_timings_daily_summaries(
                Utc,
                day,
                day,
                Some(client.to_string()),
                Some(project.to_string()),
            )
            .await?;

        if let Some(summary) = summaries.into_iter().next() {
            self.summary_cache.insert(
                (day, client.to_string(), project.to_string()),
                summary.summary.clone(),
            );
            Ok(summary.summary)
        } else {
            Ok(String::new())
        }
    }

    fn add_timing(&mut self, timing: Timing) {
        let duration = timing.end - timing.start;

        if duration < self.minimum_timing {
            log::info!(
                "Timing too short ({}s < {}s), ignoring timing: {:?} - {:?}",
                duration.num_seconds(),
                self.minimum_timing.num_seconds(),
                timing.start,
                timing.end
            );
            return;
        }

        if duration.num_seconds() > 0 {
            log::trace!("Adding timing: {:?}", timing);
            self.totals_cache.add_timing(timing.clone());
            self.unwritten_timings.push(timing);
        } else {
            log::warn!(
                "Timing is empty or timing end time {:?} is before start time {:?}, ignoring \
                 timing",
                timing.end,
                timing.start
            );
        }
    }

    fn finalize_current_timing(&mut self, now: DateTime<Utc>) {
        // Finalize the current timing without touching keep-alive state. The caller
        // is responsible for calling `keep_alive_timing` if needed.
        if let Some(current) = self.current_timing.take() {
            self.add_timing(Timing {
                client: current.client.clone(),
                project: current.project.clone(),
                start: current.start,
                end: now,
            });
        }
    }
}

impl TimingsRecording for TimingsRecorder {
    fn is_running(&self) -> bool {
        self.current_timing.is_some()
    }

    fn start_timing(&mut self, client: String, project: String, now: DateTime<Utc>) -> bool {
        let client = client.trim();
        let project = project.trim();
        log::trace!(
            "Starting timing for client={}, project={} at {:?}",
            client,
            project,
            now
        );

        self.keep_alive_timing(now);
        if (client == "") || (project == "") {
            log::warn!(
                "Client or Project is empty (client='{}', project='{}'), not starting timing",
                client,
                project
            );
            // Stop current timing
            self.stop_timing(now);
            return false;
        }

        // If client and project matches current timing, do nothing
        if let Some(current) = &self.current_timing {
            // If same client and project, do nothing, other wise stop current timing
            if current.client == client && current.project == project {
                return false;
            }
        }
        self.finalize_current_timing(now);

        self.current_timing = Some(CurrentTiming {
            client: client.to_string(),
            project: project.to_string(),
            start: now,
        });
        if let Some(callback) = &self.running_changed {
            callback(true);
        }
        return true;
    }

    fn stop_timing(&mut self, now: DateTime<Utc>) -> () {
        log::trace!("Stopping timing at {:?}", now);

        self.keep_alive_timing(now);
        self.finalize_current_timing(now);
        if let Some(callback) = &self.running_changed {
            callback(false);
        }
    }

    fn keep_alive_timing(&mut self, now: DateTime<Utc>) -> () {
        if let Some(current) = &mut self.current_timing
            && let Some(last_keep_alive) = self.last_keep_alive
            && (now - last_keep_alive).num_seconds() > 60
        {
            log::warn!(
                "Keep alive didn't happen in time, last at {:?}, now {:?}",
                last_keep_alive,
                now
            );

            let timing = Timing {
                client: current.client.clone(),
                project: current.project.clone(),
                start: current.start,
                end: last_keep_alive,
            };
            current.start = now;

            self.add_timing(timing);
        }

        log::trace!("Keep alive at {:?}", now);

        self.last_keep_alive = Some(now);
    }

    async fn write_timings(
        &mut self,
        conn: &mut impl TimingsMutations,
        now: DateTime<Utc>,
    ) -> Result<(), Error> {
        let mut timings_to_write = self.unwritten_timings.clone();

        // Include current running timing if it exists and meets minimum duration
        if let Some(current) = &self.current_timing {
            let duration = now - current.start;

            if duration >= self.minimum_timing {
                timings_to_write.push(Timing {
                    client: current.client.clone(),
                    project: current.project.clone(),
                    start: current.start,
                    end: now,
                });
            }
        }

        log::trace!("Writing {} timings to database", timings_to_write.len());
        conn.insert_timings(&timings_to_write).await?;
        self.unwritten_timings.clear();
        Ok(())
    }
}
