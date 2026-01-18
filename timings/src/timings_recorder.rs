use crate::Error;
use crate::Timing;
use crate::TimingsMutations;
use crate::TimingsQueries;
use crate::Totals;
use crate::TotalsCache;
use crate::api::TimingsRecording;
use crate::totals_cache;
use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;

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
        }
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
        if !self.totals_cache.has_cached_totals(client, project) {
            // Writing timings before getting totals to ensure up-to-date data for uncached
            // totals
            self.write_timings(conn, now).await?;
        }

        let current_timing_start = self.current_timing.as_ref().and_then(|ct| {
            if ct.client == client && ct.project == project {
                Some(ct.start)
            } else {
                None
            }
        });

        self.totals_cache
            .get_totals(client, project, now, conn, current_timing_start)
            .await
    }

    fn add_timing(&mut self, timing: Timing, now: DateTime<Utc>) {
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
            self.totals_cache.add_timing(timing.clone(), now);
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
}

impl TimingsRecording for TimingsRecorder {
    fn start_timing(&mut self, client: String, project: String, now: DateTime<Utc>) -> () {
        log::trace!(
            "Starting timing for client={}, project={} at {:?}",
            client,
            project,
            now
        );

        self.keep_alive_timing(now);

        // If client and project matches current timing, do nothing
        if let Some(current) = &self.current_timing {
            // There is already a timing going on, should we raise error? Old implementation
            // threw an error

            // log::warn!(
            //     "There is already a timing going on: {:?}, requested: client={},
            // project={}",     current,
            //     client,
            //     project
            // );

            // If same client and project, do nothing, other wise stop current timing
            if current.client == client && current.project == project {
                return ();
            } else {
                // Stop current timing
                self.stop_timing(now);
            }
        }

        self.current_timing = Some(CurrentTiming {
            client,
            project,
            start: now,
        });
    }

    fn stop_timing(&mut self, now: DateTime<Utc>) -> () {
        log::trace!("Stopping timing at {:?}", now);

        self.keep_alive_timing(now);

        // If there is a current timing, finalize it
        if let Some(current) = &self.current_timing {
            self.add_timing(
                Timing {
                    client: current.client.clone(),
                    project: current.project.clone(),
                    start: current.start,
                    end: now,
                },
                now,
            );
            self.current_timing = None;
        } else {
            // Old implementation threw an error here
            log::warn!("No current timing to stop at {:?}", now);
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

            self.add_timing(timing, now);
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
