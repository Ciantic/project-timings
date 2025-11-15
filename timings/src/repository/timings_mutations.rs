//! Repository functions for timings
//!
//! Not to be used directly, use the traits in `timings.rs` instead.

use super::utils::datetime_to_ms;
use crate::error::Error;
use crate::{SummaryForDay, Timing, TimingsMutations};
use chrono::{DateTime, Utc};
use sqlx::Acquire;
use sqlx::{Executor, SqliteConnection};

#[derive(Debug, Clone)]
struct Summary {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub text: String,
    pub client: String,
    pub project: String,
    pub archived: bool,
}

async fn get_or_create_client_id(
    conn: &mut SqliteConnection,
    client_name: &str,
) -> Result<i64, sqlx::Error> {
    // Try to get existing client
    let existing: Option<(i64,)> = sqlx::query_as("SELECT id FROM client WHERE name = ?")
        .bind(client_name)
        .fetch_optional(&mut *conn)
        .await?;

    if let Some((id,)) = existing {
        return Ok(id);
    }

    // Create new client
    let result = sqlx::query("INSERT INTO client (name) VALUES (?)")
        .bind(client_name)
        .execute(&mut *conn)
        .await?;

    Ok(result.last_insert_rowid())
}

async fn get_or_create_project_id(
    conn: &mut SqliteConnection,
    project_name: &str,
    client_id: i64,
) -> Result<i64, sqlx::Error> {
    // Try to get existing project
    let existing: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM project WHERE name = ? AND clientId = ?")
            .bind(project_name)
            .bind(client_id)
            .fetch_optional(&mut *conn)
            .await?;

    if let Some((id,)) = existing {
        return Ok(id);
    }

    // Create new project
    let result = sqlx::query("INSERT INTO project (name, clientId) VALUES (?, ?)")
        .bind(project_name)
        .bind(client_id)
        .execute(&mut *conn)
        .await?;

    Ok(result.last_insert_rowid())
}

async fn insert_timings_summary(
    conn: &mut SqliteConnection,
    summary: Summary,
) -> Result<(), Error> {
    // Get or create the client id from the client name
    let client_id = get_or_create_client_id(conn, &summary.client).await?;
    // Get or create the project id from the project and client names
    let project_id = get_or_create_project_id(conn, &summary.project, client_id).await?;

    // Convert DateTime<Utc> to milliseconds
    let start_ms = datetime_to_ms(&summary.start);

    if summary.text.is_empty() {
        // Delete the summary from the database
        sqlx::query("DELETE FROM summary WHERE start = ? AND projectId = ?")
            .bind(start_ms)
            .bind(project_id)
            .execute(conn)
            .await?;
        return Ok(());
    }

    // Convert DateTime<Utc> to milliseconds
    let end_ms = datetime_to_ms(&summary.end);

    // Insert the summary into the database
    // Using UPSERT to update text and archived if the summary already exists
    sqlx::query(
        r#"
        INSERT INTO summary (start, [end], text, projectId, archived) 
        VALUES (?, ?, ?, ?, ?)
        ON CONFLICT (projectId, start, [end]) 
        DO UPDATE 
        SET 
            text = excluded.text, 
            archived = excluded.archived
        "#,
    )
    .bind(start_ms)
    .bind(end_ms)
    .bind(summary.text)
    .bind(project_id)
    .bind(summary.archived as i32)
    .execute(&mut *conn)
    .await?;

    Ok(())
}

static CLIENT_SCHEMA: &str = include_str!("schema.sql");

impl TimingsMutations for SqliteConnection {
    async fn create_timings_database(&mut self) -> Result<(), Error> {
        self.execute(CLIENT_SCHEMA).await?;
        Ok(())
    }

    async fn insert_timings(
        &mut self,
        timings: impl IntoIterator<Item = &Timing>,
    ) -> Result<(), Error> {
        let mut tx = self.begin().await?;
        for timing in timings {
            // Get or create the client id from the client name
            let client_id = get_or_create_client_id(&mut tx, &timing.client).await?;

            // Get or create the project id from the project and client names
            let project_id = get_or_create_project_id(&mut tx, &timing.project, client_id).await?;

            // Convert DateTime<Utc> to milliseconds
            let start_ms = datetime_to_ms(&timing.start);
            let end_ms = datetime_to_ms(&timing.end);

            // Insert the timing into the database
            // Using UPSERT to update end time if the timing already exists
            sqlx::query(
                r#"
                    INSERT INTO timing (start, [end], projectId) 
                    VALUES (?, ?, ?)
                    ON CONFLICT (projectId, start) 
                    DO UPDATE SET [end] = excluded.[end]
                "#,
            )
            .bind(start_ms)
            .bind(end_ms)
            .bind(project_id)
            .execute(<&mut SqliteConnection>::from(&mut tx))
            .await?;
        }

        tx.commit().await?;

        Ok(())
    }

    async fn insert_timings_daily_summaries(
        &mut self,
        timezone: impl chrono::TimeZone,
        summaries: impl IntoIterator<Item = &SummaryForDay>,
    ) -> Result<(), Error> {
        let mut tx = self.begin().await?;

        for summary in summaries {
            // Convert NaiveDate to DateTime using the provided timezone
            let start_dt = timezone
                .from_local_datetime(&summary.day.and_hms_opt(0, 0, 0).ok_or_else(|| {
                    Error::ChronoError("Failed to create time at midnight".to_string())
                })?)
                .single()
                .map(|dt| dt.with_timezone(&Utc))
                .ok_or_else(|| {
                    Error::ChronoError("Failed to convert local datetime to UTC".to_string())
                })?;

            // Get start of next day
            let next_day_dt = summary
                .day
                .succ_opt()
                .ok_or_else(|| Error::ChronoError("Failed to get next day".to_string()))?;

            let next_day_dt = timezone
                .from_local_datetime(&next_day_dt.and_hms_opt(0, 0, 0).ok_or_else(|| {
                    Error::ChronoError("Failed to create time at midnight for next day".to_string())
                })?)
                .single()
                .map(|dt| dt.with_timezone(&Utc))
                .ok_or_else(|| {
                    Error::ChronoError(
                        "Failed to convert next day local datetime to UTC".to_string(),
                    )
                })?;

            // Insert summary using the existing insert_timings_summary
            insert_timings_summary(
                &mut tx,
                Summary {
                    start: start_dt,
                    end: next_day_dt,
                    project: summary.project.clone(),
                    client: summary.client.clone(),
                    text: summary.summary.clone(),
                    archived: summary.archived,
                },
            )
            .await?;
        }
        tx.commit().await?;

        Ok(())
    }
}
