use chrono::DateTime;
use chrono::Duration;
use chrono::TimeZone;
use chrono::Utc;
use sqlx::SqlitePool;
use timings::TimingsMutations;
use timings::TimingsQueries;
use timings::TimingsRecorder;
use timings::TimingsRecording;

async fn setup_test_db() -> Result<SqlitePool, Box<dyn std::error::Error>> {
    let pool = SqlitePool::connect("sqlite::memory:").await?;
    let mut conn = pool.acquire().await?;
    conn.create_timings_database().await?;
    Ok(pool)
}

fn call_keep_alives(recorder: &mut TimingsRecorder, start: DateTime<Utc>, end: DateTime<Utc>) {
    let duration = (end - start).num_seconds() as usize;
    let keep_alive_intervals = duration / 30;
    for i in 1..=keep_alive_intervals {
        let keep_alive_time = start + Duration::seconds((i * 30) as i64);
        recorder.keep_alive_timing(keep_alive_time);
    }
}

#[tokio::test]
async fn test_start_timing_multiple_and_persist() -> Result<(), Box<dyn std::error::Error>> {
    let pool = setup_test_db().await?;
    let mut conn = pool.acquire().await?;

    let mut recorder = TimingsRecorder::new();
    let start_time = Utc.with_ymd_and_hms(2020, 5, 5, 12, 0, 0).unwrap();

    // Create multiple timings with distinct client/project combinations
    let times = vec![
        (
            "cli_a",
            "proj_a",
            start_time,
            start_time + Duration::seconds(60),
        ),
        (
            "cli_b",
            "proj_b",
            start_time + Duration::seconds(120),
            start_time + Duration::seconds(180),
        ),
        (
            "cli_c",
            "proj_c",
            start_time + Duration::seconds(240),
            start_time + Duration::seconds(300),
        ),
    ];

    for (client, project, start, end) in times.iter() {
        recorder.keep_alive_timing(*start);
        recorder.start_timing(client.to_string(), project.to_string(), *start);
        call_keep_alives(&mut recorder, *start, *end);
        recorder.stop_timing(*end);
    }

    // Write to database
    recorder.write_timings(&mut *conn).await?;

    // Verify all were written
    let timings = conn.get_timings(None).await?;

    // Debug: print all timings
    // println!("Found {} timings:", timings.len());
    // for (i, timing) in timings.iter().enumerate() {
    //     println!(
    //         "{}: {} / {} - {} to {} (duration: {}s)",
    //         i,
    //         timing.client,
    //         timing.project,
    //         timing.start,
    //         timing.end,
    //         (timing.end - timing.start).num_seconds()
    //     );
    // }

    assert_eq!(timings.len(), 3);

    // Sort by start time to ensure consistent ordering
    let mut sorted_timings = timings.clone();
    sorted_timings.sort_by_key(|t| t.start);

    assert_eq!(sorted_timings[0].client, "cli_a");
    assert_eq!(sorted_timings[0].project, "proj_a");
    assert_eq!(sorted_timings[0].start, start_time);
    assert_eq!(sorted_timings[0].end, start_time + Duration::seconds(60));

    assert_eq!(sorted_timings[1].client, "cli_b");
    assert_eq!(sorted_timings[1].project, "proj_b");
    assert_eq!(sorted_timings[1].start, start_time + Duration::seconds(120));
    assert_eq!(sorted_timings[1].end, start_time + Duration::seconds(180));

    assert_eq!(sorted_timings[2].client, "cli_c");
    assert_eq!(sorted_timings[2].project, "proj_c");
    assert_eq!(sorted_timings[2].start, start_time + Duration::seconds(240));
    assert_eq!(sorted_timings[2].end, start_time + Duration::seconds(300));

    Ok(())
}

#[tokio::test]
async fn test_keep_alive_timeout_splits_timing() -> Result<(), Box<dyn std::error::Error>> {
    let pool = setup_test_db().await?;
    let mut conn = pool.acquire().await?;

    let mut recorder = TimingsRecorder::new();
    let start_time = Utc.with_ymd_and_hms(2020, 5, 5, 12, 0, 0).unwrap();

    recorder.start_timing("client1".to_string(), "project1".to_string(), start_time);

    // First keep-alive at 30 seconds
    recorder.keep_alive_timing(start_time + Duration::seconds(30));

    // Second keep-alive at 91 seconds - more than 60 seconds after the first (30s)
    // This should trigger the split: one timing ending at first keep-alive (30s),
    // and current timing restarting at 91 seconds
    recorder.keep_alive_timing(start_time + Duration::seconds(91));

    // Stop the timing
    recorder.stop_timing(start_time + Duration::seconds(120));

    // Write to database
    recorder.write_timings(&mut *conn).await?;

    // Verify the timing was split into two
    let timings = conn.get_timings(None).await?;

    assert_eq!(timings.len(), 2, "Expected timing to be split into 2 parts");

    // Sort by start time
    let mut sorted_timings = timings.clone();
    sorted_timings.sort_by_key(|t| t.start);

    // First timing: from start to first keep-alive (30 seconds)
    assert_eq!(sorted_timings[0].client, "client1");
    assert_eq!(sorted_timings[0].project, "project1");
    assert_eq!(sorted_timings[0].start, start_time);
    assert_eq!(sorted_timings[0].end, start_time + Duration::seconds(30));

    // Second timing: from the late keep-alive to stop (91 to 120 seconds)
    assert_eq!(sorted_timings[1].client, "client1");
    assert_eq!(sorted_timings[1].project, "project1");
    assert_eq!(sorted_timings[1].start, start_time + Duration::seconds(91));
    assert_eq!(sorted_timings[1].end, start_time + Duration::seconds(120));

    Ok(())
}
