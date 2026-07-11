//! City screen v2 — braille road map (left) + weather (right).
//! Ports state from the existing CityScreen; navigation rewritten for NavEvent.
use std::cell::Cell;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use cyberdeck_core::city::Weather;

use crate::app::screen::{ScreenId, ScreenV2, Zone};
use crate::nav::event::{Consumed, NavEvent};
use crate::nav::UiContext;
use crate::screens::city::roads::CityRoads;
use crate::screens::city::render::{draw_areas, draw_pois, draw_roads, BrailleGrid, Viewport};
use crate::screens::city::weather::weather_label;

const ZONES: &[Zone]  = &[Zone::Left, Zone::Right];
const MIN_SPAN: f64   = 0.005;
const MAX_SPAN: f64   = 5.0;
const ZOOM_STEP: f64  = 0.8;
const PAN_STEP: f64   = 0.1;

pub struct CityScreenV2 {
    pub roads:          CityRoads,
    // Cell: render() is &self but must snap viewport when live location arrives
    viewport_bbox:      Cell<[f64; 4]>,
    loaded_coords:      Cell<Option<(f64, f64)>>,
}

impl Default for CityScreenV2 {
    fn default() -> Self {
        let (_, roads) = CityRoads::load_bundled_or_default("seattle");
        let loc = roads.location();
        let bbox = loc.bbox.unwrap_or(roads.bbox);
        Self { roads, viewport_bbox: Cell::new(bbox), loaded_coords: Cell::new(None) }
    }
}

impl ScreenV2 for CityScreenV2 {
    fn id(&self) -> ScreenId { ScreenId::City }
    fn title(&self) -> &str { "City" }
    fn focusable_zones(&self) -> &[Zone] { ZONES }
    fn hint(&self) -> &str { "▲▼◀▶ pan   +/- zoom   ◀▶ pane   B back" }

    fn on_focus(&mut self, ctx: &mut crate::nav::UiContext<'_>) {
        self.sync_location(ctx.live);
    }

    fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
        match event {
            NavEvent::Tab   => { ctx.nav.focus_zone = (ctx.nav.focus_zone + 1) % ZONES.len(); Consumed::Yes }
            NavEvent::BackTab => {
                let n = ZONES.len();
                ctx.nav.focus_zone = (ctx.nav.focus_zone + n - 1) % n;
                Consumed::Yes
            }
            // Arrow keys pan the map
            NavEvent::Up    => { self.pan(0.0,  PAN_STEP); Consumed::Yes }
            NavEvent::Down  => { self.pan(0.0, -PAN_STEP); Consumed::Yes }
            NavEvent::Left  => { self.pan(-PAN_STEP, 0.0); Consumed::Yes }
            NavEvent::Right => { self.pan( PAN_STEP, 0.0); Consumed::Yes }
            NavEvent::Char('+') | NavEvent::Char('=') => { self.zoom(ZOOM_STEP); Consumed::Yes }
            NavEvent::Char('-') | NavEvent::Char('_') => { self.zoom(1.0 / ZOOM_STEP); Consumed::Yes }
            // Manual refresh: re-trigger geo + weather + roads fetch.
            NavEvent::Char('r') => {
                let city_loc     = ctx.live.city_loc.clone();
                let city_weather = ctx.live.city_weather.clone();
                let city_data    = ctx.live.city_data.clone();
                let is_day       = ctx.live.is_day.clone();
                let tx           = ctx.tx.clone();
                tokio::spawn(async move {
                    crate::app::live_data::refresh_city(city_loc, city_weather, city_data, is_day, tx).await;
                });
                Consumed::Yes
            }
            NavEvent::Back => { ctx.go_back(); Consumed::Yes }
            _ => Consumed::No,
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &UiContext<'_>) {
        // Snap viewport when live geo-location arrives (on_focus is never called
        // by the run loop, so this is the only place the viewport gets updated).
        self.sync_location(ctx.live);

        let theme = &ctx.ui.theme;
        let left_focused  = ctx.nav.focus_zone == 0;
        let right_focused = ctx.nav.focus_zone == 1;

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);

        // ── Left: braille road map ────────────────────────────────────────────
        let map_inner = cols[0];
        // inner dims inside the block border (1 cell each side)
        let w = map_inner.width.saturating_sub(2);
        let h = map_inner.height.saturating_sub(2);

        let live_data = ctx.live.city_data.try_read().ok().and_then(|g| g.clone());
        let vp_bbox = self.viewport_bbox.get();
        let map_lines: Vec<Line<'static>> = if w > 0 && h > 0 {
            let vp = Viewport::new(vp_bbox, w, h);
            let mut grid = BrailleGrid::new(w, h);
            if let Some(ref cd) = live_data {
                draw_areas(&mut grid, &vp, &cd.areas);
                draw_roads(&mut grid, &vp, &cd.roads);
                draw_pois(&mut grid, &vp, &cd.pois);
            } else {
                draw_roads(&mut grid, &vp, &self.roads.roads);
            }
            grid.to_lines()
        } else {
            vec![Line::from("")]
        };

        // Check live location for title
        let city_name = ctx.live.city_loc.try_read().ok()
            .and_then(|loc| loc.as_ref().map(|l| l.name.clone()))
            .unwrap_or_else(|| self.roads.location().name.clone());

        let map_para = Paragraph::new(map_lines)
            .block(Block::default()
                .title(Span::styled(format!(" {} ", city_name), theme.title()))
                .borders(Borders::ALL)
                .border_style(theme.border(left_focused)));
        frame.render_widget(map_para, map_inner);

        // ── Right: weather ────────────────────────────────────────────────────
        let weather_lines: Vec<Line<'static>> = match ctx.live.city_weather.try_read() {
            Ok(g) => match g.as_ref() {
                Some(wx) => build_weather_lines(wx, theme),
                None => vec![Line::from(Span::styled(
                    "(fetching weather…  press r to retry)",
                    Style::default().fg(theme.dim),
                ))],
            },
            Err(_) => vec![Line::from(Span::styled(
                "weather unavailable",
                Style::default().fg(theme.dim),
            ))],
        };

        let weather_para = Paragraph::new(weather_lines)
            .block(Block::default()
                .title(Span::styled(" weather ", theme.title()))
                .borders(Borders::ALL)
                .border_style(theme.border(right_focused)))
            .wrap(Wrap { trim: false });
        frame.render_widget(weather_para, cols[1]);
    }
}

impl CityScreenV2 {
    /// Snap viewport + loaded_coords when live geo changes.
    /// Safe from &self (Cell fields) so render() can call it.
    fn sync_location(&self, live: &crate::app::live_data::LiveData) {
        let loc = match live.city_loc.try_read().ok().and_then(|g| g.clone()) {
            Some(l) => l,
            None => return,
        };
        let needs_update = match self.loaded_coords.get() {
            Some((lat, lon)) => (lat - loc.lat).abs() >= 0.01 || (lon - loc.lon).abs() >= 0.01,
            None => true,
        };
        if !needs_update { return; }
        // Match the span used by the Overpass fetch in live_data::refresh_city
        let span = 0.1;
        self.viewport_bbox.set([loc.lat - span, loc.lon - span, loc.lat + span, loc.lon + span]);
        self.loaded_coords.set(Some((loc.lat, loc.lon)));
    }

    fn pan(&mut self, dlon_frac: f64, dlat_frac: f64) {
        let [min_lat, min_lon, max_lat, max_lon] = self.viewport_bbox.get();
        let lat_span = max_lat - min_lat;
        let lon_span = max_lon - min_lon;
        let new_min_lat = min_lat + dlat_frac * lat_span;
        let new_min_lon = min_lon + dlon_frac * lon_span;
        self.viewport_bbox.set([
            new_min_lat,
            new_min_lon,
            new_min_lat + lat_span,
            new_min_lon + lon_span,
        ]);
    }

    fn zoom(&mut self, factor: f64) {
        let [min_lat, min_lon, max_lat, max_lon] = self.viewport_bbox.get();
        let clat = (min_lat + max_lat) / 2.0;
        let clon = (min_lon + max_lon) / 2.0;
        let new_span = ((max_lat - min_lat) * factor)
            .clamp(MIN_SPAN, MAX_SPAN);
        self.viewport_bbox.set([
            clat - new_span / 2.0,
            clon - new_span / 2.0,
            clat + new_span / 2.0,
            clon + new_span / 2.0,
        ]);
    }
}

fn build_weather_lines(wx: &Weather, theme: &crate::theme::Theme) -> Vec<Line<'static>> {
    let description = weather_label(wx.weather_code);
    let dir_str = wx.wind_dir_deg
        .map(|d| format!(" {}°", d))
        .unwrap_or_default();
    vec![
        Line::from(vec![
            Span::styled("condition  ", Style::default().fg(theme.dim)),
            Span::styled(description.to_string(), Style::default().fg(theme.fg)),
        ]),
        Line::from(vec![
            Span::styled("temp       ", Style::default().fg(theme.dim)),
            Span::styled(format!("{:.1}°C (feels {:.1}°C)", wx.temp_c, wx.feels_like_c),
                Style::default().fg(theme.fg)),
        ]),
        Line::from(vec![
            Span::styled("humidity   ", Style::default().fg(theme.dim)),
            Span::styled(format!("{}%", wx.humidity_pct), Style::default().fg(theme.fg)),
        ]),
        Line::from(vec![
            Span::styled("wind       ", Style::default().fg(theme.dim)),
            Span::styled(format!("{:.0} km/h{}", wx.wind_kph, dir_str), Style::default().fg(theme.fg)),
        ]),
    ]
}
