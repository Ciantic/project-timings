//! Repository functions for timings
//!
//! Not to be used directly, use the traits in `timings.rs` instead.

use super::utils::datetime_to_ms;
use super::utils::ms_to_datetime;
use crate::DailyTotalSummary;
use crate::GetTimingsFilters;
use crate::SummaryForDay;
use crate::Timing;
use crate::TimingsQueries;
use crate::error::Error;
use chrono::NaiveDate;
use chrono::Utc;
use const_format::str_split;
use sqlx::Sqlite;
use sqlx::SqliteConnection;
use sqlx::query_builder::QueryBuilder;

// #[derive(Debug, Clone)]
// struct Summary {
//     pub start: DateTime<Utc>,
//     pub end: DateTime<Utc>,
//     pub text: String,
//     pub client: String,
//     pub project: String,
//     pub archived: bool,
// }

// Trait implementations for &mut SqliteConnection
impl TimingsQueries for SqliteConnection {
    async fn get_timings(
        &mut self,
        filters: Option<GetTimingsFilters>,
    ) -> Result<Vec<Timing>, Error> {
        let filters = filters.unwrap_or_default();
        let query_parts = str_split!(
            r#"
            SELECT
                timing.start as start,
                timing.end as end,
                project.name as project,
                client.name as client
            FROM timing, project, client
            WHERE timing.projectId = project.id AND project.clientId = client.id -- ?
            AND client.name = ? -- CONDITIONAL
            AND project.name = ? -- CONDITIONAL
            AND timing.start >= ? -- CONDITIONAL
            AND timing.start <= ? -- CONDITIONAL
            ORDER BY timing.start DESC;
        "#,
            "?"
        );

        let mut builder = QueryBuilder::<Sqlite>::new(query_parts[0]);

        if let Some(client) = filters.client {
            builder.push(query_parts[1]);
            builder.push_bind(client);
        }

        if let Some(project) = filters.project.as_deref() {
            builder.push(query_parts[2]);
            builder.push_bind(project);
        }

        if let Some(from) = filters.from {
            let from_ms = datetime_to_ms(&from);
            builder.push(query_parts[3]);
            builder.push_bind(from_ms);
        }

        if let Some(to) = filters.to {
            let to_ms = datetime_to_ms(&to);
            builder.push(query_parts[4]);
            builder.push_bind(to_ms);
        }

        builder.push(query_parts[5]);

        #[derive(sqlx::FromRow)]
        struct TimingRow {
            start: i64,
            end: i64,
            project: String,
            client: String,
        }

        let rows: Vec<TimingRow> = builder.build_query_as().fetch_all(self).await?;

        Ok(rows
            .into_iter()
            .map(|row| -> Option<Timing> {
                Some(Timing {
                    start: ms_to_datetime(row.start).ok()?,
                    end: ms_to_datetime(row.end).ok()?,
                    project: row.project,
                    client: row.client,
                })
            })
            .flatten()
            .collect())
    }

    async fn get_timings_daily_totals(
        &mut self,
        timezone: impl chrono::TimeZone,
        from: NaiveDate,
        to: NaiveDate,
        client: Option<String>,
        project: Option<String>,
    ) -> Result<Vec<DailyTotalSummary>, Error> {
        // Convert NaiveDate to milliseconds timestamps
        let from_dt = timezone
            .from_local_datetime(&from.and_hms_opt(0, 0, 0).ok_or_else(|| {
                Error::ChronoError("Failed to create time at midnight for from date".to_string())
            })?)
            .single()
            .map(|dt| dt.with_timezone(&Utc))
            .ok_or_else(|| Error::ChronoError("Failed to convert from date to UTC".to_string()))?;

        let to_dt = timezone
            .from_local_datetime(&to.and_hms_opt(23, 59, 59).ok_or_else(|| {
                Error::ChronoError("Failed to create time at end of day for to date".to_string())
            })?)
            .single()
            .map(|dt| dt.with_timezone(&Utc))
            .ok_or_else(|| Error::ChronoError("Failed to convert to date to UTC".to_string()))?;

        let from_ms = datetime_to_ms(&from_dt);
        let to_ms = datetime_to_ms(&to_dt);

        let query_parts = str_split!(
            r#"
                SELECT strftime('%Y-%m-%d', CAST (start AS REAL) / 1000, 'unixepoch', 'localtime') AS day,
                    CAST (SUM([end] - start) AS REAL) / 3600000 AS hours,
                    client.name AS client,
                    project.name AS project
                FROM timing,
                    project,
                    client
                WHERE 1=1        
                    AND timing.projectId = project.id
                    AND project.clientId = client.id
                    AND timing.start >= ?
                    AND timing.start <= ?
                    AND client.name LIKE ? -- CONDITIONAL
                    AND project.name LIKE ? -- CONDITIONAL
                GROUP BY timing.projectId, day
                ORDER BY start DESC
        "#,
            "?"
        );

        let mut builder = QueryBuilder::<Sqlite>::new(query_parts[0]);
        builder.push_bind(from_ms);

        builder.push(query_parts[1]);
        builder.push_bind(to_ms);

        if let Some(client_filter) = client {
            builder.push(query_parts[2]);
            builder.push_bind(client_filter);
        }

        if let Some(project_filter) = project {
            builder.push(query_parts[3]);
            builder.push_bind(project_filter);
        }

        builder.push(query_parts[4]);

        #[derive(sqlx::FromRow)]
        struct DailyTotalRow {
            day: String,
            hours: f64,
            client: String,
            project: String,
        }

        let rows: Vec<DailyTotalRow> = builder.build_query_as().fetch_all(self).await?;

        Ok(rows
            .into_iter()
            .map(|row| -> Option<DailyTotalSummary> {
                let day = NaiveDate::parse_from_str(&row.day, "%Y-%m-%d").ok()?;
                // Parse day string "YYYY-MM-DD" to DateTime<Utc> at midnight UTC
                Some(DailyTotalSummary {
                    day: day,
                    hours: row.hours,
                    client: row.client,
                    project: row.project,
                })
            })
            .flatten()
            .collect())
    }

    async fn get_timings_daily_summaries(
        &mut self,
        timezone: impl chrono::TimeZone,
        from: NaiveDate,
        to: NaiveDate,
        client: Option<String>,
        project: Option<String>,
    ) -> Result<Vec<SummaryForDay>, Error> {
        // Convert NaiveDate to milliseconds timestamps
        let from_dt = timezone
            .from_local_datetime(&from.and_hms_opt(0, 0, 0).ok_or_else(|| {
                Error::ChronoError("Failed to create time at midnight for from date".to_string())
            })?)
            .single()
            .map(|dt| dt.with_timezone(&Utc))
            .ok_or_else(|| Error::ChronoError("Failed to convert from date to UTC".to_string()))?;

        let to_dt = timezone
            .from_local_datetime(&to.and_hms_opt(23, 59, 59).ok_or_else(|| {
                Error::ChronoError("Failed to create time at end of day for to date".to_string())
            })?)
            .single()
            .map(|dt| dt.with_timezone(&Utc))
            .ok_or_else(|| Error::ChronoError("Failed to convert to date to UTC".to_string()))?;

        let from_ms = datetime_to_ms(&from_dt);
        let to_ms = datetime_to_ms(&to_dt);

        let mut builder = QueryBuilder::<Sqlite>::new(
            r#"
            SELECT 
                s.start, 
                s.end, 
                s.text as summary, 
                c.name as client, 
                p.name as project, 
                s.archived 
            FROM summary as s, client as c, project as p 
            WHERE p.id = s.projectId AND p.clientId = c.id
            "#,
        );

        builder.push(" AND s.start >= ");
        builder.push_bind(from_ms);

        builder.push(" AND s.start <= ");
        builder.push_bind(to_ms);

        if let Some(client_filter) = client {
            builder.push(" AND c.name = ");
            builder.push_bind(client_filter);
        }

        if let Some(project_filter) = project {
            builder.push(" AND p.name = ");
            builder.push_bind(project_filter);
        }

        builder.push(" ORDER BY s.start DESC");

        #[derive(sqlx::FromRow)]
        struct DailySummaryRow {
            start: i64,
            // end: i64,
            summary: String,
            client: String,
            project: String,
            archived: i32,
        }

        let rows: Vec<DailySummaryRow> = builder.build_query_as().fetch_all(self).await?;

        Ok(rows
            .into_iter()
            .map(|row| -> Option<SummaryForDay> {
                // Convert UTC timestamp to the provided timezone and extract the date
                let start_dt = ms_to_datetime(row.start).ok()?;
                let start_in_tz = start_dt.with_timezone(&timezone);
                let day = start_in_tz.naive_local().date();

                Some(SummaryForDay {
                    day,
                    project: row.project,
                    client: row.client,
                    summary: row.summary,
                    archived: row.archived != 0,
                })
            })
            .flatten()
            .collect())
    }
}
