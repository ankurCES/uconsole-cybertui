use wifi_radar::version;

#[test]
fn exposes_version_string() {
    let v = version();
    assert!(v.starts_with("wifi-radar "), "got {v:?}");
}