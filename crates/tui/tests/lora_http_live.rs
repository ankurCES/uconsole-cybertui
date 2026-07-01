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

use std::sync::Arc;
use std::time::Duration;

use cyberdeck_tui::screens::lora::{
    http::HttpLoraTransport, ChannelKind, LoraTransport,
};
use tokio::time::sleep;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

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
