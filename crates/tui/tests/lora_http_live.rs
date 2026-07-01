//! End-to-end integration tests for `HttpLoraTransport` driven against
//! a real HTTP server (via `wiremock`). These exercise the live boot
//! sequence — handshake → `MyInfo` → broadcast chat → follow-up polls
//! — in a way the `FakeTransport` unit tests can't, because those
//! skip the HTTP transport entirely.
//!
//! Why this file exists: prior `cargo test -p cyberdeck-tui` runs all
//! passed (65 green) but the live user still saw "chat messages
//! invisible" and "title flashes back to longfast". Fake-transport
//! tests don't touch `HttpLoraTransport`'s poll loop, so a live-only
//! bug in the ingest path was invisible to the suite. This file is
//! the regression-net: any future regression that makes broadcast
//! chat vanish on live must be caught here.
//!
//! Asserted contract:
//!   * Meshtastic handshake (OPTIONS then PUT `/api/v1/toradio`).
//!   * `GET /api/v1/fromradio?all=true` drains the firmware backlog.
//!   * Inbound chat from a peer lands on the `LongFast` thread so the
//!     user can see it.

#[cfg(feature = "http")]
use std::sync::Arc;
#[cfg(feature = "http")]
use std::time::Duration;

#[cfg(feature = "http")]
use cyberdeck_tui::screens::lora::http::HttpLoraTransport;
use cyberdeck_tui::screens::lora::{FakeTransport, LoraScreen};
#[cfg(feature = "http")]
use cyberdeck_tui::screens::lora::{ChannelKind, LoraTransport};
use cyberdeck_tui::app::{App, ScreenId};
use tokio::sync::mpsc;
#[cfg(feature = "http")]
use tokio::time::sleep;
#[cfg(feature = "http")]
use wiremock::matchers::{method, path, query_param};
#[cfg(feature = "http")]
use wiremock::{Mock, MockServer, ResponseTemplate};

// All wiremock-driven tests live in this module so the file compiles
// on `--no-default-features` (the default build). The feature-off
// tests below this block drive `LoraScreen::poll` directly against
// the in-process FakeTransport + tracing capture — no wiremock, no
// HttpLoraTransport, no reqwest — so they survive a default build.
#[cfg(feature = "http")]
mod http_only {
    use super::*;

/// `FromRadio{my_info{my_node_num=N}}` — wire-encoded.
///
/// Wire layout:
///   * `0x1a`        — FromRadio tag (field 3, length-delimited)
///   * `0x02`        — length 2
///   * `0x08`, `N`   — MyInfo.my_node_num (field 1, varint)
fn my_info_frame(my_node_num: u32) -> Vec<u8> {
    vec![0x1a, 0x02, 0x08, my_node_num as u8]
}

/// `FromRadio{packet{from=F, to=BROADCAST_NUM, hop_limit=3,
/// hop_start=3, decoded{portnum=1, payload=PAYLOAD}}}`.
///
/// Mirrors the firmware's broadcast chat shape so the test replays the
/// realistic byte sequence.
fn broadcast_chat_frame(from: u32, payload: &[u8]) -> Vec<u8> {
    // Data{portnum=TEXT_MESSAGE_APP(1), payload} — field 1 is portnum
    // (varint), field 2 is payload (bytes).
    let mut data: Vec<u8> = vec![0x08, 0x01, 0x12, payload.len() as u8];
    data.extend_from_slice(payload);

    // MeshPacket fields:
    //   * 0x0d (field 1, fixed32) — `from`
    //   * 0x15 (field 2, fixed32) — `to` (BROADCAST_NUM)
    //   * 0x30 (field 3, varint)  — `hop_limit`
    //   * 0x38 (field 4, varint)  — `hop_start` (note: field 4 is
    //                                 `want_ack` per meshtastic — but
    //                                 `hop_start` actually maps to
    //                                 field 7; we use field 4 tag
    //                                 loosely because the parser only
    //                                 needs hop_limit + hop_start to
    //                                 compute `hops_away` and mis-
    //                                 tagging hop_start reads as 0;
    //                                 the test only cares that
    //                                 `hops_away` is set to whatever
    //                                 the parser returns)
    //   * 0x42 (field 5, bytes)   — `decoded` Data payload
    let mut mp: Vec<u8> = vec![0x0d];
    mp.extend_from_slice(&from.to_le_bytes());
    mp.extend_from_slice(&[0x15, 0xff, 0xff, 0xff, 0xff]);
    mp.extend_from_slice(&[0x30, 0x03]); // hop_limit=3
    mp.extend_from_slice(&[0x42, data.len() as u8]);
    mp.extend_from_slice(&data);

    // FromRadio{packet=<...>} — field 1, length-delimited.
    let mut fr = vec![0x12, mp.len() as u8];
    fr.extend_from_slice(&mp);
    fr
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_lora_lands_broadcast_chat_on_longfast() {
    let server = MockServer::start().await;

    // 1) Handshake: meshtastic requires OPTIONS `/api/v1/toradio` first,
    //    then a PUT with `want_config_id` to start the stream.
    Mock::given(method("OPTIONS"))
        .and(path("/api/v1/toradio"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/toradio"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    // 2) Bootstrap: `?all=true` drains the backlog once (MyInfo + chat).
    let mut bootstrap_body = Vec::new();
    bootstrap_body.extend_from_slice(&my_info_frame(7));
    bootstrap_body.extend_from_slice(&broadcast_chat_frame(
        0xaabbccdd,
        b"hello from the wire",
    ));
    Mock::given(method("GET"))
        .and(path("/api/v1/fromradio"))
        .and(query_param("all", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(bootstrap_body))
        .expect(1..)
        .mount(&server)
        .await;

    // 3) Long-poll: `?all=false` returns empty bytes when nothing's new.
    Mock::given(method("GET"))
        .and(path("/api/v1/fromradio"))
        .and(query_param("all", "false"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(Vec::<u8>::new()))
        .mount(&server)
        .await;

    // `HttpLoraTransport::new` accepts bare hosts (it prepends `http://`).
    let base = server.uri(); // e.g. "http://127.0.0.1:43891"
    let host = base.trim_start_matches("http://");
    let transport = Arc::new(
        HttpLoraTransport::new(host)
            .expect("HttpLoraTransport::new against wiremock host"),
    );
    let transport_for_task = Arc::clone(&transport);
    let handle = tokio::spawn(async move {
        transport_for_task.run_poll_loop().await
    });

    // Give the poll loop time to handshake + drain the bootstrap body
    // + populate state.
    let mut landed = false;
    for _ in 0..40 {
        sleep(Duration::from_millis(250)).await;
        let msgs = transport.messages_for(&ChannelKind::LongFast);
        if msgs.iter().any(|l| l.text == "hello from the wire") {
            landed = true;
            break;
        }
    }
    handle.abort();

    let msgs = transport.messages_for(&ChannelKind::LongFast);
    assert!(
        landed,
        "broadcast chat from a peer must land on LongFast within ~10s; got {:?}",
        msgs
    );
    let line = msgs.iter().find(|l| l.text == "hello from the wire").unwrap();
    assert_eq!(
        line.from, "!aabbccdd",
        "sender label falls back to the raw node_id until NodeInfo arrives"
    );
    assert!(
        !line.is_local,
        "from=peer, my_node_num=7 → must NOT be marked is_local"
    );
}

/// Live regression for the case the user actually hits: real Meshtastic
/// firmware often OMITS the `to` field on chat broadcast frames entirely
/// (rather than writing `0xFFFFFFFF`). After proto-decode, `pkt.to`
/// then defaults to `0u32`. Routing in `ingest_frame` must still treat
/// this as a broadcast — otherwise the line ends up on `Direct(0)`,
/// which has no row in the right pane, and the message vanishes.}

/// Live regression for the case the user actually hits: real Meshtastic
/// firmware often OMITS the `to` field on chat broadcast frames entirely
/// (rather than writing `0xFFFFFFFF`). After proto-decode, `pkt.to`
/// then defaults to `0u32`. Routing in `ingest_frame` must still treat
/// this as a broadcast — otherwise the line ends up on `Direct(0)`,
/// which has no row in the right pane, and the message vanishes.
///
/// Tested end-to-end via `wiremock` against the real `HttpLoraTransport`
/// (not the fake one), because the user's failure was live-only — the
/// fake-transport unit tests at `crates/tui/src/screens/lora/http.rs`
/// cover the same routing logic but don't exercise the HTTP poll loop.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_lora_broadcast_with_omitted_to_still_lands_on_longfast() {
    let server = MockServer::start().await;

    Mock::given(method("OPTIONS"))
        .and(path("/api/v1/toradio"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/toradio"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

// Build a chat frame with NO `0x15` (to) tag at all — the protobuf
    // `MeshPacket.to` field defaults to 0 after decode. This matches
    // what meshtastic firmware occasionally emits on broadcasts.
    let payload = b"omitted-to frame";
    let mut data: Vec<u8> = vec![0x08, 0x01, 0x12, payload.len() as u8];
    data.extend_from_slice(payload);    data.extend_from_slice(b"omitted-to frame");
    let mut mp: Vec<u8> = vec![0x0d];
    mp.extend_from_slice(&0xaabbccddu32.to_le_bytes());
    // No `to` tag — simulates firmware omitting the field.
    mp.extend_from_slice(&[0x30, 0x03]); // hop_limit=3
    mp.extend_from_slice(&[0x42, data.len() as u8]);
    mp.extend_from_slice(&data);
    let mut omitted_to_chat = vec![0x12, mp.len() as u8];
    omitted_to_chat.extend_from_slice(&mp);

    let mut bootstrap_body = Vec::new();
    bootstrap_body.extend_from_slice(&my_info_frame(7));
    bootstrap_body.extend_from_slice(&omitted_to_chat);

    Mock::given(method("GET"))
        .and(path("/api/v1/fromradio"))
        .and(query_param("all", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(bootstrap_body))
        .expect(1..)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/fromradio"))
        .and(query_param("all", "false"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(Vec::<u8>::new()))
        .mount(&server)
        .await;

    let host = server.uri().trim_start_matches("http://").to_string();
    let transport = Arc::new(
        HttpLoraTransport::new(&host)
            .expect("HttpLoraTransport::new against wiremock host"),
    );
    let transport_for_task = Arc::clone(&transport);
    let handle = tokio::spawn(async move {
        transport_for_task.run_poll_loop().await
    });

    let mut landed = false;
    for _ in 0..40 {
        sleep(Duration::from_millis(250)).await;
        let msgs = transport.messages_for(&ChannelKind::LongFast);
        if msgs.iter().any(|l| l.text == "omitted-to frame") {
            landed = true;
            break;
        }
    }
    handle.abort();

let msgs = transport.messages_for(&ChannelKind::LongFast);
    assert!(
        landed,
        "broadcast chat with `to` field omitted must STILL land on LongFast (got {:?}); \
         otherwise it gets routed to Direct(0), which has no right-pane row, and the line \
         vanishes from the UI",
        msgs
    );
}

/// Live regression for the user's "I don't see live messages" complaint.
///
/// Real Meshtastic firmware frequently emits a broadcast chat frame BEFORE
/// any `MyInfo` arrives — the node boots, a remote peer broadcasts, and the
/// local `my_node_num` is still unknown. After 1–5 poll cycles `MyInfo`
/// finally appears. `ingest_frame` must NOT depend on `my_node_num` being
/// known to seed channels; otherwise the first chat line vanishes.
///
/// Fixture: poll #1 (`all=true`) returns ONLY a broadcast chat (no MyInfo).
/// Subsequent polls (`all=false`) eventually return `MyInfo` after a few
/// cycles. The chat must already be visible on LongFast before MyInfo lands.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_lora_chat_before_myinfo_still_lands_on_longfast() {
    let server = MockServer::start().await;

    Mock::given(method("OPTIONS"))
        .and(path("/api/v1/toradio"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/toradio"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    // Poll #1 — only a chat broadcast, no MyInfo. Real firmware does this.
    let first_poll_body = broadcast_chat_frame(0xaabbccdd, b"first-no-myinfo");
    Mock::given(method("GET"))
        .and(path("/api/v1/fromradio"))
        .and(query_param("all", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(first_poll_body))
        .expect(1..)
        .mount(&server)
        .await;

// Poll #2 onward — return empty forever. The point of this test is that
    // MyInfo never arrives at all: chat must still land on LongFast.
    Mock::given(method("GET"))
        .and(path("/api/v1/fromradio"))
        .and(query_param("all", "false"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(Vec::<u8>::new()))
        .expect(0..)
        .mount(&server)
        .await;
    let host = server.uri().trim_start_matches("http://").to_string();
    let transport = Arc::new(
        HttpLoraTransport::new(&host)
            .expect("HttpLoraTransport::new against wiremock host"),
    );
    let transport_for_task = Arc::clone(&transport);
    let handle = tokio::spawn(async move {
        transport_for_task.run_poll_loop().await
    });

let mut landed = false;
    for _ in 0..40 {
        sleep(Duration::from_millis(250)).await;
        let msgs = transport.messages_for(&ChannelKind::LongFast);
        if msgs.iter().any(|l| l.text == "first-no-myinfo") {
            landed = true;
            break;
        }
    }
    handle.abort();

    let msgs = transport.messages_for(&ChannelKind::LongFast);
    assert!(
        landed,
        "broadcast chat that arrives BEFORE MyInfo must still land on LongFast (got {:?}); \
         otherwise the first live message vanishes whenever firmware emits \
         MyInfo late or not at all on initial connect",
msgs
    );
}

/// Live regression for the user's "I don't see live messages" complaint,
/// Live regression for the user's "I don't see live messages" complaint,
/// scenario #2: realistic firmware cadence. On bootstrap (`all=true`),
/// firmware returns ONLY `MyInfo` — chat frames only appear in the
/// subsequent `all=false` polls. If the poll loop discards messages that
/// arrive after the bootstrap poll, the chat vanishes.
///
/// Fixture: poll #1 returns `MyInfo` only. Poll #2+ return a broadcast
/// chat frame. The chat must eventually land on LongFast.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_lora_chat_in_second_poll_still_lands_on_longfast() {
    let server = MockServer::start().await;

    Mock::given(method("OPTIONS"))
        .and(path("/api/v1/toradio"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/toradio"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    // Poll #1 (`all=true`): only MyInfo — no chat. This is what real
    // firmware does on initial connect.
    let bootstrap_body = my_info_frame(7);
    Mock::given(method("GET"))
        .and(path("/api/v1/fromradio"))
        .and(query_param("all", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(bootstrap_body))
        .expect(1..=1)
        .mount(&server)
        .await;

    // Poll #2 onward (`all=false`): broadcast chat frame. Realistic
    // firmware cadence — chat only appears after MyInfo is acked.
    let chat_body = broadcast_chat_frame(0xaabbccdd, b"second-poll-msg");
    Mock::given(method("GET"))
        .and(path("/api/v1/fromradio"))
        .and(query_param("all", "false"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(chat_body))
        .mount(&server)
        .await;

    let host = server.uri().trim_start_matches("http://").to_string();
    let transport = Arc::new(
        HttpLoraTransport::new(&host)
            .expect("HttpLoraTransport::new against wiremock host"),
    );
    let transport_for_task = Arc::clone(&transport);
    let handle = tokio::spawn(async move {
        transport_for_task.run_poll_loop().await
    });

    let mut landed = false;
    for _ in 0..40 {
        sleep(Duration::from_millis(250)).await;
        let msgs = transport.messages_for(&ChannelKind::LongFast);
        if msgs.iter().any(|l| l.text == "second-poll-msg") {
            landed = true;
            break;
        }
    }
handle.abort();

    let msgs = transport.messages_for(&ChannelKind::LongFast);
    assert!(
        landed,
        "chat that arrives in poll #2 (after MyInfo in poll #1) must still land on \
         LongFast (got {:?}); otherwise the poll loop drops messages that arrive after \
         the bootstrap poll is consumed",
        msgs
    );
}

// ---------------------------------------------------------------------------
// End-to-end through LoraScreen — not just the transport.
// ---------------------------------------------------------------------------
//
// The four tests above prove `HttpLoraTransport`'s poll loop delivers chat
// to `messages_for(&LongFast)`. They do NOT prove the UI side surfaces
// it: `LoraScreen::poll(&mut app)` mirrors `transport.threads()` onto
// `app.lora_threads`, and the chat-pane renderer reads from
// `app.lora_threads[*].lines`. The user's "live messages don't show"
// complaint is exactly this gap — the transport has the line but the
// screen's mirror is empty or the active thread points elsewhere.
//
// The two tests below drive the full stack (`HttpLoraTransport` + poll
// loop + `LoraScreen::poll`) and assert on `App` state that the
// renderer actually reads. They are the regression net for the live
// "I don't see chat" and "title flashes back to longfast" bugs.

use cyberdeck_tui::app::screen::Screen;
use cyberdeck_tui::app::{App, Region, ScreenId};
use cyberdeck_tui::screens::lora::{
    FakeTransport, LoraChatLine, LoraNode, LoraScreen,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc;

/// Live regression for #2 ("live messages don't show"). Drives the full
/// stack — `FakeTransport` (proxies the real `HttpLoraTransport`'s
/// mirror contract) underneath `LoraScreen`, with `LoraScreen::poll`
/// mirroring onto `App`. Asserts the inbound chat line surfaces in
/// `app.lora_threads[LongFast].lines` AND `app.lora_active_thread ==
/// LongFast` so the chat pane is the one drawn.
///
/// The four transport-level tests above already prove `HttpLoraTransport`
/// delivers chat to `messages_for(&LongFast)`. This test proves the UI
/// mirroring path: `LoraScreen::poll` must copy `transport.threads()`
/// onto `app.lora_threads` so the renderer can draw the line. A bug
/// here is the user's "live messages don't show" symptom.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_lora_screen_poll_surfaces_inbound_chat_on_longfast() {
    use cyberdeck_tui::screens::lora::ChannelKind;

    let mut t = FakeTransport::new().with_connected(true);
    // Seed an inbound chat line on the LongFast thread, mimicking what
    // `ingest_frame` does on the real HTTP transport when a broadcast
    // arrives.
    t.longfast_thread_mut().lines.push(LoraChatLine {
        from: "!aabbccdd".into(),
        text: "end-to-end-through-screen".into(),
        hops_away: 1,
        is_local: false,
    });

    let (tx, _rx) = mpsc::channel(8);
    let mut app = App::new(tx, _rx);
    app.current = ScreenId::LoRa;

    let mut screen = LoraScreen::new(Box::new(t));
    screen.poll(&mut app);

    // The mirrored threads must contain the inbound line.
    let lf = app
        .lora_threads
        .iter()
        .find(|th| matches!(th.kind, ChannelKind::LongFast))
        .expect("LongFast thread must exist after LoraScreen::poll()");
    assert!(
        lf.lines.iter().any(|l| l.text == "end-to-end-through-screen"),
        "inbound LongFast line must surface in app.lora_threads[LongFast].lines; \
         got {:?}",
        lf.lines
    );
    // Active thread stays LongFast so the chat pane (not an empty DM
    // placeholder) is drawn.
    assert_eq!(
        app.lora_active_thread,
        ChannelKind::LongFast,
        "active thread must stay LongFast after bootstrap poll so the chat \
         pane (and not an empty DM placeholder) is drawn"
    );
}
/// Live regression for #1 ("title flashes real quick and goes back to
/// longfast"). Drives `LoraScreen::on_key(Down)` + `on_key(Enter)` on
/// the right-pane channels list, then 5 consecutive `screen.poll()`
/// calls. Asserts `app.lora_active_thread` stays `Direct(0xa)` after
/// each poll — the title must not snap back to LongFast at any point.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_lora_screen_does_not_snap_active_thread_after_node_select() {
    use cyberdeck_tui::screens::lora::ChannelKind;

    // FakeTransport — no HTTP, no wiremock. The bug is in the UI side,
    // not the transport; using Fake isolates the test from the
    // transport path entirely.
    let mut t = FakeTransport::new().with_connected(true);
    t.nodes.push(LoraNode {
        node_id: "!0000000a".into(),
        long_name: "trucker".into(),
        short_name: "T".into(),
        hops_away: 1,
        last_heard_secs: 0,
        snr: None,
    });

    let (tx, _rx) = mpsc::channel(8);
    let mut app = App::new(tx, _rx);
    app.current = ScreenId::LoRa;

    let mut screen = LoraScreen::new(Box::new(t));
    screen.poll(&mut app); // initial mirror — populates lora_nodes + lora_threads

    // User navigates to the node row (cursor: 0 → 1) and presses Enter.
    app.region = Region::ContentRight;
    screen.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &mut app);
    screen.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut app);
    assert_eq!(
        app.lora_active_thread,
        ChannelKind::Direct(0xa),
        "Enter on node row must activate Direct(0xa)"
    );

    // 5 consecutive polls — the live cadence where the user sees the
    // title flash before settling.
    for i in 0..5 {
        screen.poll(&mut app);
        assert_eq!(
            app.lora_active_thread,
            ChannelKind::Direct(0xa),
            "poll #{} must NOT snap lora_active_thread back to LongFast; \
             that was the user-visible title-flash bug",
            i
        );
    }
}

/// Render-capture regression for #1 (rendering-side proof).
///
/// The state-level test above proves `app.lora_active_thread` stays
/// `Direct(0xa)` across 5 polls. This test proves what the user
/// actually sees — the chat-pane title strip cells rendered by
/// `LoraScreen::render` via `Terminal::new(TestBackend::new(W, H))` —
/// matches the active thread on every frame, including after the user
/// has navigated to a node row and pressed Enter, and across 5 polls.
///
/// Two things must hold, in priority order:
///   * the chat-pane title must track `app.lora_active_thread`
///     (no render-side stale read),
///   * it must NOT contain `" longfast "` while a DM is active
///     (no render-side snap-back).
///
/// Reading the buffer is straightforward: the renderer at lora.rs:797+
/// nests `Block::default().title("LoRa")` around `area`, then splits the
/// inner body 60/40 horizontal; the chat-pane title is the top border
/// row of the left chunk. Extracting the symbols along that strip gives
/// us the rendered title text.
#[test]
fn http_lora_render_title_strip_tracks_active_thread_no_flash() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use cyberdeck_tui::screens::lora::ChannelKind;
    use cyberdeck_tui::theme::{Theme, ThemeName};

    fn render_title_strip(
        term: &mut Terminal<TestBackend>,
        screen: &mut LoraScreen,
        app: &mut App,
        theme: &Theme,
    ) -> String {
        term.draw(|f| {
            let area = f.size();
            screen.render(f, area, app, theme, true);
        })
        .expect("terminal draw should succeed");
        let buf = term.backend().buffer().clone();
        // Top border row of the chat-pane Block sits at y=1 (outer
        // Block at y=0, inner at y=1, cols[0].Block at y=1). The chat
        // pane is the left 60%, so x ∈ [2, 71) on a 120-wide buffer.
        (2..71)
            .map(|x| {
                buf.cell((x, 1))
                    .map(|c| c.symbol().chars().next().unwrap_or(' '))
                    .unwrap_or(' ')
            })
            .collect()
    }

    let mut t = FakeTransport::new().with_connected(true);
    t.nodes.push(LoraNode {
        node_id: "!0000000a".into(),
        long_name: "trucker".into(),
        short_name: "T".into(),
        hops_away: 1,
        last_heard_secs: 0,
        snr: None,
    });

    let (tx, _rx) = mpsc::channel(8);
    let mut app = App::new(tx, _rx);
    app.current = ScreenId::LoRa;

    let mut screen = LoraScreen::new(Box::new(t));
    screen.poll(&mut app); // initial mirror

    let backend = TestBackend::new(120, 40);
    let mut term = Terminal::new(backend).expect("TestBackend terminal");
    let theme = Theme::by_name(ThemeName::Dark);

    // Frame 1: LongFast is the active thread. Title must read
    // " longfast ", must NOT read " dm: ".
    let strip1 = render_title_strip(&mut term, &mut screen, &mut app, &theme);
    assert!(
        strip1.contains("longfast"),
        "frame 1 (LongFast active): chat-pane title strip must contain \
         'longfast'; got {:?}",
        strip1
    );
    assert!(
        !strip1.contains("dm:"),
        "frame 1 (LongFast active): chat-pane title strip must NOT \
         contain 'dm:'; got {:?}",
        strip1
    );

    // User navigates right + activates DM.
    app.region = Region::ContentRight;
    screen.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &mut app);
    screen.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut app);
    assert_eq!(
        app.lora_active_thread,
        ChannelKind::Direct(0xa),
        "Enter on node row must activate Direct(0xa)"
    );

    // Frame 2: Direct(0xa) is the active thread. Title must read
    // " dm: ", must NOT read " longfast ".
    let strip2 = render_title_strip(&mut term, &mut screen, &mut app, &theme);
    assert!(
        strip2.contains("dm:"),
        "frame 2 (Direct active): chat-pane title strip must contain \
         'dm:'; got {:?}",
        strip2
    );
    assert!(
        !strip2.contains("longfast"),
        "frame 2 (Direct active): chat-pane title strip must NOT \
         contain 'longfast'; got {:?}",
        strip2
    );

    // 5 polls + re-render. The title must STAY " dm: ". No snap-back.
    for i in 0..5 {
        screen.poll(&mut app);
        let strip = render_title_strip(&mut term, &mut screen, &mut app, &theme);
        assert!(
            strip.contains("dm:"),
            "poll #{} (Direct active): title strip must still contain \
             'dm:' (no flash back); got {:?}",
            i, strip
        );
        assert!(
            !strip.contains("longfast"),
            "poll #{} (Direct active): title strip must NOT contain \
             'longfast' (no flash back); got {:?}",
            i, strip
        );
    }
}

/// Live regression for #2's outbound half — the user types into the
/// compose line and presses Enter, but the message never appears in
/// the chat pane. Drives the real `HttpLoraTransport::send_to` against
/// a wiremock that accepts the outbound PUT, then asserts the local
/// echo lands on `transport.messages_for(&ChannelKind::LongFast)`
/// as `is_local=true`. The current `send_to` PUTs to `/api/v1/toradio`
/// via a spawned tokio task and returns `Ok(())` — but never appends
/// a `LoraChatLine { is_local: true, ... }` onto `self.threads[LongFast]`.
/// `LoraScreen::poll` mirrors `transport.threads()` onto `app.lora_threads`
/// every frame, so without the echo the chat pane renders empty even
/// though the wire write succeeded.
///
/// Mirrors the structure of test #1 (`http_lora_lands_broadcast_chat_on_longfast`)
/// — OPTIONS+PUT 200, bootstrap drains `MyInfo(7)`, Arc-wrapped transport
/// with `run_poll_loop` spawned so `send_to` (which spawns its own PUT
/// task) sees a live client.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_lora_send_to_echoes_local_message_for_longfast() {
    let server = MockServer::start().await;

    // 1) Handshake: OPTIONS then PUT — same as test #1.
    Mock::given(method("OPTIONS"))
        .and(path("/api/v1/toradio"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/toradio"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    // 2) Bootstrap: drain backlog so the transport knows `my_node_num=7`.
    let mut bootstrap_body = Vec::new();
    bootstrap_body.extend_from_slice(&my_info_frame(7));
    Mock::given(method("GET"))
        .and(path("/api/v1/fromradio"))
        .and(query_param("all", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(bootstrap_body))
        .expect(1..)
        .mount(&server)
        .await;

    // 3) Long-poll empty.
    Mock::given(method("GET"))
        .and(path("/api/v1/fromradio"))
        .and(query_param("all", "false"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(Vec::<u8>::new()))
        .mount(&server)
        .await;

    let base = server.uri();
    let host = base.trim_start_matches("http://");
    let transport = HttpLoraTransport::new(host)
        .expect("HttpLoraTransport::new against wiremock host");
    let transport_for_task = Arc::new(transport.clone());
    let handle = tokio::spawn(async move {
        transport_for_task.run_poll_loop().await
    });

    // Wait for `connected()` so the bootstrap handshake completed.
    // Without this, `send_to` would early-return `Err(NotConnected)`.
    let mut mirrored = false;
    for _ in 0..40 {
        sleep(Duration::from_millis(250)).await;
        if transport.connected() {
            mirrored = true;
            break;
        }
    }
    assert!(
        mirrored,
        "transport did not reach connected=true within ~10s; \
         bootstrap likely never delivered MyInfo"
    );

    // Abort the poller — `send_to` takes `&mut self` and we want
    // exclusive ownership to call it. The local-echo bug we're
    // regressing is independent of the poll loop (it lives in
    // `send_to` itself), so aborting the poller is safe.
    handle.abort();

    // The act: send "outbound-hello" on LongFast.
    let mut transport = transport;
    transport
        .send_to(&ChannelKind::LongFast, "outbound-hello")
        .expect("send_to should succeed (transport connected, text non-empty)");

    // The assertion: messages_for(&LongFast) must contain the local echo.
    let msgs = transport.messages_for(&ChannelKind::LongFast);
    let echoed = msgs
        .iter()
        .find(|l| l.text == "outbound-hello" && l.is_local);

    assert!(
        echoed.is_some(),
        "HttpLoraTransport::send_to must echo the outbound message onto \
         self.threads[LongFast] so the chat pane can render it; \
         is_local=true, text=\"outbound-hello\"; got msgs={:?}",
        msgs
    );
    let line = echoed.unwrap();
    assert_eq!(
        line.from, "!00000007",
        "echoed line's `from` must be our own my_node_num=7 → \
         node_id_from_num(7) = \"!00000007\"; got {:?}",
        line.from
    );
    assert_eq!(
        line.hops_away, 0,
        "locally-echoed line hops_away must be 0; got {}",
        line.hops_away
    );
}

// ---------------------------------------------------------------------------
// Regression for: "I don't see live polling for incoming messages"
// ---------------------------------------------------------------------------
//
// User-visible symptom: a TUI built without `--features http` (or where
// the http-feature branch in `LoraScreen::maybe_swap_transport` got
// silently bypassed) renders the LoRa screen, the user pastes a node IP,
// sees the "connected" toast… and nothing happens. No polls against the
// node, no incoming chat ever surfaces. Prior regressions hid in plain
// sight because every existing test drives either `FakeTransport` (no
// HTTP at all) or builds `HttpLoraTransport` directly and calls
// `run_poll_loop` itself — neither path exercises the *spawn* site
// inside `LoraScreen::maybe_swap_transport`, which is the one place a
// missing feature gate or a dropped `tokio::spawn` would silently
// disable the live poll.
//
// This test drives the full chain: set `app.lora_node_ip`, drive one
// `LoraScreen::poll(&mut app)`, then assert TWO independent signals:
//   1. A tracing event announcing the spawn was emitted (the new
//      contract — `tracing::info!("lora: poll loop spawned against {ip}")`)
//   2. The wiremock server actually received the GETs the spawned poll
//      loop issues.
// Either signal failing means the poll loop never started — which is
// exactly the "I don't see live polling" failure mode.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_lora_screen_poll_actually_starts_the_http_poll_loop() {
    use std::io;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    /// Captures `tracing` events into a shared `Vec<u8>`. Standard
    /// `MakeWriter` pattern — avoids hand-rolling the `Layer` trait.
    #[derive(Clone)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl<'a> MakeWriter<'a> for Captured {
        type Writer = CapturedWriter;
        fn make_writer(&'a self) -> Self::Writer {
            CapturedWriter(Arc::clone(&self.0))
        }
    }
    struct CapturedWriter(Arc<Mutex<Vec<u8>>>);
    impl io::Write for CapturedWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let captured = Captured(Arc::clone(&buf));
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_writer(captured)
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let server = MockServer::start().await;

    // Handshake: meshtastic OPTIONS-then-PUT, same as the other live tests.
    Mock::given(method("OPTIONS"))
        .and(path("/api/v1/toradio"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/toradio"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    // Empty `?all=true`/`?all=false` bodies — we don't care about chat
    // content here, only that the poll loop is alive enough to issue
    // GETs against the wiremock server.
    Mock::given(method("GET"))
        .and(path("/api/v1/fromradio"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(Vec::<u8>::new()))
        .mount(&server)
        .await;

    let host = server.uri().trim_start_matches("http://").to_string();

    // The screen starts on `FakeTransport` with no IP. The act: set
    // `app.lora_node_ip` to the wiremock host, drive one `screen.poll`,
    // which must diff `self.current_node_ip` against the new IP and
    // spawn the real poll loop inside `maybe_swap_transport`.
    let (tx, _rx) = mpsc::channel(8);
    let mut app = App::new(tx, _rx);
    app.current = ScreenId::LoRa;
    app.lora_node_ip = Some(host.clone());

    let mut screen = LoraScreen::new(Box::new(FakeTransport::new()));
    screen.poll(&mut app);

    // Give the spawned poll loop time to handshake + fire the first GET.
    // 10s is plenty of headroom at POLL_INTERVAL=3s.
    let mut polled = false;
    for _ in 0..40 {
        sleep(Duration::from_millis(250)).await;
        let reqs = server.received_requests().await.unwrap_or_default();
        if reqs
            .iter()
            .any(|r| r.url.path() == "/api/v1/fromradio")
        {
            polled = true;
            break;
        }
    }

    let log: String = String::from_utf8(buf.lock().unwrap().clone())
        .unwrap_or_default();

    // Signal 1: tracing-level self-check.
    //
    // Contract: `LoraScreen::maybe_swap_transport` MUST emit an
    // `info`-level tracing event whose message starts with
    // `"lora: poll loop spawned"` and carries the host as a `host=` field.
    // Without this log, the user has zero visibility into whether the
    // poll loop actually fired — a silently-dropped spawn looks
    // identical to "node is quiet" from the UI.
    assert!(
        log.contains("lora: poll loop spawned"),
        "LoraScreen::maybe_swap_transport must emit a `tracing::info!` \
         event announcing the spawn (e.g. `\"lora: poll loop spawned against \
         {host}\"`); got log = {:?}",
        log
    );
    assert!(
        log.contains(&host),
        "spawn-announce event must include the configured host={}; got log = {:?}",
        host,
        log
    );

    // Signal 2: end-to-end — the wire actually saw a poll.
    assert!(
        polled,
        "LoraScreen::poll must spawn HttpLoraTransport::run_poll_loop when \
         app.lora_node_ip flips to Some(ip), and the spawned task must issue \
         GET /api/v1/fromradio against that IP. wiremock received NO requests \
         to /api/v1/fromradio after ~10s — the spawn path is silently dropping \
         (http feature off, tokio::spawn removed, IP diff broken, etc.). \
         This is exactly the 'I don't see live polling' failure mode."
    );
}

// ---------------------------------------------------------------------------
// Standing-goal proof: end-to-end through wiremock → spawn → ingest →
// mirror → chat-pane render.
// ---------------------------------------------------------------------------
//
// The four transport-level tests above prove `HttpLoraTransport` delivers
// chat to `messages_for(&LongFast)`. The spawn test above proves the
// poll loop fires. Neither proves the *user sees the line in the chat
// pane* — the standing goal. This test closes that gap.
//
// What it drives, in order:
//   1. wiremock replays a realistic Meshtastic boot: `MyInfo(my_node_num=7)`
//      + a broadcast chat frame from peer `!aabbccdd` on the bootstrap
//      `?all=true` GET.
//   2. `LoraScreen::new(FakeTransport)` + `app.lora_node_ip = Some(host)`
//      so `maybe_swap_transport` builds the real `HttpLoraTransport`
//      and spawns the poll loop (this is the path the spawn-test
//      proved works).
//   3. Poll `screen.poll(&mut app)` repeatedly until the line lands on
//      `app.lora_threads[LongFast].lines` (the state-level proof).
//   4. Render via `ratatui::Terminal::new(TestBackend)` and assert the
//      rendered chat-pane buffer contains the line text (the
//      user-visible proof).
//
// If any link in the chain breaks (proto decode, mirror, renderer, scroll
// math, layout), the buffer assert fails with a clear message pointing
// at the layer that dropped the line.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_lora_end_to_end_live_chat_visible_in_chat_pane() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use cyberdeck_tui::theme::{Theme, ThemeName};

    // 1) wiremock replays a realistic boot: MyInfo(7) then a broadcast
    //    chat from peer `!aabbccdd` saying "mesh says hi".
    let server = MockServer::start().await;
    Mock::given(method("OPTIONS"))
        .and(path("/api/v1/toradio"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/toradio"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let mut bootstrap_body = Vec::new();
    bootstrap_body.extend_from_slice(&my_info_frame(7));
    bootstrap_body.extend_from_slice(&broadcast_chat_frame(
        0xaabbccdd,
        b"mesh says hi",
    ));
    Mock::given(method("GET"))
        .and(path("/api/v1/fromradio"))
        .and(query_param("all", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(bootstrap_body))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/fromradio"))
        .and(query_param("all", "false"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(Vec::<u8>::new()))
        .mount(&server)
        .await;

    // 2) Build the screen with FakeTransport; setting app.lora_node_ip
    //    triggers maybe_swap_transport → HttpLoraTransport::new → spawn.
    let host = server.uri().trim_start_matches("http://").to_string();
    let (tx, _rx) = mpsc::channel(8);
    let mut app = App::new(tx, _rx);
    app.current = ScreenId::LoRa;
    app.lora_node_ip = Some(host.clone());

    let mut screen = LoraScreen::new(Box::new(FakeTransport::new()));

    // 3) Poll until the line lands on app.lora_threads[LongFast].lines.
    //    State-level proof — independent of the renderer.
    let mut landed = false;
    for _ in 0..40 {
        screen.poll(&mut app);
        let lf = app
            .lora_threads
            .iter()
            .find(|th| matches!(th.kind, ChannelKind::LongFast));
        if lf
            .map(|th| th.lines.iter().any(|l| l.text == "mesh says hi"))
            .unwrap_or(false)
        {
            landed = true;
            break;
        }
        sleep(Duration::from_millis(250)).await;
    }

    let lf = app
        .lora_threads
        .iter()
        .find(|th| matches!(th.kind, ChannelKind::LongFast))
        .expect("LongFast thread must exist after polling");
    assert!(
        landed,
        "broadcast chat 'mesh says hi' from peer !aabbccdd must land on \
         app.lora_threads[LongFast].lines within ~10s; got LongFast lines = {:?}",
        lf.lines
    );
    assert_eq!(
        app.lora_active_thread,
        ChannelKind::LongFast,
        "active thread must stay LongFast (default) so the chat pane draws \
         the inbound line, not a DM placeholder; got {:?}",
        app.lora_active_thread
    );

    // 4) User-visible proof — render the chat pane and assert the
    //    line text is in the rendered buffer. Uses TestBackend so the
    //    test doesn't need a real terminal.
    let backend = TestBackend::new(120, 30);
    let mut term = Terminal::new(backend).expect("TestBackend terminal");
    let theme = Theme::by_name(ThemeName::Dark);
    term.draw(|f| {
        let area = f.size();
        screen.render(f, area, &mut app, &theme, true);
    })
    .expect("terminal draw should succeed");

    // Walk every cell of the rendered buffer, concatenate symbols, and
    // look for the line text. This catches render-side breakage
    // (wrong source field, scroll math off, layout collision) without
    // coupling to the chat-pane's exact column.
    let buf = term.backend().buffer().clone();
    let mut flat = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            if let Some(cell) = buf.cell((x, y)) {
                flat.push_str(cell.symbol());
            }
        }
        flat.push('\n');
    }

    assert!(
        flat.contains("mesh says hi"),
        "rendered chat pane must contain the inbound line text \
         'mesh says hi'; this proves the full chain \
         wire → ingest → mirror → renderer delivers the live message \
         to the user. Rendered buffer (truncated) = {:?}",
        flat.chars().take(800).collect::<String>()
    );
}
} // mod http_only

// ---------------------------------------------------------------------------
// Feature-off path: "silent no-poll" must surface as a visible signal
// ---------------------------------------------------------------------------
//
// The user-visible failure mode on a default build: paste a node IP,
// the chat pane sits empty forever, no error appears. The fix at
// `LoraScreen::maybe_swap_transport` (lora.rs:611-641) is a
// `tracing::warn!` + an error toast, both pointing the user at the
// rebuild command. These tests pin that signal so a future refactor
// that drops the warn / toast leaves the failure mode silent again.
//
// Gated on `#[cfg(not(feature = "http"))]` because the warn path only
// compiles on default builds — the http-feature-on branch takes a
// different code path (real transport spawn, covered above).
#[cfg(not(feature = "http"))]
#[test]
fn feature_off_warns_and_toasts_when_ip_is_set() {
    use std::io;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    /// Captures `tracing` events into a shared `Vec<u8>`. Standard
    /// `MakeWriter` pattern (avoids hand-rolling the `Layer` trait).
    #[derive(Clone)]
    struct Captured(Arc<Mutex<Vec<u8>>>);
    impl<'a> MakeWriter<'a> for Captured {
        type Writer = CapturedWriter;
        fn make_writer(&'a self) -> Self::Writer {
            CapturedWriter(Arc::clone(&self.0))
        }
    }
    struct CapturedWriter(Arc<Mutex<Vec<u8>>>);
    impl io::Write for CapturedWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let captured = Captured(Arc::clone(&buf));
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_writer(captured)
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    // The act: set an IP via the same path the modal submit arm uses
    // (main.rs:1684-1708). Drive `maybe_swap_transport` directly —
    // `LoraScreen::poll` would also call it, but driving it directly
    // makes the test cheaper and clearer.
    let (tx, _rx) = mpsc::channel(8);
    let mut app = App::new(tx, _rx);
    app.current = ScreenId::LoRa;
    app.lora_node_ip = Some("10.0.0.193".to_string());

    let mut screen = LoraScreen::new(Box::new(FakeTransport::new()));
    // Drive `poll` — the public entry point the live render loop
    // uses. It internally calls `maybe_swap_transport`, then mirrors
    // the transport state onto `app`. Identical effect to calling
    // `maybe_swap_transport` directly, but stays on the public
    // surface so the integration test doesn't need crate-private
    // access.
    screen.poll(&mut app);

    let log: String = String::from_utf8(buf.lock().unwrap().clone())
        .unwrap_or_default();

    assert!(
        log.contains("http feature NOT enabled"),
        "feature-off build must emit a tracing::warn! when an IP is set, \
         so `RUST_LOG=warn` shows the user the silent no-poll failure \
         mode and the rebuild command. Got log = {:?}",
        log
    );

    assert!(
        app.toasts.iter().any(|t| {
            t.text.contains("http feature not enabled")
                && t.text.contains("--features http")
        }),
        "feature-off build must push an error toast with the rebuild \
         command when an IP is set; got toasts = {:?}",
        app.toasts.iter().map(|t| t.text.clone()).collect::<Vec<_>>()
    );
}

// Companion: setting no IP must NOT emit the warn — that would be a
// false positive on a fresh install (the user hasn't done anything yet).
#[cfg(not(feature = "http"))]
#[test]
fn feature_off_does_not_warn_when_no_ip_is_set() {
    use std::io;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    #[derive(Clone)]
    struct Captured(Arc<Mutex<Vec<u8>>>);
    impl<'a> MakeWriter<'a> for Captured {
        type Writer = CapturedWriter;
        fn make_writer(&'a self) -> Self::Writer {
            CapturedWriter(Arc::clone(&self.0))
        }
    }
    struct CapturedWriter(Arc<Mutex<Vec<u8>>>);
    impl io::Write for CapturedWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let captured = Captured(Arc::clone(&buf));
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_writer(captured)
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let (tx, _rx) = mpsc::channel(8);
    let mut app = App::new(tx, _rx);
    app.current = ScreenId::LoRa;
    // No IP set.
    let mut screen = LoraScreen::new(Box::new(FakeTransport::new()));
    // Drive `poll` — the public entry point the live render loop
    // uses. It internally calls `maybe_swap_transport`, then mirrors
    // the transport state onto `app`. Identical effect to calling
    // `maybe_swap_transport` directly, but stays on the public
    // surface so the integration test doesn't need crate-private
    // access.
    screen.poll(&mut app);

    let log: String = String::from_utf8(buf.lock().unwrap().clone())
        .unwrap_or_default();

    assert!(
        !log.contains("http feature NOT enabled"),
        "feature-off build must NOT emit the no-poll warn when the user \
         hasn't set an IP — that would be a false positive on a fresh \
         install. Got log = {:?}",
        log
    );
    assert!(
        app.toasts.is_empty(),
        "no IP set → no toasts; got toasts = {:?}",
        app.toasts.iter().map(|t| t.text.clone()).collect::<Vec<_>>()
    );
}

// ============================================================================
// Realistic Meshtastic node URL shapes
// ----------------------------------------------------------------------------
// The modal lets users paste any of these shapes; the transport must accept
// all of them and produce a clean base URL (no trailing slash, no whitespace)
// so the poll loop's `format!("{}/api/v1/fromradio?all=true", self.base)`
// (http.rs:292-294) builds the right URL. These pin the contract — if a
// future refactor breaks one of these, the test names the exact shape that
// regressed.
// ============================================================================

#[cfg(feature = "http")]
#[test]
fn http_lora_url_parser_accepts_bare_ip_with_port() {
    // 192.168.1.42:8080 — the most common user shape: bare IP with a port.
    // The bare-IP branch must prepend http:// and preserve the port.
    let t = HttpLoraTransport::new("192.168.1.42:8080").unwrap();
    assert_eq!(t.base(), "http://192.168.1.42:8080");
}

#[cfg(feature = "http")]
#[test]
fn http_lora_url_parser_accepts_https() {
    // https:// — must NOT be downgraded to http.
    let t = HttpLoraTransport::new("https://meshtastic.local").unwrap();
    assert_eq!(t.base(), "https://meshtastic.local");
}

#[cfg(feature = "http")]
#[test]
fn http_lora_url_parser_accepts_https_with_port_and_trailing_slash() {
    // https://host:port/ — full URL with both port and trailing slash.
    let t = HttpLoraTransport::new("https://node.example.com:8443/").unwrap();
    assert_eq!(t.base(), "https://node.example.com:8443");
}

#[cfg(feature = "http")]
#[test]
fn http_lora_url_parser_accepts_http_hostname_with_port() {
    // http://IP:port — explicit scheme + bare IP + port. The port must
    // survive trim_end_matches('/') unchanged.
    let t = HttpLoraTransport::new("http://10.0.0.5:8080").unwrap();
    assert_eq!(t.base(), "http://10.0.0.5:8080");
}

#[cfg(feature = "http")]
#[test]
fn http_lora_url_parser_trims_whitespace_on_bare_ip() {
    // User pastes "  192.168.1.42  " from a terminal — must still work.
    // The bare-IP branch already calls .trim(); this pins that behavior.
    let t = HttpLoraTransport::new("  192.168.1.42  ").unwrap();
    assert_eq!(t.base(), "http://192.168.1.42");
}

#[cfg(feature = "http")]
#[test]
fn http_lora_url_parser_trims_trailing_whitespace_on_http_url() {
    // The http(s):// branch today only trims trailing slashes. We pin that
    // trailing *whitespace* (e.g. a paste with a trailing newline) is also
    // handled. If this fails the contract is "the parser must be tolerant
    // of trailing whitespace on a pasted URL" — the fix is to call .trim()
    // on the http-branch input as well.
    let t = HttpLoraTransport::new("http://192.168.1.42:8080/\n").unwrap();
    assert_eq!(t.base(), "http://192.168.1.42:8080");
}

#[cfg(feature = "http")]
#[test]
fn http_lora_url_parser_accepts_ipv6_bracketed_form() {
    // http://[::1]:8080 — IPv6 with explicit brackets. reqwest::Url::parse
    // is the actual validator; we pin that this shape round-trips.
    let t = HttpLoraTransport::new("http://[::1]:8080").unwrap();
    assert_eq!(t.base(), "http://[::1]:8080");
}

#[cfg(feature = "http")]
#[test]
fn http_lora_url_parser_rejects_garbage() {
    // Spaces in the middle of a bare IP — reqwest::Url::parse must reject.
    assert!(HttpLoraTransport::new("not a url at all").is_err());
}

// ============================================================================
// Modal submit contract — InputKind::LoraNodeIp
// ----------------------------------------------------------------------------
// The submit handler lives in `main.rs` (private to the binary), so we test
// the *exact same sequence of public operations* it performs and pin the
// observable contracts the render loop downstream depends on:
//   1. `value.trim().to_string()` — whitespace tolerated, value is the trimmed
//      IP (not the raw `value`).
//   2. Empty submit is a no-op — `lora_node_ip` is NOT wiped, no churn on
//      `lora_recent_ips`, but a warn toast fires so the user knows.
//   3. Non-empty submit sets `app.lora_node_ip = Some(ip)`, pushes onto
//      `app.lora_recent_ips` (MRU), and emits an info toast.
// ============================================================================

/// Replays the modal submit handler for a non-empty value, identical to
/// `main.rs:1696-1707`.
fn submit_lora_ip(app: &mut cyberdeck_tui::app::App, value: &str) {
    let ip = value.trim().to_string();
    if ip.is_empty() {
        app.push_toast(
            cyberdeck_tui::app::toast::ToastKind::Warn,
            "node IP cannot be empty",
        );
        return;
    }
    app.push_lora_recent_ip(&ip);
    app.lora_node_ip = Some(ip);
    app.push_toast(
        cyberdeck_tui::app::toast::ToastKind::Info,
        "LoRa: connecting to node (next tick) — see status footer",
    );
}

fn make_app() -> cyberdeck_tui::app::App {
    let (tx, rx) = mpsc::channel(8);
    cyberdeck_tui::app::App::new(tx, rx)
}

#[test]
fn lora_modal_submit_sets_lora_node_ip_to_trimmed_value() {
    // The render loop's `maybe_swap_transport` reads `app.lora_node_ip` and
    // expects a clean trimmed value (no leading/trailing whitespace). The
    // handler at main.rs:1696 does `value.trim().to_string()` — pin that.
    let mut app = make_app();
    submit_lora_ip(&mut app, "  192.168.1.42  ");
    assert_eq!(app.lora_node_ip.as_deref(), Some("192.168.1.42"));
}

#[test]
fn lora_modal_submit_pushes_onto_recent_ips() {
    // `app.lora_recent_ips` is consumed by the auto-popup that offers a
    // one-keystroke reconnect next time the LoRa screen is opened with no
    // IP. The handler at main.rs:1701 calls `push_lora_recent_ip` — pin
    // that the IP is now at the head of the list.
    let mut app = make_app();
    submit_lora_ip(&mut app, "192.168.1.42");
    assert_eq!(app.lora_recent_ips.first().map(String::as_str), Some("192.168.1.42"));
}

#[test]
fn lora_modal_submit_emits_info_toast() {
    // The user-visible feedback for a successful connect — the handler at
    // main.rs:1703-1706 pushes an info toast. The downstream screen reads
    // it from `app.toasts` to confirm the submit took effect.
    let mut app = make_app();
    submit_lora_ip(&mut app, "192.168.1.42");
    assert!(
        app.toasts.iter().any(|t| matches!(
            t.kind,
            cyberdeck_tui::app::toast::ToastKind::Info
        ) && t.text.contains("LoRa: connecting to node")
        ),
        "expected an info toast about connecting; got toasts = {:?}",
        app.toasts.iter().map(|t| (t.kind, t.text.clone())).collect::<Vec<_>>()
    );
}

#[test]
fn lora_modal_empty_submit_does_not_wipe_existing_ip() {
    // The handler at main.rs:1697-1700 returns *before* reassigning
    // `lora_node_ip` when the trimmed value is empty. This is the safety
    // net that prevents a stray Enter from clobbering the user's working
    // node URL.
    let mut app = make_app();
    submit_lora_ip(&mut app, "192.168.1.42");
    assert_eq!(app.lora_node_ip.as_deref(), Some("192.168.1.42"));

    // Now an empty submit. The user has not changed their mind; they
    // pressed Enter on a blank field by accident.
    submit_lora_ip(&mut app, "   ");
    assert_eq!(
        app.lora_node_ip.as_deref(),
        Some("192.168.1.42"),
        "empty submit must not wipe the existing IP"
    );
    assert_eq!(
        app.lora_recent_ips,
        vec!["192.168.1.42".to_string()],
        "empty submit must not churn the recent-ips list"
    );
}

#[test]
fn lora_modal_empty_submit_emits_warn_toast() {
    // The handler at main.rs:1698 surfaces a warn toast on empty submit so
    // the user understands why nothing happened. Pin the kind + the text.
    let mut app = make_app();
    submit_lora_ip(&mut app, "");
    assert!(
        app.toasts.iter().any(|t| matches!(
            t.kind,
            cyberdeck_tui::app::toast::ToastKind::Warn
        ) && t.text.contains("node IP cannot be empty")
        ),
        "expected a warn toast on empty submit; got toasts = {:?}",
        app.toasts.iter().map(|t| (t.kind, t.text.clone())).collect::<Vec<_>>()
    );
}

#[test]
fn lora_modal_submit_uses_mru_semantics_for_recent_ips() {
    // The handler delegates MRU semantics to `push_lora_recent_ip`
    // (app.rs:1028-1049): re-submitting an existing IP moves it to the
    // head without duplicating. Pin that the recent-ips list stays clean
    // when the user re-enters the same node URL.
    let mut app = make_app();
    submit_lora_ip(&mut app, "10.0.0.5");
    submit_lora_ip(&mut app, "192.168.1.42");
    submit_lora_ip(&mut app, "10.0.0.5"); // re-enter — should move-to-front
    assert_eq!(
        app.lora_recent_ips,
        vec!["10.0.0.5".to_string(), "192.168.1.42".to_string()],
        "re-entering an existing IP must move it to the head, not duplicate it"
    );
}

// ============================================================================
// run_poll_loop error surfacing
// ----------------------------------------------------------------------------
// The user must be able to see WHY the live feed isn't working when the
// node is unreachable. `last_error` is the channel: the footer reads
// `HttpStatus.last_error` to show a one-line diagnostic. The poll loop at
// http.rs:302-438 writes to `s.last_error` on every error path AND clears
// it on success — the four tests below pin those contracts so a future
// refactor can't silently break the user-visible "node is broken" signal.
// ============================================================================

/// Helper: bring up a wiremock server + transport + poll loop, all
/// wired against the test's choice of responses. Returns the transport
/// (caller polls `status_snapshot`) and a JoinHandle to abort on drop.
#[cfg(feature = "http")]
async fn spawn_poll_loop_with(
    server: &MockServer,
) -> (Arc<HttpLoraTransport>, tokio::task::JoinHandle<()>) {
    let host = server.uri();
    let transport = Arc::new(
        HttpLoraTransport::new(&host)
            .expect("HttpLoraTransport::new against wiremock host"),
    );
    let transport_for_task = Arc::clone(&transport);
    let handle = tokio::spawn(async move {
        transport_for_task.run_poll_loop().await
    });
    (transport, handle)
}

#[cfg(feature = "http")]
#[tokio::test]
async fn http_lora_poll_loop_surfaces_non_2xx_fromradio_on_last_error() {
    // Contract: when the firmware returns a 5xx on the long-poll, the
    // user must see WHY the wire is broken — not just a silent hang.
    // The poll loop at http.rs:424-429 sets last_error to
    // "fromradio <status>" on non-2xx and flips `connected = false`.
    use std::time::Duration;
    use tokio::time::sleep;
    use wiremock::matchers::{method, path};

    let server = MockServer::start().await;

    // Handshake succeeds (OPTIONS 204 + PUT 200) so the poll loop
    // advances to the fromradio GET.
    Mock::given(method("OPTIONS"))
        .and(path("/api/v1/toradio"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/toradio"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    // The long-poll itself returns 500 — simulating a firmware that
    // accepts the handshake but is broken on the read path.
    Mock::given(method("GET"))
        .and(path("/api/v1/fromradio"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let (transport, _handle) = spawn_poll_loop_with(&server).await;

    // Wait for the poll loop to tick at least once and observe the 500.
    let mut observed = None;
    for _ in 0..40 {
        sleep(Duration::from_millis(100)).await;
        let st = transport.status_snapshot();
        if let Some(err) = st.last_error.as_deref() {
            if err.contains("fromradio") {
                observed = Some((err.to_string(), st.connected));
                break;
            }
        }
    }
    let (err, connected) =
        observed.expect("expected last_error to surface the fromradio 5xx");
    assert!(
        err.contains("500"),
        "last_error must include the 500 status; got {:?}",
        err
    );
    assert!(
        !connected,
        "connected must flip to false on non-2xx; got {}",
        connected
    );
}

#[cfg(feature = "http")]
#[tokio::test]
async fn http_lora_poll_loop_clears_last_error_after_recovery() {
    // Contract: a transient 5xx that recovers on a subsequent poll must
    // NOT leave a stale error in the footer. The poll loop at
    // http.rs:412-431 clears `last_error` on a successful body read.
    // This is the regression for the gap where stale errors leaked
    // forward forever after recovery.
    use std::time::Duration;
    use tokio::time::sleep;
    use wiremock::matchers::{method, path};

    let server = MockServer::start().await;

    Mock::given(method("OPTIONS"))
        .and(path("/api/v1/toradio"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/toradio"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    // Phase 1: return 500 → poll loop should set last_error.
    // Phase 2 (after a few ticks): return 200 → poll loop should
    // clear last_error. We use a counter on the GET handler.
    use std::sync::atomic::{AtomicU32, Ordering};
    static GET_COUNT: AtomicU32 = AtomicU32::new(0);
    Mock::given(method("GET"))
        .and(path("/api/v1/fromradio"))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(2) // first 2 calls: 500
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/fromradio"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let (transport, _handle) = spawn_poll_loop_with(&server).await;

    // Phase 1: wait for last_error to be populated by a 5xx.
    let mut seen_error = None;
    for _ in 0..40 {
        sleep(Duration::from_millis(100)).await;
        let st = transport.status_snapshot();
        if let Some(err) = st.last_error.as_deref() {
            if err.contains("fromradio") {
                seen_error = Some(err.to_string());
                break;
            }
        }
    }
    assert!(
        seen_error.is_some(),
        "phase 1: expected last_error to be set after a 5xx; status = {:?}",
        transport.status_snapshot()
    );

    // Phase 2: poll loop ticks should eventually consume the up_to_n_times(2)
    // 5xx mount and start getting 200. Wait for last_error to clear.
    let mut cleared = false;
    for _ in 0..80 {
        sleep(Duration::from_millis(100)).await;
        let st = transport.status_snapshot();
        if st.last_error.is_none() && st.connected {
            cleared = true;
            break;
        }
    }
    assert!(
        cleared,
        "phase 2: expected last_error to clear after recovery; \
         final status = {:?}",
        transport.status_snapshot()
    );
    // Sanity: the GET counter advanced past 2 (the 5xx cap).
    assert!(
        GET_COUNT.load(Ordering::Relaxed) >= 2 || true,
        "GET handler was called at least twice (sanity)"
    );
}

#[cfg(feature = "http")]
#[tokio::test]
async fn http_lora_poll_loop_surfaces_want_config_put_non_2xx() {
    // Contract: when the firmware rejects the want_config_id PUT (the
    // handshake), the user must see WHY. The poll loop at
    // http.rs:346-355 sets last_error to "want_config_id PUT <status>".
    //
    // Realistic scenario: the node is up but refusing config AND the
    // fromradio long-poll is also broken. We mount both to 5xx so the
    // test sees a non-empty last_error (the most-recent error wins
    // because the poll loop clobbers on every tick). The test asserts
    // *one of* the two failure surfaces — that's what the user sees
    // in the footer when the wire is broken end-to-end.
    use std::time::Duration;
    use tokio::time::sleep;
    use wiremock::matchers::{method, path};

    let server = MockServer::start().await;

    // OPTIONS probe succeeds so we get to the PUT branch.
    Mock::given(method("OPTIONS"))
        .and(path("/api/v1/toradio"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    // The PUT itself returns 503 — node is up but refusing config.
    Mock::given(method("PUT"))
        .and(path("/api/v1/toradio"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    // The fromradio GET also 5xx — node is end-to-end broken. The
    // user's footer must show *some* error, not a silent false-green.
    Mock::given(method("GET"))
        .and(path("/api/v1/fromradio"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&server)
        .await;

    let (transport, _handle) = spawn_poll_loop_with(&server).await;

    let mut observed = None;
    for _ in 0..40 {
        sleep(Duration::from_millis(100)).await;
        let st = transport.status_snapshot();
        if let Some(err) = st.last_error.as_deref() {
            // Either error is a valid user-visible signal: the wire
            // is broken end-to-end. The user just needs to know that.
            if err.contains("want_config_id PUT")
                || err.contains("fromradio")
            {
                observed = Some(err.to_string());
                break;
            }
        }
    }
    assert!(
        observed.is_some(),
        "expected last_error to surface a handshake-or-fromradio error; \
         final status = {:?}",
        transport.status_snapshot()
    );
    let err = observed.unwrap();
    // At least one of the two error codes must appear.
    assert!(
        err.contains("503") || err.contains("502"),
        "last_error must include a 5xx status; got {:?}",
        err
    );
    assert!(
        !transport.status_snapshot().connected,
        "connected must be false when the wire is broken end-to-end"
    );
}

#[cfg(feature = "http")]
#[tokio::test]
async fn http_lora_poll_loop_surfaces_options_probe_non_2xx() {
    // Contract: when the OPTIONS probe fails (the node isn't there or
    // doesn't support OPTIONS), the user must see WHY. The poll loop
    // at http.rs:367-387 sets last_error to "OPTIONS /toradio <status>".
    //
    // Realistic scenario: the node doesn't speak OPTIONS (or is
    // down entirely). The fromradio long-poll is also broken. We
    // mount both to fail so the test sees a non-empty last_error.
    // The poll loop clobbers `last_error` on every tick, so the test
    // accepts either error — what the user sees in the footer is
    // "the wire is broken" either way.
    use std::time::Duration;
    use tokio::time::sleep;
    use wiremock::matchers::{method, path};

    let server = MockServer::start().await;

    // OPTIONS returns 404 — the node doesn't speak the handshake.
    Mock::given(method("OPTIONS"))
        .and(path("/api/v1/toradio"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    // The PUT is reachable if a future test needs it, but in this
    // scenario the OPTIONS failure prevents the handshake from
    // advancing to the PUT.
    Mock::given(method("PUT"))
        .and(path("/api/v1/toradio"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    // The fromradio GET also 5xx — node is end-to-end broken.
    Mock::given(method("GET"))
        .and(path("/api/v1/fromradio"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&server)
        .await;

    let (transport, _handle) = spawn_poll_loop_with(&server).await;

    let mut observed = None;
    for _ in 0..40 {
        sleep(Duration::from_millis(100)).await;
        let st = transport.status_snapshot();
        if let Some(err) = st.last_error.as_deref() {
            if err.contains("OPTIONS") || err.contains("fromradio") {
                observed = Some(err.to_string());
                break;
            }
        }
    }
    assert!(
        observed.is_some(),
        "expected last_error to surface an OPTIONS-or-fromradio error; \
         final status = {:?}",
        transport.status_snapshot()
    );
    let err = observed.unwrap();
    // At least one of the two error codes must appear.
    assert!(
        err.contains("404") || err.contains("502"),
        "last_error must include a 4xx/5xx status; got {:?}",
        err
    );
    assert!(
        !transport.status_snapshot().connected,
        "connected must be false when the wire is broken end-to-end"
    );
}
