use chrono::Days;
use chrono::Local;
use chrono::Utc;
use sqlx::SqlitePool;

use timings::*;

#[tokio::main]
async fn main() -> Result<(), Error> {
    println!("Hello, world!");

    let pool = SqlitePool::connect("sqlite::memory:").await?;
    let mut conn = pool.acquire().await?;
    conn.create_timings_database().await?;

    let now = Utc::now();
    conn.insert_timings(&[Timing {
        client: "foo".to_string(),
        project: "zoo".to_string(),
        start: now,
        end: now.checked_add_days(Days::new(1)).unwrap(),
    }])
    .await?;

    conn.insert_timings_daily_summaries(
        chrono::Local,
        &[SummaryForDay {
            day: Local::now().date_naive(),
            project: "zoo".to_string(),
            client: "foo".to_string(),
            summary: "Worked on foo zoo project".to_string(),
            archived: false,
        }],
    )
    .await?;

    let timings = conn.get_timings(None).await?;
    for timing in timings {
        println!("{:?}", timing);
    }

    let summaries = conn
        .get_timings_daily_summaries(
            chrono::Local,
            Local::now().date_naive(),
            Local::now().date_naive(),
            None,
            None,
        )
        .await?;
    for summary in summaries {
        println!("{:?}", summary);
    }

    println!("Database created successfully!");

    Ok(())
}
