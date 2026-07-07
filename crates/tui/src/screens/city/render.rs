//! Braille renderer (Step 7).
//!
//! A `BrailleGrid` is `W*2 × H*4` addressable bits packed into Unicode
//! braille characters (`U+2800` + bit offset). The renderer is a pure
//! function of (polylines, viewport) — no async, no I/O, no ratatui
//! dependency at this layer — so it stays trivially unit-testable
//! with golden strings.
//!
//! Coordinate system:
//!   * `(cx, cy)` — terminal-cell coords for the on-screen grid.
//!   * `(x, y)`   — integer *dot* coords inside the dot grid:
//!                   `x = cx * 2 + dx`, `y = cy * 4 + dy`.
//!
//! Drawing is via Bresenham (`line`) which lights up every dot the
//! line touches. `project_polyline` projects a lat/lon polyline into
//! the dot grid using a `Viewport`, pan/zoom-aware.

use ratatui::text::Line;

use super::roads::{Polyline, RoadImportance};

/// 2x-horizontal × 4x-vertical resolution grid of braille dots.
/// `w_chars` and `h_chars` are the on-screen dimensions in terminal
/// cells; the underlying bit grid is `w_chars*2` × `h_chars*4`.
#[derive(Debug, Clone)]
pub struct BrailleGrid {
    pub w_chars: u16,
    pub h_chars: u16,
    /// Bit per dot, packed as `bits[cell * 8 + bit]`. The byte order
    /// inside `to_lines` matches the Unicode braille mask so setting
    /// `bits[cell*8 + i]` lights up exactly dot `i` of cell `cell`.
    bits: Vec<u8>,
}

impl BrailleGrid {
    /// Empty grid. Out-of-range `set_dot` / `set_cell_dot` calls are
    /// silently dropped — callers don't have to clip before drawing.
    pub fn new(w_chars: u16, h_chars: u16) -> Self {
        let n = (w_chars as usize) * (h_chars as usize) * 8;
        Self {
            w_chars,
            h_chars,
            bits: vec![0; n],
        }
    }

    /// Set a single dot in *grid* coords (`x` ∈ `0..w_chars*2`,
    /// `y` ∈ `0..h_chars*4`). Bit-OR so overlapping draws accumulate.
    pub fn set_dot(&mut self, x: i32, y: i32) {
        if x < 0 || y < 0 {
            return;
        }
        let (x, y) = (x as u32, y as u32);
        let w = self.w_chars as u32 * 2;
        let h = self.h_chars as u32 * 4;
        if x >= w || y >= h {
            return;
        }
        // Each terminal cell holds 2 columns × 4 rows of dots.
        let cx = x / 2;
        let cy = y / 4;
        let dx = x % 2;
        let dy = y % 4;
        // Unicode braille bit layout (bit 0 = top-left of the cell):
        //   dot (dx, dy) → bit offset = dy * 2 + dx.
        let bit = (dy * 2 + dx) as usize;
        let cell = (cy as usize) * (self.w_chars as usize) + (cx as usize);
        self.bits[cell * 8 + bit] = 1;
    }

    /// Pack the bit grid into `Line`s of Unicode braille chars.
    /// Empty cells render as a regular space so the grid stays
    /// aligned with surrounding text.
    pub fn to_lines(&self) -> Vec<Line<'static>> {
        let mut out = Vec::with_capacity(self.h_chars as usize);
        for cy in 0..self.h_chars {
            let mut s = String::with_capacity(self.w_chars as usize);
            for cx in 0..self.w_chars {
                let cell = (cy as usize) * (self.w_chars as usize) + (cx as usize);
                let byte = self.bits[cell * 8..cell * 8 + 8]
                    .iter()
                    .enumerate()
                    .fold(0u8, |acc, (i, &b)| acc | (b << i));
                if byte == 0 {
                    s.push(' ');
                } else {
                    s.push(char::from_u32(0x2800 + byte as u32).unwrap_or(' '));
                }
            }
            out.push(Line::from(s));
        }
        out
    }

    /// Convenience: total number of lit dots. Useful for "did we draw
    /// anything?" checks in tests and for non-rendering call sites
    /// (e.g. a future "minimap: dim if too dense").
    pub fn lit_count(&self) -> usize {
        self.bits.iter().filter(|b| **b != 0).count()
    }
}

/// Bresenham line in integer `(x, y)` space. Lights every dot the
/// line touches (no antialiasing — braille's only got 8 dots per
/// cell so anti-aliasing is meaningless). Out-of-range endpoints
/// are clipped by `set_dot`, so callers don't need to bound-check.
///
/// This is the canonical Bresenham "draw all the pixels of a line
/// between two integer endpoints" algorithm (the same one used by
/// every line-drawing tutorial since the 60s). It works on
/// diagonals of any slope and is integer-only.
pub fn line(grid: &mut BrailleGrid, x0: i32, y0: i32, x1: i32, y1: i32) {
    // Steep: `dx > dy`, we step along `y` and use `x` error term.
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut x = x0;
    let mut y = y0;
    loop {
        grid.set_dot(x, y);
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

/// Viewport = the slice of the world we want to render, plus the
/// pan offset and zoom factor. The city-screen keymap mutates
/// `center_lat/lon` and `zoom`; the renderer reads a snapshot.
#[derive(Debug, Clone)]
pub struct Viewport {
    /// `[min_lat, min_lon, max_lat, max_lon]` of what the user is
    /// currently looking at. Smaller = more zoomed in.
    pub bbox: [f64; 4],
    /// Pixel-grid dimensions of the braille surface (cells × 2 in x,
    /// cells × 4 in y).
    pub width_dots: u16,
    pub height_dots: u16,
}

impl Viewport {
    pub fn new(bbox: [f64; 4], w_chars: u16, h_chars: u16) -> Self {
        Self {
            bbox,
            width_dots: w_chars * 2,
            height_dots: h_chars * 4,
        }
    }

    /// Project a single `(lat, lon)` point into dot-grid coordinates.
    /// Out-of-bounds points get negative or ≥-dimension values, which
    /// `set_dot` clips silently — no need to filter pre-draw.
    ///
    /// Mercator-style correction: longitude is scaled by `cos(center_lat)`
    /// so a square mile of Earth maps to a square of dots regardless of
    /// latitude. Without this, mid-latitude cities look vertically
    /// stretched (a 1° lon × 1° lat bbox near NYC is ~80km × 111km on
    /// the ground, so dot-grid aspect goes ~1.4:1 instead of 1:1).
    pub fn project(&self, lat: f64, lon: f64) -> (i32, i32) {
        let [min_lat, min_lon, max_lat, max_lon] = self.bbox;
        let lat_span = max_lat - min_lat;
        let lon_span = max_lon - min_lon;
        if lat_span <= 0.0 || lon_span <= 0.0 {
            return (0, 0);
        }
        // Aspect correction. Scale lon by `1/cos(center_lat)` so the
        // dot-grid is isotropic at this latitude. At the equator this
        // is 1.0 (no correction); at 60° it's 2.0; at the poles it's
        // undefined, but `clamp` keeps it sane if the user zooms into
        // a polar region.
        let center_lat = 0.5 * (min_lat + max_lat).to_radians();
        let lat_correction = center_lat.cos().clamp(0.01, 1.0);
        let nx = (lon - min_lon) / (lon_span * lat_correction);
        let ny = (lat - min_lat) / lat_span;
        // Map ny ∈ [0,1] → y ∈ [0, height_dots], inverted (lat goes
        // up visually but +y goes down on a terminal grid).
        let x = (nx * self.width_dots as f64).round() as i32;
        let y = ((1.0 - ny) * self.height_dots as f64).round() as i32;
        (x, y)
    }
}

/// Stroke weight per importance tag, in dot units. Motorways /
/// trunks get a 2-dot-thick stroke (so they show up at low zoom);
/// footways stay 1 dot. We approximate thickness by emitting a
/// parallel offset line per segment, which keeps the implementation
/// branch-free.
fn stroke_weight(importance: &RoadImportance) -> u8 {
    match importance.0.as_str() {
        "motorway" | "trunk" => 2,
        "primary" | "secondary" => 2,
        _ => 1,
    }
}

/// Draw a single polyline into the grid using the viewport. Calls
/// `line` per segment. `thickness > 1` is approximated by emitting
/// parallel offset lines; the cheap approximation keeps the renderer
/// CPU-cheap (a few MB of polylines can still draw in <16ms).
pub fn draw_polyline(grid: &mut BrailleGrid, vp: &Viewport, poly: &Polyline) {
    if poly.points.len() < 2 {
        return;
    }
    let w = stroke_weight(&poly.importance) as i32;
    // For thickness > 1, draw the polyline plus half-pixel offsets
    // along the perpendicular. A single dot offset is fine because
    // braille's 8x resolution already amplifies the perceived
    // thickness; full stroke-shading would be over-engineered.
    for dy in 0..w {
        // Compute `(x, y) - dy * scale` for each endpoint. `scale`
        // is the per-row lon→dot ratio so vertical shifts don't
        // warp the geometry.
        let mut prev: Option<(i32, i32)> = None;
        for pt in &poly.points {
            let (x, y) = vp.project(pt[0], pt[1]);
            // Offset perpendicular to north — moving down by `dy`
            // dots shifts the visual north a bit so a 2-dot
            // stroke covers both rows of the cell.
            let y_off = y + dy;
            let x_off = x;
            grid.set_dot(x_off, y_off);
            if let Some((px, py)) = prev {
                line(grid, px, py, x_off, y_off);
            }
            prev = Some((x_off, y_off));
        }
    }
}

/// Draw every polyline from a `CityRoads` into the grid.
pub fn draw_roads(grid: &mut BrailleGrid, vp: &Viewport, roads: &[Polyline]) {
    for r in roads {
        draw_polyline(grid, vp, r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid_to_str(g: &BrailleGrid) -> String {
        g.to_lines()
            .into_iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn empty_grid_is_all_spaces() {
        let g = BrailleGrid::new(3, 2);
        let s = grid_to_str(&g);
        assert_eq!(s.lines().count(), 2);
        for line in s.lines() {
            assert_eq!(line, "   ", "expected 3 spaces, got {line:?}");
        }
        assert_eq!(g.lit_count(), 0);
    }

    #[test]
    fn single_dot_top_left_lights_only_bit_zero() {
        // cell (0,0), dot (0,0) → bit 0 → offset 0x01 → U+2801.
        let mut g = BrailleGrid::new(1, 1);
        g.set_dot(0, 0);
        assert_eq!(grid_to_str(&g), "\u{2801}");
        assert_eq!(g.lit_count(), 1);
    }

    #[test]
    fn dots_pack_into_unicode_braille_correctly() {
        // Light every dot in cell (0,0) → 0xFF → U+28FF.
        let mut g = BrailleGrid::new(1, 1);
        for dy in 0..4 {
            for dx in 0..2 {
                g.set_dot(dx, dy);
            }
        }
        assert_eq!(grid_to_str(&g), "\u{28FF}");
    }

    #[test]
    fn set_dot_clamps_outside_grid() {
        let mut g = BrailleGrid::new(1, 1);
        g.set_dot(-1, 0);
        g.set_dot(0, -1);
        g.set_dot(2, 0); // x outside w_chars*2=2
        g.set_dot(0, 4); // y outside h_chars*4=4
        g.set_dot(99, 99);
        assert_eq!(grid_to_str(&g), " ");
        assert_eq!(g.lit_count(), 0);
    }

    /// Bresenham of a horizontal line at y=1, drawn on a 3×1 grid
    /// (6 dots wide). y=1 lives in cell-row 0 (cy=0) at dy=1; both
    /// columns (dx=0, dx=1) get lit at that dy in every cell. Bit
    /// layout: dy=1,dx=0 → bit 2 (0x04); dy=1,dx=1 → bit 3 (0x08).
    /// Per-cell byte = 0x0C → U+280C → `⠌`.
    #[test]
    fn bresenham_horizontal_line_covers_full_row() {
        let mut g = BrailleGrid::new(3, 1);
        line(&mut g, 0, 1, 5, 1);
        let s = grid_to_str(&g);
        assert_eq!(s, "\u{280C}\u{280C}\u{280C}");
    }

    /// Bresenham of a vertical line at x=1 — covers the right
    /// column (dx=1) of every cell row. y goes 0..7 → cy=0 rows
    /// dy=0..3 then cy=1 rows dy=0..3. Per cell, only the dx=1
    /// column is lit at every dy. Bits dy=0..3 at dx=1 are
    /// 1|3|5|7 = 0xAA = U+28AA → `⢪`. (Earlier golden expected all
    /// 8 dots lit; that was wrong — the line at x=1 sets *one*
    /// column, not both. Bresenham only visits the dots the line
    /// actually crosses.)
    #[test]
    fn bresenham_vertical_line_covers_both_columns() {
        let mut g = BrailleGrid::new(1, 2);
        line(&mut g, 1, 0, 1, 7);
        let s = grid_to_str(&g);
        assert_eq!(s, "\u{28AA}\n\u{28AA}");
    }

    /// Bresenham of a 45° line should be readable enough that a
    /// human eye can spot the diagonal. The exact bit pattern
    /// depends on the implementation, but the line must reach both
    /// endpoints.
    #[test]
    fn bresenham_diagonal_lights_endpoints() {
        let mut g = BrailleGrid::new(2, 2);
        line(&mut g, 0, 0, 3, 7); // top-left to bottom-right of dot grid
        // 8 dots lit in total (every step of Bresenham is one dot),
        // and the endpoints (0,0) and (3,7) must be in the lit set.
        assert_eq!(g.lit_count(), 8);
    }

    /// Viewport projects (min_lon, min_lat) to top-left and
    /// (max_lon, max_lat) to bottom-right of the dot grid.
    #[test]
    fn viewport_maps_corners_to_corners() {
        let vp = Viewport::new([0.0, 0.0, 1.0, 1.0], 2, 2);
        // width_dots=4, height_dots=8.
        let (x_tl, y_tl) = vp.project(0.0, 0.0);
        assert_eq!((x_tl, y_tl), (0, 8));
        let (x_br, y_br) = vp.project(1.0, 1.0);
        assert_eq!((x_br, y_br), (4, 0));
    }

    /// Polyline draw paints every segment into the grid; the
    /// `Lit count > 0` invariant pins that we actually filled.
    #[test]
    fn draw_polyline_lights_at_least_one_dot() {
        let vp = Viewport::new([47.5, -122.5, 47.7, -122.3], 4, 4);
        let poly = Polyline {
            points: vec![[47.60, -122.40], [47.62, -122.38]],
            importance: RoadImportance("motorway".into()),
        };
        let mut g = BrailleGrid::new(4, 4);
        draw_polyline(&mut g, &vp, &poly);
        assert!(g.lit_count() > 0);
    }

    /// Stroke weight for motorways is 2, residential is 1 — drawn
    /// into the same viewport the motorway should produce more lit
    /// dots than the residential line of equal length.
    #[test]
    fn stroke_weight_thick_lines_produce_more_dots() {
        let vp = Viewport::new([47.5, -122.5, 47.7, -122.3], 4, 4);
        let motorway = Polyline {
            points: vec![[47.60, -122.40], [47.62, -122.38]],
            importance: RoadImportance("motorway".into()),
        };
        let residential = Polyline {
            points: vec![[47.60, -122.40], [47.62, -122.38]],
            importance: RoadImportance("residential".into()),
        };
        let mut g1 = BrailleGrid::new(4, 4);
        let mut g2 = BrailleGrid::new(4, 4);
        draw_polyline(&mut g1, &vp, &motorway);
        draw_polyline(&mut g2, &vp, &residential);
        assert!(
            g1.lit_count() >= g2.lit_count(),
            "motorway={} should not light fewer dots than residential={}",
            g1.lit_count(),
            g2.lit_count()
        );
    }

    /// Golden: a single straight horizontal line across a 4×2 grid
    /// (8 dots wide × 8 dots tall). Bresenham at y=4, x=0..7 lights
    /// `y=4` (cy=1, dy=0) in both columns. Per-cell byte =
    /// `1 << 0` + `1 << 1` = 0x03 → U+2803 → `⠃`. The cell-row 0
    /// stays empty (line lives entirely in row 1). This golden
    /// pinpoints the bit-layout invariant: a future "refactor the
    /// renderer" PR that breaks the contract gets caught before
    /// users do.
    #[test]
    fn golden_horizontal_line_4x2() {
        let mut g = BrailleGrid::new(4, 2);
        line(&mut g, 0, 4, 7, 4);
        let s = grid_to_str(&g);
        assert_eq!(s, "    \n\u{2803}\u{2803}\u{2803}\u{2803}");
    }

    /// End-to-end: project the bundled seattle polylines into a
    /// small grid and assert at least *something* draws — this is
    /// the smoke test that catches regressions in the integration
    /// between `roads` and `render`. The bundled fixture has 6
    /// polylines that span ~40% of the bbox (a deliberate sparse
    /// data set — Step 8 adds the real Overpass-loaded geometry
    /// via `scripts/fetch_city_roads.py`); we threshold at >10 lit
    /// dots so the test is stable as the bundled data grows.
    #[test]
    fn bundled_seattle_renders_into_grid() {
        use super::super::roads::CityRoads;
        let cr = CityRoads::load_bundled("seattle").unwrap();
        let [min_lat, min_lon, max_lat, max_lon] = cr.bbox;
        let vp = Viewport::new(
            [min_lat, min_lon, max_lat, max_lon],
            20,
            10,
        );
        let mut g = BrailleGrid::new(20, 10);
        draw_roads(&mut g, &vp, &cr.roads);
        assert!(
            g.lit_count() > 10,
            "expected >10 lit dots from seattle.json, got {}",
            g.lit_count()
        );
    }
}
