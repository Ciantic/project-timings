use crate::Error;
use chrono::DateTime;
use chrono::NaiveDate;
use chrono::TimeZone;
use chrono::Utc;

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Timing {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub project: String,
    pub client: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GetTimingsFilters {
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub client: Option<String>,
    pub project: Option<String>,
}

pub struct DailyTotalSummary {
    pub day: NaiveDate,
    pub hours: f64,
    pub client: String,
    pub project: String,
}

#[derive(Debug, Clone)]
pub struct SummaryForDay {
    pub day: NaiveDate,
    pub project: String,
    pub client: String,
    pub summary: String,
    pub archived: bool,
}

/// Trait for querying timings database.
///
/// This is implemented for &mut SqliteConnection in
/// repository/timings_queries.rs
pub trait TimingsQueries {
    async fn get_timings(
        &mut self,
        filters: Option<GetTimingsFilters>,
    ) -> Result<Vec<Timing>, Error>;

    async fn get_timings_daily_totals(
        &mut self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        client: Option<String>,
        project: Option<String>,
    ) -> Result<Vec<DailyTotalSummary>, Error>;

    async fn get_timings_daily_summaries(
        &mut self,
        timezone: impl TimeZone,
        from: NaiveDate,
        to: NaiveDate,
        client: Option<String>,
        project: Option<String>,
    ) -> Result<Vec<SummaryForDay>, Error>;
}

/// Trait for mutating timings database.
///
/// This is implemented for &mut SqliteConnection in
/// repository/timings_mutations.rs
pub trait TimingsMutations {
    async fn create_timings_database(&mut self) -> Result<(), Error>;

    async fn insert_timings(
        &mut self,
        timings: impl IntoIterator<Item = &Timing>,
    ) -> Result<(), Error>;

    async fn insert_timings_daily_summaries(
        &mut self,
        timezone: impl TimeZone,
        summaries: impl IntoIterator<Item = &SummaryForDay>,
    ) -> Result<(), Error>;
}

pub trait TimingsRecording {
    /// Starts a new timing for the given client and project at the given time.
    fn start_timing(&mut self, client: String, project: String, now: DateTime<Utc>) -> ();

    /// Stops the current timing at the given time.
    fn stop_timing(&mut self, now: DateTime<Utc>) -> ();

    /// Keeps the current timing alive by updating its end time to now.
    ///
    /// Must be called at least once a minute, if there is gap lasting longer
    /// than a minute, the timing will be considered stopped at the time of
    /// the last keep-alive call.
    ///
    /// This ensures that for instance system sleep or hibernation does not
    /// cause excessively long timings.
    fn keep_alive_timing(&mut self, now: DateTime<Utc>) -> ();

    /// Flushes unwritten timings to the database.
    async fn write_timings(&mut self, conn: &mut impl TimingsMutations) -> Result<(), Error>;
}
