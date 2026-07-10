//! Integration test: the full router (shell + API + SSE) returns 200 on
//! `GET /` and the body contains the radar canvas element.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::Request;
use tokio::sync::{broadcast, mpsc};
use tower::ServiceExt;

use wifi_radar::api::AppState;
use wifi_radar::ble_devices::BleDeviceStore;
use wifi_radar::devices::DeviceStore;
use wifi_radar::frames::DeviceEvent;
use wifi_radar::run::build_router;
use wifi_radar::tags::TagDb;

fn temp_tags_path() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "wifi-radar-webshell-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("tags.json")
}

fn workspace_web_dir() -> PathBuf {
    // tests/ run from the crate root, so `web/` is the static dir.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("web")
}

#[tokio::test]
async fn get_root_returns_radar_html() {
    let tags_path = temp_tags_path();
    let _ = std::fs::remove_file(&tags_path);
    let tags = Arc::new(TagDb::load(&tags_path).unwrap());
    let store = Arc::new(DeviceStore::new());
    let (events_tx, _) = broadcast::channel::<DeviceEvent>(16);
    let (scanner_tx, _scanner_rx) = mpsc::channel::<DeviceEvent>(16);
    let state = Arc::new(AppState {
        store,
        tags,
        vitals: std::sync::Arc::new(wifi_radar::csi::VitalsStore::new()),
        events_tx,
        scanner_tx,
        ble_store: Arc::new(BleDeviceStore::new()),
    });
    let app = build_router(state, workspace_web_dir());

    let resp = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = axum::body::to_bytes(resp.into_body(), 256 * 1024).await.unwrap();
    let s = String::from_utf8_lossy(&body);
    assert!(s.contains("<canvas"), "shell body should contain a <canvas>: {s}");
    assert!(
        s.contains("id=\"radar\""),
        "shell body should contain <canvas id=\"radar\">: {s}"
    );
}

#[tokio::test]
async fn static_assets_are_served() {
    let tags_path = temp_tags_path();
    let _ = std::fs::remove_file(&tags_path);
    let tags = Arc::new(TagDb::load(&tags_path).unwrap());
    let store = Arc::new(DeviceStore::new());
    let (events_tx, _) = broadcast::channel::<DeviceEvent>(16);
    let (scanner_tx, _scanner_rx) = mpsc::channel::<DeviceEvent>(16);
    let state = Arc::new(AppState {
        store,
        tags,
        vitals: std::sync::Arc::new(wifi_radar::csi::VitalsStore::new()),
        events_tx,
        scanner_tx,
        ble_store: Arc::new(BleDeviceStore::new()),
    });
    let app = build_router(state, workspace_web_dir());

    for asset in ["style.css", "app.js", "radar.js"] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/static/{asset}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "{asset} returned {}", resp.status());
    }
}

/// Regression test: when `--static-dir` doesn't exist on disk (e.g. the
/// binary is installed at `/usr/local/bin/wifi-radar` and started from
/// cwd `/`), the embedded-asset fallback must still serve CSS/JS. Before
/// this fix the browser rendered only the topbar (`<span>wifi-radar
/// wifi-radar 0.1.0 connecting…</span>`) because every static asset 404'd.
#[tokio::test]
async fn static_assets_fall_back_to_embedded_when_dir_absent() {
    let tags_path = temp_tags_path();
    let _ = std::fs::remove_file(&tags_path);
    let tags = Arc::new(TagDb::load(&tags_path).unwrap());
    let store = Arc::new(DeviceStore::new());
    let (events_tx, _) = broadcast::channel::<DeviceEvent>(16);
    let (scanner_tx, _scanner_rx) = mpsc::channel::<DeviceEvent>(16);
    let state = Arc::new(AppState {
        store,
        tags,
        vitals: std::sync::Arc::new(wifi_radar::csi::VitalsStore::new()),
        events_tx,
        scanner_tx,
        ble_store: Arc::new(BleDeviceStore::new()),
    });

    // Point at a path that definitely doesn't exist on disk.
    let missing = PathBuf::from("/tmp/wifi-radar-does-not-exist-1234567890/web");
    assert!(
        !missing.exists(),
        "test precondition: {missing:?} must not exist"
    );

    let app = build_router(state, missing);

    for asset in ["style.css", "app.js", "radar.js"] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/static/{asset}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            200,
            "{asset} should fall back to embedded (got {})",
            resp.status()
        );
        // Body should not be empty — the browser would otherwise log a
        // "Refused to execute script" warning.
        let body = axum::body::to_bytes(resp.into_body(), 256 * 1024)
            .await
            .unwrap();
        assert!(
            !body.is_empty(),
            "{asset} fell back but body was empty (mime guess wrong?)"
        );
    }
}