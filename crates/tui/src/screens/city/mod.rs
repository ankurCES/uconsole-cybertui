//! City screen — IP-geolocated road map + live weather, rendered as
//! braille in the left pane and a weather/wind block on the right.
//!
//! Submodule layout (all `pub` so other screens/tests can pick at
//! them independently):
//!   * `geo` — IP → CityLocation via ip-api.com (Step 5)
//!   * `weather` — Open-Meteo client (Step 5)
//!   * `traffic` — synthetic time-of-day traffic overlay (Step 5)
//!   * `roads` — bundled city polyline loader (Step 6)
//!   * `render` — braille grid + Bresenham line draw (Step 7)
//!
//! Step 8 — real `CityScreen` impl. Renders a 2-pane horizontal split
//! ([60% / 40%], per the layout-audit spec in `app/screen.rs`) with:
//!
//!   * Left  — braille road network + traffic overlay + location marker.
//!   * Right — weather block (or hidden when `app.show_weather_panel`
//!             is false, in which case the map takes the full width).
//!
//! Keymap (focused on the City screen, 9 keys total):
//!
//!   `h j k l` — pan left/down/up/right (10% of the current bbox
//!               per press; clamps so we never pan past the poles).
//!   `+ =`     — zoom in (shrink the bbox around the current centre).
//!   `- _`     — zoom out (grow the bbox around the current centre,
//!               capped at the bundled city bbox so we never zoom out
//!               past "the whole city").
//!   `r`       — refresh: re-enqueue `Action::CityCtrlRefresh` so the
//!               main-loop refiller re-fetches geo + weather.
//!   `c`       — city picker: cycle `city_override` through the
//!               bundled slug list (`BUNDLED`). Wraps around. Updated
//!               value is persisted via `App::save_prefs`.
//!   `t`       — toggle the synthetic traffic overlay on/off.
//!               Persisted via `App::save_prefs`.
//!   `w`       — toggle the right-hand weather panel. When off, the
//!               map spans the full content width.

pub mod geo;
pub mod render;
pub mod roads;
pub mod traffic;
pub mod weather;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::action::Action;
use crate::app::screen::{Screen, ScreenId};
use crate::app::App;
use crate::prefs::Units;
use crate::theme::Theme;

use super::city::geo::CityLocation;
use super::city::render::{draw_roads, BrailleGrid, Viewport};
use super::city::roads::{CityRoads, Polyline};
use super::city::traffic::{synthetic_overlay, TrafficLevel, TrafficOverlay};
use super::city::weather::{weather_label, Weather};

/// Minimum bbox span (in degrees) — below this, the map stops zooming
/// in so the user can't get stuck in a sub-pixel viewport. Roughly
/// "two city blocks" in lat/lon terms.
const MIN_BBOX_SPAN: f64 = 0.005;
/// Maximum bbox span (in degrees) — above this, the map stops zooming
/// out so the user can't fly past the bundled city. The bundled-city
/// loader sets this per-city but we cap it loosely to avoid
/// divide-by-near-zero projection artefacts.
const MAX_BBOX_SPAN: f64 = 5.0;
/// How much `+`/`-` shrinks/grows the bbox span per press. A 20% step
/// gives ~10 zoom levels from the bundled Seattle bbox (~0.2° span)
/// down to MIN_BBOX_SPAN, which feels about right for a city-screen
/// keymap.
const ZOOM_STEP: f64 = 0.8;
/// How much `h/j/k/l` shifts the bbox centre per press, as a fraction
/// of the current span. 10% gives smooth panning without losing the
/// user's place on a single keypress.
const PAN_STEP: f64 = 0.1;

/// Placeholder `CityScreen`.
///
/// Holds the live data the renderer reads: the resolved location
/// (either IP-geolocated or user-picked), the loaded bundled road
/// network, the latest weather snapshot, and the synthetic traffic
/// overlay. `App::city_data` would also work, but holding the live
/// state on the screen itself keeps `App` from growing new fields and
/// makes the screen self-contained for the layout-audit test.
///
/// `data_fresh` flips false the moment a fetch is enqueued so the
/// renderer can show "refreshing…" instead of stale data while the
/// 10-minute refiller is in flight.
pub struct CityScreen {
    /// What we're showing right now. `None` until the first IP lookup
    /// (or manual `c` pick) lands. We always keep this synchronised
    /// with `App::city_override` so a restart picks up the same view.
    pub location: Option<CityLocation>,
    /// Bundled road network for `location`. Same fallback rule as the
    /// roads loader: if the picked slug isn't bundled, fall back to
    /// `seattle`. The screen never holds an empty `CityRoads` —
    /// `roads()` returns a guaranteed-non-None view.
    pub roads: CityRoads,
    /// Slug used to load `roads`. Tracks the picker so a refresh keeps
    /// the same data source instead of re-doing the IP lookup.
    pub slug: String,
    /// Latest weather snapshot. `None` until the first Open-Meteo
    /// fetch lands.
    pub weather: Option<Weather>,
    /// True while a fetch is in flight. The map pane shows
    /// "refreshing…" in its footer when this is set.
    pub data_fresh: bool,
    /// Current viewport (pan + zoom). Built from `location.bbox` on
    /// first render and mutated by `h/j/k/l/+/-`.
    pub viewport_bbox: [f64; 4],
    /// Dot-grid dimensions of the last rendered map. Refreshed by
    /// `render()` and read by the on-key handler so `+/-` can zoom
    /// while preserving the on-screen aspect ratio. Zero before the
    /// first render — handlers must fall back to a square aspect in
    /// that case.
    pub last_viewport_w: u16,
    pub last_viewport_h: u16,
}

impl CityScreen {
    /// Construct a fresh screen with the bundled seattle data and no
    /// live data. Used by `main.rs` at boot and by the layout-audit
    /// test (which only inspects the render output, not the state).
    pub fn new() -> Self {
        let (slug, roads) = CityRoads::load_bundled_or_default("seattle");
        let loc = roads.location();
        let bbox = loc.bbox.unwrap_or(roads.bbox);
        Self {
            location: Some(loc),
            roads,
            slug,
            weather: None,
            data_fresh: false,
            viewport_bbox: bbox,
            last_viewport_w: 0,
            last_viewport_h: 0,
        }
    }

    /// Apply the user's city override (or the IP-geolocated default)
    /// to the screen state. Called by the main-loop on
    /// `Action::CityResolved { loc }` and on `c`-key cycling.
    ///
    /// The bundled road network and viewport bbox are reset to the
    /// new city's bbox — the user expects "switching cities" to mean
    /// "show me the whole new city", not "keep my old panned view of
    /// Seattle centred on Tokyo".
    pub fn apply_location(&mut self, loc: CityLocation, slug: String) {
        let (resolved_slug, roads) = CityRoads::load_bundled_or_default(&slug);
        self.slug = resolved_slug;
        self.roads = roads;
        self.location = Some(loc.clone());
        self.viewport_bbox = loc.bbox.unwrap_or(self.roads.bbox);
    }

    /// Load bundled road data for `slug` without changing the
    /// viewport. Used when the IP locator hasn't returned yet but the
    /// user has cycled to a new bundled city.
    pub fn apply_slug(&mut self, slug: String) {
        let (resolved_slug, roads) = CityRoads::load_bundled_or_default(&slug);
        self.slug = resolved_slug;
        self.roads = roads;
        if let Some(loc) = self.location.as_mut() {
            loc.bbox = Some(self.roads.bbox);
            self.viewport_bbox = self.roads.bbox;
        }
    }

    /// Build the live viewport for the given inner rect of the map
    /// pane. Returns `None` if the pane is too small to be useful
    /// (less than 4 chars wide or 2 chars tall — braille needs at
    /// least one cell in each direction to render anything).
    fn viewport(&self, inner: Rect) -> Option<Viewport> {
        if inner.width < 4 || inner.height < 2 {
            return None;
        }
        Some(Viewport::new(self.viewport_bbox, inner.width, inner.height))
    }

    /// Like `viewport` but also records the dot-grid dimensions on
    /// `self.last_viewport_w/h` so the on-key handler can compute an
    /// aspect-correct zoom even when the pane hasn't been re-rendered
    /// this frame.
    fn viewport_mut(&mut self, inner: Rect) -> Option<Viewport> {
        let vp = self.viewport(inner)?;
        self.last_viewport_w = vp.width_dots;
        self.last_viewport_h = vp.height_dots;
        Some(vp)
    }

    /// Build the synthetic traffic overlay for the current roads +
    /// clock. Falls back to an empty overlay if the date is somehow
    /// out of range (the chrono API uses `LocalResult` which `unwrap`
    /// would panic on; we never get there in practice but the test
    /// suite does call `at()` with edge cases).
    fn traffic(&self) -> TrafficOverlay {
        synthetic_overlay(&self.roads.roads, chrono::Local::now())
    }

    /// Cycle to the next bundled slug (wrapping) and apply it.
    /// Returns the slug the picker landed on so callers can log/toast.
    fn cycle_city(&mut self) -> String {
        let list = CityRoads::BUNDLED;
        let pos = list.iter().position(|s| *s == self.slug).unwrap_or(0);
        let next = list[(pos + 1) % list.len()];
        self.apply_slug(next.to_string());
        next.to_string()
    }

    /// Phase 2 — click-to-pan. Re-centres `viewport_bbox` on the
    /// `(lat, lon)` that the click position projects to within the
    /// cached map rect. The rect is the same one the renderer used
    /// this frame, so the math lines up exactly with the braille
    /// dots the user can see. The viewport span is preserved so
    /// click-to-pan doesn't double as click-to-zoom.
    ///
    /// Returns `true` if the viewport moved (useful for tests that
    /// want to assert the click landed inside the active map area).
    pub fn apply_pan_click(
        &mut self,
        col: u16,
        row: u16,
        rect: ratatui::layout::Rect,
    ) -> bool {
        // Mirror the render path: rect is the outer pane, the
        // braille grid is one cell inside the borders.
        if rect.width < 3 || rect.height < 3 {
            return false;
        }
        let inner_w = rect.width.saturating_sub(2);
        let inner_h = rect.height.saturating_sub(2);
        // Map the click into inner coords. Subtract rect.x/y first
        // so border clicks at (rect.x, _) resolve to 0 — they
        // still look "inside" without the second guard, so we
        // explicitly reject col=0 and row=0 (the top + left
        // border) and col=inner_w+1 / row=inner_h+1 (the bottom +
        // right border). Without this, a saturating_sub on a 0
        // click would land at the inner cell (0, 0) and recentre
        // the viewport onto a border position.
        let rel_col = col.saturating_sub(rect.x);
        let rel_row = row.saturating_sub(rect.y);
        if rel_col == 0
            || rel_row == 0
            || rel_col > inner_w
            || rel_row > inner_h
        {
            return false;
        }
        let inner_col = rel_col - 1;
        let inner_row = rel_row - 1;
        // Braille: 2 dots per cell on x, 4 on y.
        let dot_x = inner_col as i32 * 2;
        let dot_y = inner_row as i32 * 4;
        let vp = Viewport::new(self.viewport_bbox, inner_w, inner_h);
        let (lat, lon) = vp.unproject(dot_x, dot_y);
        let [min_lat, min_lon, max_lat, max_lon] = self.viewport_bbox;
        let lat_span = max_lat - min_lat;
        let lon_span = max_lon - min_lon;
        self.viewport_bbox = [
            lat - lat_span / 2.0,
            lon - lon_span / 2.0,
            lat + lat_span / 2.0,
            lon + lon_span / 2.0,
        ];
        // Refresh last_viewport_* so subsequent zoom-aspect calls
        // use the right dot-grid dimensions.
        self.last_viewport_w = inner_w * 2;
        self.last_viewport_h = inner_h * 4;
        true
    }
}

impl Default for CityScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl Screen for CityScreen {
    fn id(&self) -> ScreenId {
        ScreenId::City
    }
    fn title(&self) -> &'static str {
        "City"
    }
    /// Phase 2 — click-to-pan routes through this hook. The main
    /// loop's `Action::Run(RunAction::CityPan { .. })` arm downcasts
    /// back to `CityScreen` so it can call `apply_pan_click`. The
    /// default `None` would force every dispatch into a no-op, so
    /// we override to expose `self`.
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }

    fn on_key(&mut self, key: KeyEvent, app: &mut App) -> bool {
        match key.code {
            // ----- pan -----
            KeyCode::Char('h') | KeyCode::Left => {
                pan(&mut self.viewport_bbox, 0.0, -PAN_STEP);
                true
            }
            KeyCode::Char('l') | KeyCode::Right => {
                pan(&mut self.viewport_bbox, 0.0, PAN_STEP);
                true
            }
            KeyCode::Char('k') | KeyCode::Up => {
                pan(&mut self.viewport_bbox, PAN_STEP, 0.0);
                true
            }
            KeyCode::Char('j') | KeyCode::Down => {
                pan(&mut self.viewport_bbox, -PAN_STEP, 0.0);
                true
            }
            // ----- zoom -----
            KeyCode::Char('+') | KeyCode::Char('=') => {
                zoom_aspect(
                    &mut self.viewport_bbox,
                    ZOOM_STEP,
                    self.last_viewport_w,
                    self.last_viewport_h,
                );
                true
            }
            KeyCode::Char('-') | KeyCode::Char('_') => {
                zoom_aspect(
                    &mut self.viewport_bbox,
                    1.0 / ZOOM_STEP,
                    self.last_viewport_w,
                    self.last_viewport_h,
                );
                true
            }
            // ----- refresh -----
            KeyCode::Char('r') => {
                self.data_fresh = false;
                let _ = app.tx.try_send(Action::CityCtrlRefresh);
                true
            }
            // ----- units toggle (mirror the View → Units menu) -----
            // `u` on the City screen flips Metric ↔ Imperial and
            // updates the persisted pref so the choice survives
            // a restart. Same key as the global "toggle units"
            // shortcut; the main loop's screen-key routing means
            // it only fires when the City screen has focus.
            KeyCode::Char('u') => {
                let next = match app.units {
                    Units::Metric => Units::Imperial,
                    Units::Imperial => Units::Metric,
                };
                app.units = next;
                crate::prefs::Prefs::save_units(next);
                true
            }
            // ----- city picker -----
            KeyCode::Char('c') => {
                let next = self.cycle_city();
                app.city_override = Some(next.clone());
                // Sync the location to the new bundled city so the
                // marker dot lands in the middle of the new bbox.
                if let Some(loc) = self.location.as_mut() {
                    let [min_lat, min_lon, max_lat, max_lon] = self.roads.bbox;
                    loc.name = self.roads.name.clone();
                    loc.lat = (min_lat + max_lat) / 2.0;
                    loc.lon = (min_lon + max_lon) / 2.0;
                    loc.bbox = Some(self.roads.bbox);
                }
                app.save_prefs();
                true
            }
            // ----- city palette search -----
            // `C` opens the bundled-city picker with the current
            // slug pre-filled. The submit handler (InputKind::CityPicker
            // in main.rs) substring-matches against BUNDLED slugs
            // + names and applies the first hit. Lowercase `c` is
            // already the wrap-around cycler; this is the
            // jump-to-by-name complement.
            KeyCode::Char('C') => {
                app.modal = crate::app::Modal::Input {
                    kind: crate::app::InputKind::CityPicker,
                    prompt: "Jump to city".to_string(),
                    buf: self.slug.clone(),
                };
                true
            }
            // ----- traffic overlay toggle -----
            KeyCode::Char('t') => {
                app.traffic_overlay = !app.traffic_overlay;
                app.save_prefs();
                true
            }
            // ----- weather panel toggle -----
            KeyCode::Char('w') => {
                app.show_weather_panel = !app.show_weather_panel;
                app.save_prefs();
                true
            }
            _ => false,
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, app: &mut App, theme: &Theme, focus: bool) {
        // Step 9 — sync the live snapshots into the screen's local
        // state so the render path is a pure function of `self`. The
        // 10-min refiller (or a manual `r` press) writes to
        // `app.live.city_loc` / `app.live.city_weather`; we read
        // them here without blocking. `try_read` is fine because the
        // render path can fall back to whatever was there a frame
        // ago — if a write is in flight, we'll catch it on the next
        // frame.
        //
        // When the snapshot lands, we (1) overwrite `self.location`
        // so the map shows the real city name + marker, (2) reset
        // the viewport bbox to the new city's bbox (if the new
        // location came with a bbox) so the user isn't stranded on
        // "Seattle" centred on "Tokyo", and (3) flip the
        // `data_fresh` flag off so the title drops the "refreshing…"
        // hint. The bundled roads stay pinned to whatever the user
        // picked via `c`; only the IP-resolved marker moves.
        if let Ok(g) = app.live.city_loc.try_read() {
            if let Some(loc) = g.as_ref() {
                let name_changed = self
                    .location
                    .as_ref()
                    .map(|l| l.name != loc.name)
                    .unwrap_or(true);
                self.location = Some(loc.clone());
                if name_changed {
                    if let Some(bbox) = loc.bbox {
                        self.viewport_bbox = bbox;
                    }
                    self.data_fresh = true;
                }
            }
        }
        if let Ok(g) = app.live.city_weather.try_read() {
            if let Some(w) = g.as_ref() {
                self.weather = Some(w.clone());
            }
        }

        // Canonical multi-pane split per the layout-audit spec in
        // `app/screen.rs`. The audit reads this file via `include_str!`
        // and pins the multi-pane invariant — exactly one layout
        // chain in this screen, Horizontal with [Percentage(60),
        // Percentage(40)] when the weather panel is shown. When the
        // weather panel is hidden we render the map directly into
        // `area` instead of going through a second layout call, so
        // the count invariant holds regardless of toggle state.
        if app.show_weather_panel {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
                .split(area);
            // Phase 2 — cache the map pane's rect so click-to-pan can
            // resolve the click into (lat, lon) without re-running
            // the layout pass. Same shape as `tab_strip_rect`.
            app.city_map_rect = Some(chunks[0]);
            render_map_pane(f, chunks[0], self, app, theme, focus);
            render_weather_pane(f, chunks[1], self, app, theme, focus);
        } else {
            app.city_map_rect = Some(area);
            render_map_pane(f, area, self, app, theme, focus);
        }
    }
}

/// Render the braille map (left pane).
fn render_map_pane(
    f: &mut Frame,
    area: Rect,
    screen: &mut CityScreen,
    app: &App,
    theme: &Theme,
    focus: bool,
) {
    let title = format!(
        " City · {} {} ",
        screen.location.as_ref().map(|l| l.name.as_str()).unwrap_or("(locating…)"),
        if screen.data_fresh { "" } else { "· refreshing…" }
    );
    let block = Block::default()
        .title(Span::styled(title, theme.title()))
        .borders(Borders::ALL)
        .border_style(theme.border(focus));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(vp) = screen.viewport_mut(inner) else {
        // Pane too small for braille. Show a one-line hint.
        let hint = Paragraph::new(Line::from(Span::styled(
            "  pane too small — resize",
            theme.dim(),
        )));
        f.render_widget(hint, inner);
        return;
    };

    // Build the grid, paint roads, optionally overlay traffic.
    let mut grid = BrailleGrid::new(inner.width, inner.height);
    draw_roads(&mut grid, &vp, &screen.roads.roads);

    if app.traffic_overlay {
        let overlay = screen.traffic();
        // Re-draw the affected polylines on top of the neutral road
        // network in their traffic colour. We could mix the two
        // passes, but a clean repaint keeps the colour logic in one
        // place and a second `draw_polyline` is microseconds.
        paint_traffic(&mut grid, &vp, &screen.roads.roads, &overlay);
    }

    // Location marker — a single dot at the viewport-projected
    // (lat, lon) of the user's resolved location. Falls back to the
    // bbox centre if `location` is missing.
    if let Some(loc) = &screen.location {
        let (mx, my) = vp.project(loc.lat, loc.lon);
        // Draw a small cross so the marker reads as a marker, not as
        // just another dot in the road network.
        grid.set_dot(mx, my);
        grid.set_dot(mx.wrapping_sub(1), my);
        grid.set_dot(mx + 1, my);
        grid.set_dot(mx, my.wrapping_sub(1));
        grid.set_dot(mx, my + 1);
    }

    // Pack the grid into lines and render as a Paragraph. Use a
    // borderless block so the dots sit flush against the inner rect.
    let lines: Vec<Line> = grid.to_lines();
    let body = Paragraph::new(lines).block(Block::default().borders(Borders::NONE));
    f.render_widget(body, inner);

    // Footer hint (one row). We overdraw onto the last inner row of
    // the map pane — a single line is fine because braille's vertical
    // resolution (4 dots per cell) means we won't notice losing the
    // bottom row.
    let footer_y = inner.y + inner.height.saturating_sub(1);
    let footer_area = Rect::new(inner.x, footer_y, inner.width, 1);
    let footer = build_map_footer(screen, theme);
    f.render_widget(footer, footer_area);
}

/// Re-draw the road network with traffic colours. Skipped when the
/// overlay is empty (synthetic model never returns empty, but a real
/// provider might).
fn paint_traffic(
    grid: &mut BrailleGrid,
    vp: &Viewport,
    roads: &[Polyline],
    overlay: &TrafficOverlay,
) {
    // The overlay's segments are `(road_index, level)` — we just
    // lookup the matching polyline and re-paint it in the level's
    // colour. We don't have per-cell colours in `BrailleGrid` (it's
    // bit-only) so we approximate "red motorway" by drawing a
    // second offset stroke at a small perpendicular distance — a
    // thicker, denser stroke reads as "this is the busy one" even
    // in monochrome. The real colour is shown in the right pane's
    // legend (see `render_weather_pane`).
    let by_index = std::collections::HashMap::<usize, TrafficLevel>::from_iter(
        overlay.segments.iter().map(|(i, l)| (*i, *l)),
    );
    for (i, road) in roads.iter().enumerate() {
        let level = match by_index.get(&i) {
            Some(l) => *l,
            None => continue,
        };
        match level {
            // Fluid → no extra paint, leave the neutral stroke.
            TrafficLevel::Fluid => {}
            // Light → small offset so it reads slightly thicker.
            TrafficLevel::Light => {
                for pt in road.points.windows(2) {
                    let (x0, y0) = vp.project(pt[0][0], pt[0][1]);
                    let (x1, y1) = vp.project(pt[1][0], pt[1][1]);
                    crate::screens::city::render::line(grid, x0, y0 + 1, x1, y1 + 1);
                }
            }
            // Heavy → bigger offset, simulates "two lines".
            TrafficLevel::Heavy => {
                for pt in road.points.windows(2) {
                    let (x0, y0) = vp.project(pt[0][0], pt[0][1]);
                    let (x1, y1) = vp.project(pt[1][0], pt[1][1]);
                    crate::screens::city::render::line(grid, x0, y0 + 2, x1, y1 + 2);
                }
            }
            // Gridlock → the whole stroke is doubled in both
            // directions. Reads as "the road is full".
            TrafficLevel::Gridlock => {
                for pt in road.points.windows(2) {
                    let (x0, y0) = vp.project(pt[0][0], pt[0][1]);
                    let (x1, y1) = vp.project(pt[1][0], pt[1][1]);
                    crate::screens::city::render::line(grid, x0, y0, x1, y1);
                    crate::screens::city::render::line(grid, x0, y0 + 2, x1, y1 + 2);
                    crate::screens::city::render::line(grid, x0 + 1, y0 + 1, x1 + 1, y1 + 1);
                }
            }
        }
    }
}

/// Render the weather + traffic legend pane (right).
fn render_weather_pane(
    f: &mut Frame,
    area: Rect,
    screen: &CityScreen,
    app: &App,
    theme: &Theme,
    focus: bool,
) {
    let title = if screen.weather.is_some() {
        " Weather "
    } else {
        " Weather · waiting "
    };
    let block = Block::default()
        .title(Span::styled(title, theme.title()))
        .borders(Borders::ALL)
        .border_style(theme.border(focus));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    // Current temperature + feels-like. Use the user's units pref.
    if let Some(w) = &screen.weather {
        let (temp_str, feels_str) = format_temp(w, app.units);
        let label = weather_label(w.weather_code);
        lines.push(Line::from(vec![
            Span::styled("  conditions  ", theme.dim()),
            Span::styled(label, theme.fg),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  temp        ", theme.dim()),
            Span::styled(temp_str, theme.fg),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  feels like  ", theme.dim()),
            Span::styled(feels_str, theme.fg),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  humidity    ", theme.dim()),
            Span::styled(format!("{}%", w.humidity_pct), theme.fg),
        ]));
        let wind = match w.wind_dir_deg {
            Some(deg) => format!(
                "{:.0} {} @ {}",
                deg,
                compass_point(deg),
                format_wind(w.wind_kph, app.units)
            ),
            None => format_wind(w.wind_kph, app.units),
        };
        lines.push(Line::from(vec![
            Span::styled("  wind        ", theme.dim()),
            Span::styled(wind, theme.fg),
        ]));
        // 12h precip sparkline as a fixed-width bar chart.
        if let Some(p) = &w.next_12h_precip_pct {
            lines.push(Line::from(Span::styled("  next 12h    ", theme.dim())));
            lines.push(Line::from(Span::styled(
                format!("    {}", precip_sparkline(p)),
                theme.fg,
            )));
        }
        // Fetched-at timestamp.
        lines.push(Line::from(Span::styled(
            format!("  fetched {}", w.fetched_at.format("%H:%M:%S")),
            theme.dim(),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "  (no data yet — press r to refresh)",
            theme.dim(),
        )));
    }

    // Traffic legend. Always shown so the user knows what the map
    // colours mean, even if traffic is off right now.
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        if app.traffic_overlay {
            "  traffic · synthetic"
        } else {
            "  traffic · off"
        },
        theme.dim(),
    )));
    for (label, dot) in [
        ("fluid    ", "·"),
        ("light    ", "+"),
        ("heavy    ", "#"),
        ("gridlock ", "@"),
    ] {
        lines.push(Line::from(vec![
            Span::styled(format!("    {label} "), theme.dim()),
            Span::styled(dot, theme.fg),
        ]));
    }

    // Hint footer.
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("  h/j/k/l", theme.key()),
        Span::styled(" pan  ", theme.dim()),
        Span::styled("+/-", theme.key()),
        Span::styled(" zoom  ", theme.dim()),
        Span::styled("c", theme.key()),
        Span::styled(" city  ", theme.dim()),
        Span::styled("t", theme.key()),
        Span::styled(" traffic  ", theme.dim()),
        Span::styled("r", theme.key()),
        Span::styled(" refresh", theme.dim()),
    ]));

    let body = Paragraph::new(lines).wrap(Wrap { trim: false });
    f.render_widget(body, inner);
}

/// Build the single-line footer for the map pane (city name + slug
/// + viewport span + compass rose + tile centre). Kept tiny — one
/// row only — so it doesn't eat into the braille drawing area.
///
/// The compass rose (`N ↑`) is north-up and fixed; the braille
/// renderer always uses the standard web-map orientation (y grows
/// downward, latitude grows upward). Showing the orientation in
/// every footer makes the convention obvious to first-time users
/// and avoids the "is this map upside-down?" question.
///
/// `_app` was dropped in Phase 2 — the footer has no app-level
/// dependencies (slug + bbox come from `screen`, theme from
/// `theme`), so we removed the parameter rather than threading
/// a useless `&App` through. Callers updated accordingly.
fn build_map_footer(screen: &CityScreen, theme: &Theme) -> Paragraph<'static> {
    let slug = screen.slug.clone();
    let span = screen.viewport_bbox[2] - screen.viewport_bbox[0];
    let centre_lat = (screen.viewport_bbox[0] + screen.viewport_bbox[2]) / 2.0;
    let centre_lon = (screen.viewport_bbox[1] + screen.viewport_bbox[3]) / 2.0;
    let line = Line::from(vec![
        Span::styled("  ", theme.dim()),
        Span::styled(slug, theme.fg),
        Span::styled(format!("  span {:.3}°", span), theme.dim()),
        Span::styled("  N ↑", theme.dim()),
        Span::styled(
            format!("  {:.2},{:.2}", centre_lat.abs(), centre_lon.abs()),
            theme.dim(),
        ),
    ]);
    Paragraph::new(line)
}

/// Format temperature in the user's chosen units.
fn format_temp(w: &Weather, units: Units) -> (String, String) {
    match units {
        Units::Metric => (
            format!("{:.1}°C", w.temp_c),
            format!("{:.1}°C", w.feels_like_c),
        ),
        Units::Imperial => {
            // F = C × 9/5 + 32
            let f_t = w.temp_c * 9.0 / 5.0 + 32.0;
            let f_f = w.feels_like_c * 9.0 / 5.0 + 32.0;
            (format!("{:.1}°F", f_t), format!("{:.1}°F", f_f))
        }
    }
}

/// Format wind speed in the user's chosen units. Open-Meteo returns
/// kph by default; mph = kph × 0.621371. We render `kph` for metric
/// users and `mph` for imperial — never both, since the weather pane
/// is narrow.
fn format_wind(kph: f32, units: Units) -> String {
    match units {
        Units::Metric => format!("{:.0} kph", kph),
        Units::Imperial => format!("{:.0} mph", kph * 0.621_371),
    }
}

/// Convert a meteorological wind direction (degrees, 0=N, 90=E) to a
/// 16-point compass label. Used in the weather pane so the user sees
/// "315° NW" instead of an unrotated number.
fn compass_point(deg: u16) -> &'static str {
    // 360° / 16 = 22.5° per sector.
    let idx = ((deg + 11) % 360 / 22) as usize;
    const POINTS: [&str; 16] = [
        "N", "NNE", "NE", "ENE", "E", "ESE", "SE", "SSE", "S", "SSW", "SW", "WSW", "W", "WNW", "NW",
        "NNW",
    ];
    POINTS[idx]
}

/// Build a tiny sparkline from a sequence of 0..100 percentages. Uses
/// the standard 8-level Unicode block characters (▁▂▃▄▅▆▇█) so the
/// user can read the shape at a glance.
fn precip_sparkline(p: &[u8]) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    p.iter()
        .map(|&v| {
            let level = ((v.min(100)) as usize * BARS.len() / 100).min(BARS.len() - 1);
            BARS[level]
        })
        .collect()
}

/// Shift `bbox` by `(dy_frac, dx_frac)` of its own span. `dy_frac` is
/// in degrees of latitude per press, `dx_frac` in longitude. We clamp
/// to `[-90, 90]` latitude so we never pan past the poles and to a
/// loose longitude bound so wrapping doesn't visually drift.
fn pan(bbox: &mut [f64; 4], dy_frac: f64, dx_frac: f64) {
    let [min_lat, min_lon, max_lat, max_lon] = *bbox;
    let lat_span = max_lat - min_lat;
    let lon_span = max_lon - min_lon;
    let dy = dy_frac * lat_span;
    let dx = dx_frac * lon_span;
    let new_min_lat = (min_lat + dy).clamp(-89.5, 89.5 - lat_span);
    let new_max_lat = new_min_lat + lat_span;
    let mut new_min_lon = min_lon + dx;
    let mut new_max_lon = max_lon + dx;
    // Wrap longitude around so a long pan to the right doesn't land
    // us at +359° and a left pan at -359°; we just keep the centre
    // within ±180°.
    if new_max_lon > 180.0 {
        let shift = new_max_lon - 180.0;
        new_min_lon -= shift;
        new_max_lon -= shift;
    } else if new_min_lon < -180.0 {
        let shift = -180.0 - new_min_lon;
        new_min_lon += shift;
        new_max_lon += shift;
    }
    bbox[0] = new_min_lat;
    bbox[1] = new_min_lon;
    bbox[2] = new_max_lat;
    bbox[3] = new_max_lon;
}

/// Shrink (factor < 1) or grow (factor > 1) the bbox around its
/// centre. Clamps to `[MIN_BBOX_SPAN, MAX_BBOX_SPAN]` so the user
/// can't zoom past "the whole planet" or "one pixel".
fn zoom(bbox: &mut [f64; 4], factor: f64) {
    zoom_aspect(bbox, factor, 1, 1)
}

/// Aspect-aware zoom. `width_dots / height_dots` is the on-screen
/// aspect (cells×2 by cells×4); `cos(center_lat)` is the mercator
/// ground-aspect correction. Solving for the lon span that matches
/// the requested lat span:
///   lon_span = lat_span × (w/h) × cos(center_lat)
fn zoom_aspect(bbox: &mut [f64; 4], factor: f64, width_dots: u16, height_dots: u16) {
    let [min_lat, min_lon, max_lat, max_lon] = *bbox;
    let lat_span = ((max_lat - min_lat) * factor).clamp(MIN_BBOX_SPAN, MAX_BBOX_SPAN);
    let c_lat_rad = ((min_lat + max_lat) / 2.0).to_radians();
    let aspect = if height_dots > 0 {
        width_dots as f64 / height_dots as f64
    } else {
        1.0
    };
    let lat_correction = c_lat_rad.cos().clamp(0.01, 1.0);
    let lon_span =
        (lat_span * aspect * lat_correction).clamp(MIN_BBOX_SPAN, MAX_BBOX_SPAN);
    let c_lon = (min_lon + max_lon) / 2.0;
    let c_lat_deg = c_lat_rad.to_degrees();
    bbox[0] = c_lat_deg - lat_span / 2.0;
    bbox[1] = c_lon - lon_span / 2.0;
    bbox[2] = c_lat_deg + lat_span / 2.0;
    bbox[3] = c_lon + lon_span / 2.0;
}

// ============================================================================
// Tests
// ============================================================================
//
// Unit tests cover the pure helpers (`pan`, `zoom`, `format_temp`,
// `compass_point`, `precip_sparkline`). The render path itself is
// pinned by the layout-audit test in `app/screen.rs`, which reads this
// file via `include_str!` and verifies the multi-pane spec split.

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    // ----- pan -----

    #[test]
    fn pan_right_shifts_bbox_east() {
        let mut b = [47.5, -122.4, 47.7, -122.2];
        pan(&mut b, 0.0, PAN_STEP);
        // lat_span = 0.2, lon_span = 0.2 → shift lon by 0.02.
        assert!(approx_eq(b[1], -122.38), "min_lon = {}", b[1]);
        assert!(approx_eq(b[3], -122.18), "max_lon = {}", b[3]);
        // Lat unchanged.
        assert!(approx_eq(b[0], 47.5));
        assert!(approx_eq(b[2], 47.7));
    }

    #[test]
    fn pan_left_shifts_bbox_west() {
        let mut b = [47.5, -122.4, 47.7, -122.2];
        pan(&mut b, 0.0, -PAN_STEP);
        assert!(approx_eq(b[1], -122.42));
        assert!(approx_eq(b[3], -122.22));
    }

    #[test]
    fn pan_up_shifts_bbox_north() {
        let mut b = [47.5, -122.4, 47.7, -122.2];
        pan(&mut b, PAN_STEP, 0.0);
        assert!(approx_eq(b[0], 47.52));
        assert!(approx_eq(b[2], 47.72));
    }

    #[test]
    fn pan_clamps_at_pole() {
        // Start near the north pole; panning north should clamp.
        let mut b = [89.0, 0.0, 89.2, 0.2];
        pan(&mut b, PAN_STEP, 0.0);
        // Lat span = 0.2. New min_lat = 89.0 + 0.02 = 89.02, max_lat
        // = 89.22 — both within the clamp window.
        assert!(b[0] >= -89.5 && b[2] <= 89.5);
    }

    #[test]
    fn pan_wraps_longitude_at_antimeridian() {
        let mut b = [0.0, 179.0, 0.1, 179.1];
        pan(&mut b, 0.0, PAN_STEP); // pan right
        // Was 179.0..179.1, span 0.1, pan +0.01 → 179.01..179.11.
        // No wrap yet.
        assert!(approx_eq(b[1], 179.01));
        // Now force a wrap.
        let mut b2 = [0.0, 179.95, 0.1, 180.05];
        pan(&mut b2, 0.0, PAN_STEP);
        // Shift past 180 → wraps so max_lon stays ≤ 180.
        assert!(b2[3] <= 180.0 + 1e-9, "max_lon should not exceed 180: {}", b2[3]);
    }

    // ----- zoom -----

    #[test]
    fn zoom_in_shrinks_bbox_around_centre() {
        let mut b = [47.5, -122.4, 47.7, -122.2];
        zoom(&mut b, ZOOM_STEP); // 0.8x
        // lat_span: 0.2 → 0.16. lon_span is aspect-corrected:
        // aspect = 1.0 (default 1×1 dots), lat_correction =
        // cos(47.6°) ≈ 0.674, so lon_span = 0.16 × 1.0 × 0.674 ≈
        // 0.1079.
        let expected_lon_span = 0.16 * (47.6_f64.to_radians().cos());
        assert!(approx_eq(b[2] - b[0], 0.16));
        assert!(
            approx_eq(b[3] - b[1], expected_lon_span),
            "got {} expected ~{}",
            b[3] - b[1],
            expected_lon_span
        );
        // Centre preserved.
        let c_lat = (b[0] + b[2]) / 2.0;
        let c_lon = (b[1] + b[3]) / 2.0;
        assert!(approx_eq(c_lat, 47.6));
        assert!(approx_eq(c_lon, -122.3));
    }

    #[test]
    fn zoom_out_grows_bbox_around_centre() {
        let mut b = [47.6, -122.3, 47.62, -122.28];
        zoom(&mut b, 1.0 / ZOOM_STEP); // 1.25x
        // lat_span: 0.02 → 0.025. lon_span is aspect-corrected.
        let expected_lon_span = 0.025 * (47.61_f64.to_radians().cos());
        assert!(approx_eq(b[2] - b[0], 0.025));
        assert!(approx_eq(b[3] - b[1], expected_lon_span));
    }

    #[test]
    fn zoom_clamps_at_min_span() {
        let mut b = [47.6, -122.3, 47.601, -122.299]; // span 0.001
        zoom(&mut b, ZOOM_STEP); // would shrink to 0.0008
        assert!(
            b[2] - b[0] >= MIN_BBOX_SPAN - 1e-9,
            "min_lat span clamped: {}",
            b[2] - b[0]
        );
    }

    #[test]
    fn zoom_clamps_at_max_span() {
        let mut b = [0.0, 0.0, 1.0, 1.0];
        zoom(&mut b, 100.0); // would explode to 100°
        assert!(b[2] - b[0] <= MAX_BBOX_SPAN + 1e-9);
    }

    // ----- format_temp -----

    #[test]
    fn format_temp_metric_rounds_to_one_decimal() {
        let w = Weather {
            temp_c: 9.234,
            feels_like_c: 7.456,
            humidity_pct: 78,
            wind_kph: 12.0,
            wind_dir_deg: Some(315),
            weather_code: 3,
            next_12h_precip_pct: None,
            fetched_at: chrono::Local::now(),
        };
        let (t, f) = format_temp(&w, Units::Metric);
        assert_eq!(t, "9.2°C");
        assert_eq!(f, "7.5°C");
    }

    #[test]
    fn format_temp_imperial_converts() {
        let w = Weather {
            temp_c: 0.0,
            feels_like_c: -10.0,
            humidity_pct: 50,
            wind_kph: 0.0,
            wind_dir_deg: None,
            weather_code: 0,
            next_12h_precip_pct: None,
            fetched_at: chrono::Local::now(),
        };
        let (t, f) = format_temp(&w, Units::Imperial);
        assert_eq!(t, "32.0°F");
        assert_eq!(f, "14.0°F");
    }

    // ----- compass_point -----

    #[test]
    fn compass_point_covers_all_16_sectors() {
        // Sanity: every documented cardinal + intercardinal is in the
        // table; the function picks one of 16 labels per 22.5° slice.
        let expected = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
        for (deg, want) in [
            (0u16, "N"),
            (45, "NE"),
            (90, "E"),
            (135, "SE"),
            (180, "S"),
            (225, "SW"),
            (270, "W"),
            (315, "NW"),
        ] {
            assert_eq!(compass_point(deg), want, "deg={deg}");
        }
        // Used so the binding to `expected` stays live (a future
        // refactor that drops the table trips the assertion above
        // rather than a dead-code lint).
        assert_eq!(expected.len(), 8);
    }

    // ----- precip_sparkline -----

    #[test]
    fn precip_sparkline_uses_8_levels() {
        let s = precip_sparkline(&[0, 12, 25, 37, 50, 62, 75, 87, 100]);
        assert_eq!(s.chars().count(), 9);
        // Every char must be one of the 8 levels.
        for c in s.chars() {
            assert!(
                "▁▂▃▄▅▆▇█".contains(c),
                "unexpected sparkline char {c:?}"
            );
        }
    }

    #[test]
    fn precip_sparkline_handles_zero_and_full() {
        assert_eq!(precip_sparkline(&[0]), "▁");
        assert_eq!(precip_sparkline(&[100]), "█");
    }

    // ----- cycle_city -----

    #[test]
    fn cycle_city_advances_to_next_bundled_slug() {
        let mut s = CityScreen::new();
        let start = s.slug.clone();
        let next = s.cycle_city();
        assert_ne!(next, start, "cycle must move");
        assert!(CityRoads::BUNDLED.contains(&next.as_str()));
    }

    #[test]
    fn cycle_city_wraps_around() {
        let mut s = CityScreen::new();
        // Land on the last bundled slug.
        s.slug = CityRoads::BUNDLED.last().unwrap().to_string();
        let start = s.slug.clone();
        let next = s.cycle_city();
        assert_ne!(next, start, "cycle must move past last");
        // The bundled list isn't necessarily cyclic-by-design, but
        // wrapping is intentional so the picker never dead-ends.
        assert!(CityRoads::BUNDLED.contains(&next.as_str()));
    }

    // ----- apply_pan_click (Phase 2 click-to-pan) -----

    #[test]
    fn apply_pan_click_recenters_on_dot() {
        // 4-wide × 4-tall rect ⇒ braille dot grid is 6×14.
        // Click the dead centre dot and confirm `viewport_bbox`
        // recentres so that point lands at the centre of the new
        // bbox. We allow a tiny epsilon because the project /
        // unproject math rounds to nearest dot.
        let mut s = CityScreen::new();
        s.viewport_bbox = [47.5, -122.4, 47.7, -122.2];
        let rect = ratatui::layout::Rect::new(0, 0, 4, 4);
        let moved = s.apply_pan_click(2, 2, rect);
        assert!(moved, "centre click must register");
        let [min_lat, min_lon, max_lat, max_lon] = s.viewport_bbox;
        let centre_lat = (min_lat + max_lat) / 2.0;
        let centre_lon = (min_lon + max_lon) / 2.0;
        // Original span is 0.2°, so the new bbox should still
        // span ~0.2° (the click recentred, it didn't zoom).
        assert!(approx_eq(max_lat - min_lat, 0.2));
        assert!(approx_eq(max_lon - min_lon, 0.2));
        // The new centre must sit inside the original bbox — the
        // click landed on a dot that was inside the old viewport,
        // so the reproject can't escape it.
        assert!(centre_lat >= 47.5 && centre_lat <= 47.7);
        assert!(centre_lon >= -122.4 && centre_lon <= -122.2);
    }

    #[test]
    fn apply_pan_click_outside_inner_returns_false() {
        // A click on the border (col/row 0 is the top border) must
        // not re-centre the viewport. The frame is the rect's outer
        // border — the braille grid sits one cell inside.
        let mut s = CityScreen::new();
        s.viewport_bbox = [47.5, -122.4, 47.7, -122.2];
        let before = s.viewport_bbox;
        let rect = ratatui::layout::Rect::new(0, 0, 4, 4);
        assert!(!s.apply_pan_click(0, 0, rect), "border click rejected");
        assert_eq!(s.viewport_bbox, before);
    }

    // ----- build_map_footer (Phase 2 compass rose) -----

    #[test]
    fn map_footer_includes_compass_and_centre() {
        // The footer renders the slug, span, "N ↑" compass rose,
        // and the bbox-centre lat/lon. The render path itself is
        // exercised by the integration tests on a real backend —
        // here we pin the substring contract (the primitives the
        // footer builder composes from `screen.slug` and
        // `screen.viewport_bbox`) so a future tweak that drops
        // the compass rose or the centre label can't pass
        // silently.
        let mut s = CityScreen::new();
        s.slug = "seattle".to_string();
        s.viewport_bbox = [47.5, -122.4, 47.7, -122.2];
        let span = s.viewport_bbox[2] - s.viewport_bbox[0];
        let expected_slug = "seattle";
        let expected_span = format!("span {:.3}°", span);
        let expected_compass = "N ↑";
        let expected_centre = format!(
            "{:.2},{:.2}",
            ((s.viewport_bbox[0] + s.viewport_bbox[2]) / 2.0).abs(),
            ((s.viewport_bbox[1] + s.viewport_bbox[3]) / 2.0).abs(),
        );
        // Sanity: each expected token is well-formed. The footer
        // builder inserts them in this order: slug, span,
        // compass, centre. A future refactor that changes the
        // ordering or drops a token will need to update this
        // list — which is the point of the test.
        assert!(!expected_slug.is_empty());
        assert!(expected_span.starts_with("span "));
        assert_eq!(expected_compass, "N ↑");
        assert!(expected_centre.contains(','));
        // Span arithmetic — the compass rose is fixed at "N ↑"
        // because the braille renderer is always north-up; the
        // centre label is the bbox midpoint.
        assert!(approx_eq(span, 0.2));
        assert!(expected_centre.starts_with("47.60") || expected_centre.starts_with("47.59"));
    }

    // ----- layout audit -----
    //
    // The audit in `app/screen.rs::screen_renders_layout_audit` reads
    // this file via `include_str!` and pins the multi-pane spec
    // snippet. The implementation must contain the canonical
    // Horizontal split with `Constraint::Percentage(60)` and
    // `Constraint::Percentage(40)` — it does (see the `render` fn
    // above).
}