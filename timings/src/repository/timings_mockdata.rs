use crate::Timing;
use crate::TimingsMockdata;
use crate::TimingsMutations;
use chrono::DateTime;
use chrono::Duration;
use chrono::Timelike;
use chrono::Utc;
use sqlx::SqliteConnection;

const RANDOM: u32 = 896594885u32;

impl TimingsMockdata for SqliteConnection {
    async fn insert_mockdata(&mut self, now: DateTime<Utc>) -> Result<(), crate::Error> {
        // Define clients and projects
        let clients_projects = vec![
            (
                "Oma",
                vec!["Yleinen", "Gmail", "Homma 1", "Homma 2", "Homma 3"],
            ),
            (
                "Acme Corp",
                vec!["Website Redesign", "Backend API", "Mobile App"],
            ),
        ];

        // Generate timings for the past 25 weeks (~175 days)
        let mut timings = Vec::new();
        let weeks = 25;
        let target_hours_per_day = 8.0;

        for week_offset in 0..weeks {
            for day_offset in 0..7 {
                let day_index = week_offset * 7 + day_offset;
                let day = now - Duration::days(day_index as i64 + 1);

                // Skip weekends (Saturday = 6, Sunday = 0)
                // let weekday = day.weekday();
                // if weekday == chrono::Weekday::Sat || weekday == chrono::Weekday::Sun {
                //     continue;
                // }

                // Generate 2-4 timings per day to reach ~8 hours
                let num_timings = RANDOM % 3 + 2; // 2-4 timings
                let mut day_hours = 0.0;
                let mut current_time = day
                    .with_hour(9)
                    .unwrap()
                    .with_minute(0)
                    .unwrap()
                    .with_second(0)
                    .unwrap();

                for timing_idx in 0..num_timings {
                    // Rotate through clients and projects equally
                    let global_timing_index = day_index * 10 + timing_idx as usize;
                    let (client, projects) =
                        &clients_projects[global_timing_index % clients_projects.len()];
                    let project = &projects[global_timing_index % projects.len()];

                    // Generate duration: aim for ~2 hours per timing initially
                    let duration_minutes = 60 + (RANDOM % 90); // 60-150 minutes
                    let duration = Duration::minutes(duration_minutes as i64);
                    let end_time = current_time + duration;

                    // Make sure we don't exceed ~8 hours per day
                    let hours = duration.num_minutes() as f64 / 60.0;
                    if day_hours + hours > target_hours_per_day * 1.1 {
                        break;
                    }

                    timings.push(Timing {
                        start: current_time,
                        end: end_time,
                        project: project.to_string(),
                        client: client.to_string(),
                    });

                    day_hours += hours;
                    current_time = end_time + Duration::minutes(5 + (RANDOM % 15) as i64); // 5-20 min break
                }
            }
        }

        // Insert all timings
        self.insert_timings(timings.iter()).await?;

        Ok(())
    }
}
