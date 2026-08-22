use bhtune_db::{models::SettingRow, pool::connect_in_memory};
use chrono::{TimeZone, Utc};

#[tokio::test]
async fn setting_round_trips_and_is_overwritten_by_key() {
    let pool = connect_in_memory().await.unwrap();
    let first_time = Utc.with_ymd_and_hms(2026, 8, 16, 10, 0, 0).unwrap();
    let second_time = Utc.with_ymd_and_hms(2026, 8, 16, 10, 1, 0).unwrap();

    assert!(
        SettingRow::get(&pool, "new_run_draft")
            .await
            .unwrap()
            .is_none()
    );

    let first = SettingRow::upsert(
        &pool,
        "new_run_draft",
        &serde_json::json!({"driver": "simulator", "notes": "not persisted"}),
        first_time,
    )
    .await
    .unwrap();
    assert_eq!(first.key, "new_run_draft");
    assert_eq!(first.value["driver"], "simulator");
    assert_eq!(first.updated_at, first_time);

    let second = SettingRow::upsert(
        &pool,
        "new_run_draft",
        &serde_json::json!({"driver": "opcda", "bridge_host": "localhost:7600"}),
        second_time,
    )
    .await
    .unwrap();
    assert_eq!(second.value["driver"], "opcda");
    assert_eq!(second.value["bridge_host"], "localhost:7600");
    assert_eq!(second.updated_at, second_time);

    let fetched = SettingRow::get(&pool, "new_run_draft")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched, second);
}
