// Probe inserted as a sanity test inside the tui crate to compare
// behaviour with the standalone probe project.
#[test]
fn tz_diag_probe() {
    use chrono::TimeZone;
    println!("TZ env = {:?}", std::env::var("TZ"));
    let res = chrono::Local.with_ymd_and_hms(2024, 6, 8, 12, 0, 0);
    println!("2024-06-08 12:00 Local → {res:?}");
    panic!("forced panic to see stdout above");
}
