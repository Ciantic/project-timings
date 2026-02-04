use crate::Error;
use crate::Timing;
use crate::TimingsQueries;
use chrono::DateTime;
use chrono::Datelike;
use chrono::Duration;
use chrono::NaiveDate;
use chrono::Utc;
use sqlx::Sqlite;
use sqlx::pool::PoolConnection;
use std::collections::HashMap;
use std::ops::Add;

pub struct DailyTotals(HashMap<NaiveDate, Duration>);

impl DailyTotals {
    pub fn new() -> Self {
        DailyTotals(HashMap::new())
    }

    pub fn get(&self, date: &NaiveDate) -> Option<&Duration> {
        self.0.get(date)
    }

    pub fn insert(&mut self, date: NaiveDate, duration: Duration) {
        self.0.insert(date, duration);
    }

    pub fn insert_timing(&mut self, start: &DateTime<Utc>, end: &DateTime<Utc>) {
        let (date, duration) = {
            let local_start = start.with_timezone(&chrono::Local);
            let local_end = end.with_timezone(&chrono::Local);
            let date = local_start.date_naive();
            let duration = local_end - local_start;
            (date, duration)
        };
        let entry = self.0.entry(date).or_insert_with(|| Duration::zero());
        *entry = *entry + duration;
    }

    pub async fn from_database(
        conn: &mut PoolConnection<Sqlite>,
        client: &str,
        project: &str,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Self, Error> {
        let mut daily_totals_map: HashMap<NaiveDate, Duration> = HashMap::new();
        let daily_totals = conn
            .get_timings_daily_totals(
                from,
                to,
                Some(client.to_string()),
                Some(project.to_string()),
            )
            .await?;
        for daily_total in daily_totals {
            daily_totals_map.insert(
                daily_total.day,
                Duration::milliseconds((daily_total.hours * 3600.0 * 1000.0) as i64),
            );
        }
        Ok(DailyTotals(daily_totals_map))
    }

    pub fn from_timings(timings: &[(DateTime<Utc>, DateTime<Utc>)]) -> Self {
        let mut daily_totals = DailyTotals::new();
        for (start, end) in timings {
            daily_totals.insert_timing(start, end);
        }
        daily_totals
    }

    pub fn to_totals(&self, now: DateTime<Utc>) -> Totals {
        // Calculate totals for day, this week, last week, and eight weeks
        //
        // Note, we must assume local timezone for daily totals, as well as for week
        // calculations

        // Convert now to local date for calculations
        let today = now.with_timezone(&chrono::Local).date_naive();

        // Calculate day total (today)
        let day = self
            .0
            .get(&today)
            .copied()
            .unwrap_or_else(|| Duration::zero());

        // Calculate week boundaries (assuming weeks start on Monday)
        let days_from_monday = today.weekday().num_days_from_monday();
        let this_week_start = today - Duration::days(days_from_monday as i64);
        let last_week_start = this_week_start - Duration::days(7);
        let last_week_end = this_week_start - Duration::days(1);
        let eight_weeks_start = today - Duration::weeks(8);

        // Calculate this week total (from Monday to today)
        let mut this_week_total = Duration::zero();
        let mut current_date = this_week_start;
        while current_date <= today {
            if let Some(duration) = self.get(&current_date) {
                this_week_total = this_week_total + *duration;
            }
            current_date = current_date + Duration::days(1);
        }

        // Calculate last week total (full week, Monday to Sunday)
        let mut last_week_total = Duration::zero();
        let mut current_date = last_week_start;
        while current_date <= last_week_end {
            if let Some(duration) = self.get(&current_date) {
                last_week_total = last_week_total + *duration;
            }
            current_date = current_date + Duration::days(1);
        }

        // Calculate eight weeks total (last 8 weeks including today)
        let mut eight_weeks_total = Duration::zero();
        let mut current_date = eight_weeks_start;
        while current_date <= today {
            if let Some(duration) = self.get(&current_date) {
                eight_weeks_total = eight_weeks_total + *duration;
            }
            current_date = current_date + Duration::days(1);
        }

        Totals {
            today: day,
            this_week: this_week_total,
            last_week: last_week_total,
            eight_weeks: eight_weeks_total,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Totals {
    pub today: Duration,
    pub this_week: Duration,
    pub last_week: Duration,
    pub eight_weeks: Duration,
}

impl Totals {
    pub fn with_current_timing(&self, start: DateTime<Utc>, now: DateTime<Utc>) -> Totals {
        let duration = now - start;
        Totals {
            today: self.today + duration,
            this_week: self.this_week + duration,
            last_week: self.last_week,
            eight_weeks: self.eight_weeks + duration,
        }
    }
}

impl Add for Totals {
    type Output = Totals;

    fn add(self, other: Totals) -> Totals {
        Totals {
            today: self.today + other.today,
            this_week: self.this_week + other.this_week,
            last_week: self.last_week + other.last_week,
            eight_weeks: self.eight_weeks + other.eight_weeks,
        }
    }
}

pub(crate) struct TotalsCache {
    // Key: (client, project) -> Daily totals (NaiveDate = Local date)
    totals: HashMap<(String, String), DailyTotals>,
}

impl TotalsCache {
    pub fn new() -> Self {
        TotalsCache {
            totals: HashMap::new(),
        }
    }

    /// Add a timing to the cache and update cached totals
    pub fn add_timing(&mut self, timing: Timing) {
        // Add to existing totals only
        if let Some(totals) = self
            .totals
            .get_mut(&(timing.client.clone(), timing.project.clone()))
        {
            totals.insert_timing(&timing.start, &timing.end);
        }

        // Do nothing if no existing totals
    }

    pub fn has_cached_totals(&self, client: &str, project: &str) -> bool {
        self.totals
            .contains_key(&(client.to_string(), project.to_string()))
    }

    pub async fn get_totals(
        &mut self,
        client: &str,
        project: &str,
        now: DateTime<Utc>,
        conn: &mut PoolConnection<Sqlite>,
        current_timing_start: Option<DateTime<Utc>>,
    ) -> Result<Totals, Error> {
        let totals = match self.totals.get(&(client.to_string(), project.to_string())) {
            // 1. Get cached totals if available
            Some(totals) => totals.to_totals(now),
            // 2. Calculate totals from database, and cache them
            None => {
                let daily_totals = DailyTotals::from_database(
                    conn,
                    client,
                    project,
                    now - Duration::weeks(8),
                    now,
                )
                .await?;

                let totals = daily_totals.to_totals(now);

                // Cache the daily totals
                self.totals
                    .insert((client.to_string(), project.to_string()), daily_totals);

                totals
            }
        };

        // Include current timing if any
        match current_timing_start {
            Some(start) => Ok(totals.with_current_timing(start, now)),
            None => Ok(totals),
        }
    }
}
