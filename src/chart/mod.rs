//! The chart widget: volume candles, equivolume and regular candles with a price
//! axis, a time axis, crosshair, legend and a volume overlay.
//!
//! The horizontal axis is in *world units*. In the time-slot modes every bar is
//! one unit wide; in equivolume mode a bar is as wide as its volume divided by
//! the mean volume. `edges[i]..edges[i + 1]` is bar `i`, so panning, zooming and
//! hit-testing are the same in every mode. Candles, volume and grid are drawn
//! as one pixel-aligned mesh (crisp 1-pixel wicks, no anti-aliasing blur), and
//! bars narrower than ~2 pixels are merged per pixel column.

mod axis;
mod theme;

use std::collections::VecDeque;
use std::sync::Arc;

use eframe::egui::{
    self, pos2, vec2, Align2, Color32, CursorIcon, FontId, Key, Mesh, Pos2, Rect, Sense, Shape,
    Stroke, Ui,
};
use serde::{Deserialize, Serialize};

pub use axis::{format_price, format_volume};
pub use theme::{pine, Theme};

use crate::data::datetime::{self, DAY};
use crate::data::Candle;
use crate::indicators::{self, Cdc, CdcSettings, Ribbon, Rsi, RsiSettings, Trend, Zone};

/// Default zoom: pixels per bar.
const DEFAULT_BAR_PX: f64 = 8.0;
/// Empty space right of the last bar, in bars.
const MARGIN_BARS: f64 = 4.0;
const MAX_BAR_PX: f64 = 120.0;
/// Narrowest equivolume bar, as a share of the mean volume (zero-volume bars stay visible).
const MIN_EQUIVOLUME_WIDTH: f64 = 0.05;
const TIME_AXIS_H: f32 = 26.0;
/// Height of the volume overlay, as a share of the plot.
const VOLUME_SHARE: f32 = 0.18;
/// Height of the RSI pane, as a share of the chart (at least `RSI_MIN_H`).
const RSI_SHARE: f32 = 0.24;
const RSI_MIN_H: f32 = 70.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChartKind {
    VolumeCandles,
    Equivolume,
    Candles,
}

impl ChartKind {
    pub const ALL: [ChartKind; 3] = [
        ChartKind::VolumeCandles,
        ChartKind::Equivolume,
        ChartKind::Candles,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ChartKind::VolumeCandles => "Volume candles",
            ChartKind::Equivolume => "Equivolume",
            ChartKind::Candles => "Candles",
        }
    }

    pub fn help(self) -> &'static str {
        match self {
            ChartKind::VolumeCandles => {
                "Candles stay in their time slot, and the body is wider the higher the volume \
                 (the widest is the largest volume on screen, or over the last N bars; see Settings)."
            }
            ChartKind::Equivolume => {
                "Each candle takes horizontal space in proportion to its volume (CandleVolume): \
                 busy periods stretch out and quiet ones shrink, so the time axis is not linear."
            }
            ChartKind::Candles => "Regular candlesticks, all the same width.",
        }
    }
}

/// What the widest volume candle is compared against.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WidthRef {
    /// The largest volume among the bars on screen.
    Visible,
    /// The largest volume of the last `lookback` bars (stable while panning).
    Lookback,
}

/// User-facing chart options (persisted).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Style {
    pub kind: ChartKind,
    pub width_ref: WidthRef,
    pub lookback: usize,
    pub show_volume: bool,
    pub log_scale: bool,
    /// Color by close vs. previous close instead of close vs. open.
    pub prev_close_colors: bool,
    pub dark: bool,
    /// Added to every displayed time (e.g. +420 shows UTC data in Bangkok time).
    pub tz_offset_min: i32,
    /// CDC ActionZone V3 2020 (piriya33) on the price pane.
    pub cdc: CdcSettings,
    /// RSI in its own pane under the prices.
    pub rsi: RsiSettings,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            kind: ChartKind::VolumeCandles,
            width_ref: WidthRef::Visible,
            lookback: 50,
            show_volume: true,
            log_scale: false,
            prev_close_colors: false,
            dark: true,
            tz_offset_min: 0,
            cdc: CdcSettings::default(),
            rsi: RsiSettings::default(),
        }
    }
}

/// The color CDC ActionZone paints a bar in each zone.
pub fn zone_color(z: Zone) -> Color32 {
    match z {
        Zone::Green => pine::GREEN,
        Zone::Blue => pine::BLUE,
        Zone::LightBlue => pine::AQUA,
        Zone::Red => pine::RED,
        Zone::Orange => pine::ORANGE,
        Zone::Yellow => pine::YELLOW,
    }
}

fn trend_color(t: Trend) -> Color32 {
    match t {
        Trend::Bullish => pine::GREEN,
        Trend::Bearish => pine::RED,
        Trend::Sideways => pine::BLACK,
    }
}

/// What the chart shows (already resampled).
pub struct ChartData {
    pub candles: Vec<Candle>,
    pub name: String,
    pub timeframe: String,
    /// Bar spacing in seconds (0 if unknown).
    pub interval: i64,
    pub decimals: usize,
    pub has_volume: bool,
    /// Show clock times (intraday bars).
    pub intraday: bool,
    /// Changes whenever `candles` change.
    pub generation: u64,
}

enum Anchor {
    /// Show the latest bars at the default zoom.
    Reset,
    /// Keep the bar width and the time at the right edge (reloads, timeframe changes).
    Keep {
        right_t: i64,
        at_end: bool,
        margin_bars: f64,
        px_per_bar: f64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragTarget {
    Plot,
    PriceAxis,
    TimeAxis,
}

/// Price ↔ screen y.
#[derive(Clone, Copy)]
struct YMap {
    lo: f64,
    hi: f64,
    top: f32,
    bottom: f32,
    log: bool,
}

fn to_scale(p: f64, log: bool) -> f64 {
    if log {
        p.max(1e-12).ln()
    } else {
        p
    }
}

fn from_scale(v: f64, log: bool) -> f64 {
    if log {
        v.exp()
    } else {
        v
    }
}

impl YMap {
    fn y(&self, price: f64) -> f32 {
        let f = (to_scale(price, self.log) - self.lo) / (self.hi - self.lo);
        (f64::from(self.bottom) - f * f64::from(self.bottom - self.top)).clamp(-1e6, 1e6) as f32
    }

    fn price(&self, y: f32) -> f64 {
        let f = f64::from(self.bottom - y) / f64::from(self.bottom - self.top);
        from_scale(self.lo + f * (self.hi - self.lo), self.log)
    }
}

/// One bar (or several merged into a pixel column) in physical pixels.
struct Bar {
    /// Index of the (last) candle drawn by this bar.
    last: usize,
    /// The bar's whole time slot.
    x0: f32,
    x1: f32,
    left: f32,
    right: f32,
    wick: f32,
    vleft: f32,
    vright: f32,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    volume: f64,
    up: bool,
    merged: bool,
}

/// Pixel-aligned rectangles, in physical pixels.
struct Pixels {
    mesh: Mesh,
    ppp: f32,
}

impl Pixels {
    fn new(ppp: f32) -> Self {
        Self {
            mesh: Mesh::default(),
            ppp,
        }
    }

    fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, color: Color32) {
        if x1 > x0 && y1 > y0 {
            let p = self.ppp;
            self.mesh.add_colored_rect(
                Rect::from_min_max(pos2(x0 / p, y0 / p), pos2(x1 / p, y1 / p)),
                color,
            );
        }
    }

    fn snap(&self, v: f32) -> f32 {
        (v * self.ppp).round()
    }

    fn hline(&mut self, x0: f32, x1: f32, y: f32, color: Color32, dash: Option<f32>) {
        let (x0, x1, row) = (self.snap(x0), self.snap(x1), (y * self.ppp).floor());
        self.dashes(x0, x1, dash, |s, a, b| s.rect(a, row, b, row + 1.0, color));
    }

    fn vline(&mut self, x: f32, y0: f32, y1: f32, color: Color32, dash: Option<f32>) {
        let (y0, y1, col) = (self.snap(y0), self.snap(y1), (x * self.ppp).floor());
        self.dashes(y0, y1, dash, |s, a, b| s.rect(col, a, col + 1.0, b, color));
    }

    fn dashes(
        &mut self,
        a: f32,
        b: f32,
        dash: Option<f32>,
        mut put: impl FnMut(&mut Self, f32, f32),
    ) {
        match dash {
            None => put(self, a, b),
            Some(len) => {
                let len = (len * self.ppp).round().max(1.0);
                let mut s = a;
                while s < b {
                    put(self, s, (s + len).min(b));
                    s += 2.0 * len;
                }
            }
        }
    }

    fn into_shape(self) -> Shape {
        Shape::mesh(self.mesh)
    }
}

pub struct Chart {
    /// World x at the right edge of the plot.
    right: f64,
    /// Pixels per world unit.
    scale: f64,
    auto_y: bool,
    /// Current price range in scale space (ln(price) on a log axis).
    y_lo: f64,
    y_hi: f64,
    y_log: bool,
    edges: Vec<f64>,
    edges_for: Option<(u64, ChartKind)>,
    roll: Vec<f64>,
    roll_for: Option<(u64, usize)>,
    anchor: Option<Anchor>,
    drag: Option<DragTarget>,
    axis_w: f32,
    /// Indicator results for (data generation, settings).
    cdc: Option<(u64, CdcSettings, Arc<Cdc>)>,
    rsi: Option<(u64, RsiSettings, Arc<Rsi>)>,
}

impl Default for Chart {
    fn default() -> Self {
        Self::new()
    }
}

impl Chart {
    pub fn new() -> Self {
        Self {
            right: 0.0,
            scale: DEFAULT_BAR_PX,
            auto_y: true,
            y_lo: 0.0,
            y_hi: 1.0,
            y_log: false,
            edges: vec![0.0],
            edges_for: None,
            roll: Vec::new(),
            roll_for: None,
            anchor: Some(Anchor::Reset),
            drag: None,
            axis_w: 72.0,
            cdc: None,
            rsi: None,
        }
    }

    /// Show the latest bars at the default zoom, with automatic price scaling.
    pub fn reset_view(&mut self) {
        self.anchor = Some(Anchor::Reset);
        self.auto_y = true;
    }

    /// Call before replacing the data with a reloaded or resampled version of
    /// `old`: keeps the bar width and the time at the right edge, and keeps
    /// following the newest bar if it is on screen.
    pub fn keep_view(&mut self, old: &ChartData) {
        let n = old.candles.len();
        if n == 0
            || self.edges.len() != n + 1
            || self.edges_for.map(|k| k.0) != Some(old.generation)
        {
            self.anchor = Some(Anchor::Reset);
            return;
        }
        let unit = self.unit();
        let total = self.edges[n];
        let idx = self.bar_at(self.right).unwrap_or(n - 1).min(n - 1);
        self.anchor = Some(Anchor::Keep {
            right_t: old.candles[idx].t,
            at_end: self.right >= total,
            margin_bars: ((self.right - total) / unit).max(0.0),
            px_per_bar: self.scale * unit,
        });
    }

    fn n(&self) -> usize {
        self.edges.len() - 1
    }

    /// Mean bar width in world units.
    fn unit(&self) -> f64 {
        let n = self.n();
        if n == 0 {
            1.0
        } else {
            (self.edges[n] / n as f64).max(1e-9)
        }
    }

    fn sx(&self, w: f64, plot: &Rect) -> f32 {
        plot.max.x - ((self.right - w) * self.scale) as f32
    }

    fn world(&self, x: f32, plot: &Rect) -> f64 {
        self.right - f64::from(plot.max.x - x) / self.scale
    }

    fn mid(&self, i: usize) -> f64 {
        (self.edges[i] + self.edges[i + 1]) * 0.5
    }

    fn bar_at(&self, w: f64) -> Option<usize> {
        let i = self.edges.partition_point(|&e| e <= w);
        (1..=self.n()).contains(&i).then(|| i - 1)
    }

    /// First and last bar that are at least partly on screen.
    fn visible(&self, plot: &Rect) -> Option<(usize, usize)> {
        let n = self.n();
        let (w0, w1) = (self.world(plot.min.x, plot), self.right);
        let first = self.edges[1..].partition_point(|&e| e <= w0);
        let end = self.edges[..n].partition_point(|&e| e < w1);
        (first < end).then(|| (first, end - 1))
    }

    fn zoom_at(&mut self, x: f32, plot: &Rect, factor: f64) {
        let w = self.world(x, plot);
        self.scale *= factor;
        self.clamp_scale(f64::from(plot.width()));
        self.right = w + f64::from(plot.max.x - x) / self.scale;
    }

    fn clamp_scale(&mut self, plot_w: f64) {
        let (unit, total) = (self.unit(), self.edges[self.n()]);
        let max = MAX_BAR_PX / unit;
        let min = (plot_w / (total + 10.0 * unit)).min(max);
        self.scale = self.scale.clamp(min, max);
    }

    fn clamp_view(&mut self, plot_w: f64) {
        if self.n() == 0 {
            return;
        }
        self.clamp_scale(plot_w);
        let total = self.edges[self.n()];
        let span = plot_w / self.scale;
        let min_right = (span * 0.15).min(total);
        let max_right = (total + span * 0.85).max(min_right);
        self.right = self.right.clamp(min_right, max_right);
    }

    fn ensure_edges(&mut self, data: &ChartData, kind: ChartKind, plot_w: f64) {
        let key = (data.generation, kind);
        if self.edges_for == Some(key) {
            return;
        }
        let n = data.candles.len();
        // Same data, other chart type: keep the same bars on screen.
        let carry = match self.edges_for {
            Some((g, _)) if g == data.generation && self.edges.len() == n + 1 => Some((
                world_to_index(&self.edges, self.right - plot_w / self.scale),
                world_to_index(&self.edges, self.right),
            )),
            _ => None,
        };
        if carry.is_none() && self.anchor.is_none() {
            self.anchor = Some(Anchor::Reset);
        }
        self.edges = build_edges(&data.candles, kind);
        self.edges_for = Some(key);
        if let Some((l, r)) = carry {
            let (wl, wr) = (
                index_to_world(&self.edges, l),
                index_to_world(&self.edges, r),
            );
            if wr > wl {
                self.scale = plot_w / (wr - wl);
                self.right = wr;
            }
        }
    }

    fn ensure_roll(&mut self, data: &ChartData, lookback: usize) {
        let key = (data.generation, lookback);
        if self.roll_for != Some(key) {
            self.roll = rolling_max(&data.candles, lookback.max(1));
            self.roll_for = Some(key);
        }
    }

    /// (Re)computes the enabled indicators when the data or their settings change.
    fn ensure_indicators(&mut self, data: &ChartData, style: &Style) {
        let g = data.generation;
        if style.cdc.enabled
            && !matches!(&self.cdc, Some((cg, s, _)) if *cg == g && *s == style.cdc)
        {
            let out = indicators::cdc::compute(&data.candles, data.interval, &style.cdc);
            self.cdc = Some((g, style.cdc.clone(), Arc::new(out)));
        }
        if style.rsi.enabled
            && !matches!(&self.rsi, Some((rg, s, _)) if *rg == g && *s == style.rsi)
        {
            let out = indicators::rsi::compute(&data.candles, &style.rsi);
            self.rsi = Some((g, style.rsi.clone(), Arc::new(out)));
        }
    }

    fn apply_anchor(&mut self, data: &ChartData, plot_w: f64) {
        let n = self.n();
        let Some(anchor) = self.anchor.take() else {
            return;
        };
        if n == 0 {
            return;
        }
        let (unit, total) = (self.unit(), self.edges[n]);
        match anchor {
            Anchor::Reset => {
                let bars = ((plot_w / DEFAULT_BAR_PX).round() as usize).clamp(1, n);
                let margin = MARGIN_BARS * unit;
                self.scale = plot_w / (total - self.edges[n - bars] + margin);
                self.right = total + margin;
                self.auto_y = true;
            }
            Anchor::Keep {
                right_t,
                at_end,
                margin_bars,
                px_per_bar,
            } => {
                self.scale = px_per_bar / unit;
                self.right = if at_end {
                    total + margin_bars * unit
                } else {
                    let idx = data
                        .candles
                        .partition_point(|c| c.t <= right_t)
                        .saturating_sub(1);
                    self.edges[idx + 1]
                };
            }
        }
    }

    /// Price range (scale space) that fits the visible bars and `extra` lines
    /// (indicator values on the price scale; NaN is skipped), with padding.
    fn auto_range(
        &self,
        data: &ChartData,
        vis: Option<(usize, usize)>,
        log: bool,
        pad_top: f64,
        pad_bottom: f64,
        extra: &[&[f64]],
    ) -> (f64, f64) {
        let Some((a, b)) = vis else {
            return (self.y_lo, self.y_hi);
        };
        let (mut lo, mut hi) = data.candles[a..=b]
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), c| {
                (lo.min(c.low), hi.max(c.high))
            });
        for &v in extra.iter().flat_map(|s| s.iter()) {
            if v.is_finite() && (!log || v > 0.0) {
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
        let (mut lo, mut hi) = (to_scale(lo, log), to_scale(hi, log));
        if hi - lo < 1e-12 {
            let pad = if log {
                0.01
            } else {
                (hi.abs() * 0.01).max(1e-6)
            };
            lo -= pad;
            hi += pad;
        }
        let span = (hi - lo) / (1.0 - pad_top - pad_bottom);
        let lo = lo - span * pad_bottom;
        (lo, lo + span)
    }

    fn scale_y(&mut self, factor: f64) {
        let mid = (self.y_lo + self.y_hi) * 0.5;
        let half = ((self.y_hi - self.y_lo) * 0.5 * factor).max(1e-12);
        self.y_lo = mid - half;
        self.y_hi = mid + half;
    }

    /// `plot` is the area of all panes (they share the time axis); `price_h` is
    /// the height of the price pane, for moving prices by dragging.
    #[allow(clippy::too_many_arguments)]
    fn input(
        &mut self,
        ui: &Ui,
        resp: &egui::Response,
        style: &mut Style,
        plot: &Rect,
        price_h: f32,
        price_axis: &Rect,
        time_axis: &Rect,
    ) {
        if resp.drag_started() {
            let origin = ui
                .input(|i| i.pointer.press_origin())
                .unwrap_or(plot.center());
            self.drag = if price_axis.contains(origin) {
                Some(DragTarget::PriceAxis)
            } else if time_axis.contains(origin) {
                Some(DragTarget::TimeAxis)
            } else if plot.contains(origin) {
                Some(DragTarget::Plot)
            } else {
                None
            };
            if self.drag == Some(DragTarget::PriceAxis) {
                self.auto_y = false;
            }
        }
        if resp.dragged() {
            let d = resp.drag_delta();
            match self.drag {
                Some(DragTarget::Plot) => {
                    self.right -= f64::from(d.x) / self.scale;
                    if !self.auto_y {
                        let dy = f64::from(d.y) / f64::from(price_h) * (self.y_hi - self.y_lo);
                        self.y_lo += dy;
                        self.y_hi += dy;
                    }
                }
                Some(DragTarget::PriceAxis) => self.scale_y((f64::from(d.y) * 0.005).exp()),
                Some(DragTarget::TimeAxis) => {
                    self.zoom_at(plot.max.x, plot, (f64::from(d.x) * 0.006).exp())
                }
                None => {}
            }
        }
        if resp.drag_stopped() {
            self.drag = None;
        }

        if let Some(p) = resp.hover_pos() {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
            if price_axis.contains(p) {
                let f = (-f64::from(scroll.y) * 0.003).exp() / f64::from(zoom);
                if (f - 1.0).abs() > 1e-9 {
                    self.auto_y = false;
                    self.scale_y(f);
                }
            } else {
                // While the newest bar is on screen it stays put (like TradingView);
                // otherwise zoom around the mouse.
                let at_end = self.right >= self.edges[self.n()];
                let anchor = if at_end { plot.max.x } else { p.x };
                if zoom != 1.0 {
                    self.zoom_at(anchor, plot, f64::from(zoom));
                }
                if scroll.y != 0.0 {
                    self.zoom_at(anchor, plot, (f64::from(scroll.y) * 0.0025).exp());
                }
                if scroll.x != 0.0 {
                    self.right -= f64::from(scroll.x) / self.scale;
                }
            }
            if resp.double_clicked() {
                if price_axis.contains(p) {
                    self.auto_y = true;
                } else if plot.contains(p) {
                    self.reset_view();
                }
            }
            let icon = if self.drag == Some(DragTarget::Plot) && resp.dragged() {
                CursorIcon::Grabbing
            } else if price_axis.contains(p) {
                CursorIcon::ResizeVertical
            } else if time_axis.contains(p) {
                CursorIcon::ResizeHorizontal
            } else {
                CursorIcon::Crosshair
            };
            ui.ctx().set_cursor_icon(icon);
        }

        if ui.ctx().wants_keyboard_input() {
            return;
        }
        let keys = ui.input(|i| {
            // Counted, so key repeats are not lost when frames are slow.
            let n = |key| i.num_presses(key) as i32;
            let k = |key| i.key_pressed(key);
            (
                n(Key::ArrowRight) - n(Key::ArrowLeft),
                n(Key::ArrowUp) + n(Key::Plus) + n(Key::Equals) - n(Key::ArrowDown) - n(Key::Minus),
                k(Key::Home),
                k(Key::End),
                i.modifiers.shift,
                [k(Key::A), k(Key::L), k(Key::V), k(Key::R)],
                [k(Key::Num1), k(Key::Num2), k(Key::Num3)],
            )
        });
        let (pan, zoom, home, end, shift, [a, l, v, r], kinds) = keys;
        if pan != 0 {
            self.right += f64::from(pan) * if shift { 10.0 } else { 1.0 } * self.unit();
        }
        if zoom != 0 {
            self.zoom_at(plot.max.x, plot, 1.25f64.powi(zoom));
        }
        if home {
            self.right = f64::from(plot.width()) / self.scale;
        }
        if end {
            self.right = self.edges[self.n()] + MARGIN_BARS * self.unit();
        }
        if a {
            self.auto_y = !self.auto_y;
        }
        if l {
            style.log_scale = !style.log_scale;
        }
        if v {
            style.show_volume = !style.show_volume;
        }
        if r {
            self.reset_view();
        }
        for (pressed, kind) in kinds.into_iter().zip(ChartKind::ALL) {
            if pressed {
                style.kind = kind;
            }
        }
    }

    /// Draws the chart into all of `ui`'s remaining space and handles input.
    pub fn ui(&mut self, ui: &mut Ui, data: &ChartData, style: &mut Style) {
        let theme = Theme::new(style.dark);
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme.bg);
        let n = data.candles.len();
        if n == 0 || rect.width() < 160.0 || rect.height() < 100.0 {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "No data",
                FontId::proportional(16.0),
                theme.muted,
            );
            return;
        }
        let ppp = ui.ctx().pixels_per_point();
        let small = FontId::proportional(12.0);
        // Panes: prices on top, the RSI (when shown) under them, one time axis for both.
        let area = Rect::from_min_max(
            rect.min,
            pos2(rect.max.x - self.axis_w, rect.max.y - TIME_AXIS_H),
        );
        let rsi_h = if style.rsi.enabled && area.height() >= 3.0 * RSI_MIN_H {
            (area.height() * RSI_SHARE).max(RSI_MIN_H).round()
        } else {
            0.0
        };
        let plot = Rect::from_min_max(area.min, pos2(area.max.x, area.max.y - rsi_h));
        let rsi_pane =
            (rsi_h > 0.0).then(|| Rect::from_min_max(pos2(area.min.x, plot.max.y), area.max));
        let price_axis =
            Rect::from_min_max(pos2(plot.max.x, rect.min.y), pos2(rect.max.x, plot.max.y));
        let time_axis =
            Rect::from_min_max(pos2(rect.min.x, area.max.y), pos2(area.max.x, rect.max.y));
        let corner = Rect::from_min_max(area.max, rect.max);
        let plot_w = f64::from(plot.width());
        // Shifting daily bars would move them to another date.
        let tz = if data.intraday {
            i64::from(style.tz_offset_min) * 60
        } else {
            0
        };

        self.ensure_edges(data, style.kind, plot_w);
        if style.kind == ChartKind::VolumeCandles && style.width_ref == WidthRef::Lookback {
            self.ensure_roll(data, style.lookback);
        }
        self.apply_anchor(data, plot_w);
        self.input(
            ui,
            &resp,
            style,
            &area,
            plot.height(),
            &price_axis,
            &time_axis,
        );
        self.clamp_view(plot_w);
        if style.log_scale != self.y_log {
            self.y_log = style.log_scale;
            self.auto_y = true;
        }
        self.ensure_indicators(data, style);
        let cdc_out = self
            .cdc
            .as_ref()
            .filter(|_| style.cdc.enabled)
            .map(|c| Arc::clone(&c.2));
        let cdc = cdc_out.as_deref();
        let rsi_out = self
            .rsi
            .as_ref()
            .filter(|_| rsi_pane.is_some())
            .map(|c| Arc::clone(&c.2));
        let rsi = rsi_out.as_deref();

        let vis = self.visible(&plot);
        // Bars denser than pixels: indicator lines are drawn per pixel column.
        let cols = vis
            .filter(|(a, b)| {
                (cdc.is_some() || rsi.is_some()) && (b - a + 1) as f32 * 2.0 > plot.width() * ppp
            })
            .map(|(a, b)| self.columns(a, b, &plot, ppp));
        let cols = cols.as_deref();
        let show_volume = style.show_volume && data.has_volume;
        if self.auto_y {
            let pad_bottom = if show_volume { 0.22 } else { 0.08 };
            // Like TradingView, the price scale also fits the indicator lines.
            let mut extra: Vec<&[f64]> = Vec::new();
            if let (Some(c), Some((a, b))) = (cdc, vis) {
                if style.cdc.show_fast {
                    extra.push(&c.fast[a..=b]);
                }
                if style.cdc.show_slow {
                    extra.push(&c.slow[a..=b]);
                }
            }
            (self.y_lo, self.y_hi) =
                self.auto_range(data, vis, style.log_scale, 0.1, pad_bottom, &extra);
        }
        let ymap = YMap {
            lo: self.y_lo,
            hi: self.y_hi,
            top: plot.min.y,
            bottom: plot.max.y,
            log: style.log_scale,
        };

        // --- Grid, axes ----------------------------------------------------
        let (p_lo, p_hi) = (ymap.price(plot.max.y), ymap.price(plot.min.y));
        let (ticks, tick_decimals) = axis::price_ticks(p_lo, p_hi, style.log_scale, plot.height());
        let label_decimals = data.decimals.max(tick_decimals).min(10);
        let time_marks = vis
            .map(|(a, b)| self.time_marks(data, a, b, &plot, tz))
            .unwrap_or_default();
        let placed = place_time_labels(&painter, &time_marks, &small, &plot, theme.muted);

        let mut grid = Pixels::new(ppp);
        for &p in &ticks {
            grid.hline(plot.min.x, plot.max.x, ymap.y(p), theme.grid, None);
        }
        for (x, ..) in &placed {
            grid.vline(*x, area.min.y, area.max.y, theme.grid, None);
        }
        grid.vline(plot.max.x, rect.min.y, area.max.y, theme.border, None);
        grid.hline(rect.min.x, area.max.x, area.max.y, theme.border, None);
        if let Some(pane) = rsi_pane {
            grid.hline(rect.min.x, rect.max.x, pane.min.y, theme.border, None);
        }
        painter.add(grid.into_shape());

        let plot_painter = painter.with_clip_rect(plot);
        let watermark = format!("{} · {}", data.name, data.timeframe);
        let wm_size = (plot.height() * 0.13).clamp(18.0, 72.0);
        plot_painter.text(
            plot.center(),
            Align2::CENTER_CENTER,
            watermark,
            FontId::proportional(wm_size),
            theme.watermark,
        );

        // --- Bars --------------------------------------------------------------
        let mut vis_max_volume = 0.0;
        if let Some((a, b)) = vis {
            vis_max_volume = data.candles[a..=b]
                .iter()
                .map(|c| c.volume)
                .fold(0.0, f64::max);
            if let Some(c) = cdc.filter(|_| style.cdc.show_fast && style.cdc.show_slow) {
                plot_painter.add(match cols {
                    Some(cols) => dense_fill(c, cols, &ymap, ppp),
                    None => self.cdc_fill(c, a, b, &plot, ppp, &ymap),
                });
            }
            let bars = self.build_bars(data, style, &plot, ppp, a, b, vis_max_volume);
            let mut px = Pixels::new(ppp);
            if show_volume {
                let max = bars.iter().map(|b| b.volume).fold(0.0, f64::max);
                let bottom = px.snap(plot.max.y);
                let height = plot.height() * VOLUME_SHARE * ppp;
                let alpha = if style.dark { 0.45 } else { 0.5 };
                for bar in bars.iter().filter(|_| max > 0.0) {
                    let h = ((bar.volume / max) as f32 * height).round();
                    let color = if bar.up { theme.up } else { theme.down };
                    px.rect(
                        bar.vleft,
                        bottom - h,
                        bar.vright,
                        bottom,
                        color.gamma_multiply(alpha),
                    );
                }
            }
            // CDC "Paint Bar Colors": the zone's color instead of up/down.
            let paint = cdc.filter(|_| style.cdc.paint_bars);
            for bar in &bars {
                let plain = if bar.up { theme.up } else { theme.down };
                let color = paint
                    .and_then(|c| c.zone[bar.last])
                    .map_or(plain, zone_color);
                let (yh, yl) = (px.snap(ymap.y(bar.high)), px.snap(ymap.y(bar.low)));
                px.rect(bar.wick, yh, bar.wick + 1.0, yl.max(yh + 1.0), color);
                if !bar.merged {
                    let (yo, yc) = (ymap.y(bar.open), ymap.y(bar.close));
                    let top = px.snap(yo.min(yc));
                    let bottom = px.snap(yo.max(yc)).max(top + 1.0);
                    px.rect(bar.left, top, bar.right, bottom, color);
                }
            }
            // CDC buy/sell ribbon: the trend of every bar along the top or bottom.
            if let Some(c) = cdc.filter(|_| style.cdc.ribbon != Ribbon::Off) {
                let h = (5.0 * ppp).round().max(2.0);
                let (y0, y1) = if style.cdc.ribbon == Ribbon::Top {
                    let top = px.snap(plot.min.y);
                    (top, top + h)
                } else {
                    let bottom = px.snap(plot.max.y);
                    (bottom - h, bottom)
                };
                for bar in &bars {
                    px.rect(bar.x0, y0, bar.x1, y1, trend_color(c.trend[bar.last]));
                }
            }
            plot_painter.add(px.into_shape());
            if let Some(c) = cdc {
                self.cdc_overlay(
                    &plot_painter,
                    c,
                    &style.cdc,
                    data,
                    &plot,
                    ppp,
                    &ymap,
                    (a, b),
                    cols,
                );
            }
        }

        // --- Last price line ---------------------------------------------------
        let last = &data.candles[n - 1];
        let last_up = if style.prev_close_colors && n > 1 {
            last.close >= data.candles[n - 2].close
        } else {
            last.close >= last.open
        };
        let last_color = if last_up { theme.up } else { theme.down };
        let last_y = ymap.y(last.close);
        let mut lines = Pixels::new(ppp);
        if (plot.min.y..=plot.max.y).contains(&last_y) {
            lines.hline(plot.min.x, plot.max.x, last_y, last_color, Some(2.0));
        }

        // --- Crosshair (across all panes) ----------------------------------------
        let hover = resp.hover_pos().filter(|p| area.contains(*p));
        let hovered = hover.and_then(|p| self.bar_at(self.world(p.x, &plot)));
        if let Some(p) = hover {
            let x = hovered.map_or(p.x, |i| self.sx(self.mid(i), &plot));
            lines.vline(x, area.min.y, area.max.y, theme.crosshair, Some(3.0));
            lines.hline(area.min.x, area.max.x, p.y, theme.crosshair, Some(3.0));
        }
        painter.add(lines.into_shape());

        // --- RSI pane ------------------------------------------------------------
        let mut widest: f32 = 0.0;
        if let (Some(pane), Some(r)) = (rsi_pane, rsi) {
            let axis =
                Rect::from_min_max(pos2(pane.max.x, pane.min.y), pos2(rect.max.x, pane.max.y));
            widest = widest.max(self.rsi_pane(
                &painter,
                pane,
                axis,
                &style.rsi,
                &theme,
                r,
                vis,
                cols,
                ppp,
                hovered.unwrap_or(n - 1),
                hover,
                &small,
            ));
        }

        // --- Axis labels -------------------------------------------------------
        let axis_painter = painter.with_clip_rect(price_axis);
        for &p in &ticks {
            let y = ymap.y(p);
            let g =
                painter.layout_no_wrap(format_price(p, label_decimals), small.clone(), theme.muted);
            widest = widest.max(g.size().x);
            if y > plot.min.y + 6.0 && y < plot.max.y - 6.0 {
                axis_painter.galley(
                    pos2(price_axis.min.x + 8.0, y - g.size().y / 2.0),
                    g,
                    theme.muted,
                );
            }
        }
        // The crosshair's date tag hides the time labels under it.
        let cross_tag = hover.zip(hovered).map(|(_, i)| {
            let text = datetime::format_long(data.candles[i].t + tz, data.intraday);
            let g = painter.layout_no_wrap(text, small.clone(), theme.label_text);
            let half = g.size().x / 2.0 + 8.0;
            let x = self
                .sx(self.mid(i), &plot)
                .clamp(time_axis.min.x + half, time_axis.max.x - half);
            (
                Rect::from_center_size(
                    pos2(x, time_axis.center().y),
                    vec2(half * 2.0, TIME_AXIS_H - 4.0),
                ),
                g,
            )
        });
        for (x, text, level) in &placed {
            let color = if *level >= 4 { theme.text } else { theme.muted };
            let r = painter.layout_no_wrap(text.clone(), small.clone(), color);
            let (x0, x1) = (x - r.size().x / 2.0 - 4.0, x + r.size().x / 2.0 + 4.0);
            if cross_tag
                .as_ref()
                .is_some_and(|(t, _)| x0 < t.max.x && x1 > t.min.x)
            {
                continue;
            }
            painter.galley(
                pos2(
                    x - r.size().x / 2.0,
                    time_axis.center().y - r.size().y / 2.0,
                ),
                r,
                color,
            );
        }
        widest = widest.max(axis_tag(
            &painter,
            price_axis,
            &small,
            format_price(last.close, label_decimals),
            last_y,
            last_color,
            Color32::WHITE,
        ));
        if let Some(p) = hover.filter(|p| plot.contains(*p)) {
            widest = widest.max(axis_tag(
                &painter,
                price_axis,
                &small,
                format_price(ymap.price(p.y), label_decimals),
                p.y,
                theme.label_bg,
                theme.label_text,
            ));
        }
        if let Some((r, g)) = cross_tag {
            painter.rect_filled(r, 2.0, theme.label_bg);
            painter.galley(r.center() - g.size() / 2.0, g, theme.label_text);
        }
        let want_w = (widest + 18.0).clamp(52.0, 150.0).ceil();
        if (want_w - self.axis_w).abs() >= 1.0 {
            self.axis_w = want_w;
            ui.ctx().request_repaint();
        }

        // --- Legend, corner buttons -------------------------------------------
        self.legend(
            &painter,
            &plot,
            data,
            style,
            &theme,
            hovered.unwrap_or(n - 1),
            vis_max_volume,
            tz,
            cdc,
        );
        self.corner_buttons(ui, &painter, corner, style, &theme);
    }

    /// The translucent fill between the two EMAs: green while the fast one is
    /// above, red while it is below (`fill(FastL, SlowL, fillcolor)`). One point
    /// per pixel column (the last bar in it).
    #[allow(clippy::too_many_arguments)]
    fn cdc_fill(
        &self,
        c: &Cdc,
        first: usize,
        last: usize,
        plot: &Rect,
        ppp: f32,
        ymap: &YMap,
    ) -> Shape {
        let mut mesh = Mesh::default();
        let n = c.fast.len().min(self.n());
        let (a, b) = (first.saturating_sub(1), (last + 1).min(n - 1));
        let mut pending: Option<(i64, usize, Pos2, Pos2)> = None;
        let mut prev: Option<(Pos2, Pos2)> = None;
        for i in a..=b {
            let (f, s) = (c.fast[i], c.slow[i]);
            if !(f.is_finite() && s.is_finite()) {
                if let Some((_, j, pf, ps)) = pending.take() {
                    fill_step(&mut mesh, &mut prev, pf, ps, fill_color(c, j));
                }
                prev = None;
                continue;
            }
            let x = self.sx(self.mid(i), plot);
            let col = (x * ppp).floor() as i64;
            if let Some((pc, j, pf, ps)) = pending {
                if pc != col {
                    fill_step(&mut mesh, &mut prev, pf, ps, fill_color(c, j));
                }
            }
            pending = Some((col, i, pos2(x, ymap.y(f)), pos2(x, ymap.y(s))));
        }
        if let Some((_, j, pf, ps)) = pending {
            fill_step(&mut mesh, &mut prev, pf, ps, fill_color(c, j));
        }
        Shape::mesh(mesh)
    }

    /// CDC ActionZone's lines, signals and label, drawn over the candles.
    #[allow(clippy::too_many_arguments)]
    fn cdc_overlay(
        &self,
        painter: &egui::Painter,
        c: &Cdc,
        s: &CdcSettings,
        data: &ChartData,
        plot: &Rect,
        ppp: f32,
        ymap: &YMap,
        (first, last): (usize, usize),
        cols: Option<&PixelColumns>,
    ) {
        // Fixed-timeframe mode draws plot.style_stepline.
        let step = c.fixed.is_some();
        let y = |v: f64| ymap.y(v);
        let line = |values: &[f64], color| {
            self.series(
                painter,
                cols,
                values,
                (first, last),
                plot,
                ppp,
                y,
                color,
                step,
            );
        };
        if s.show_fast {
            line(&c.fast, pine::RED);
        }
        if s.show_slow {
            line(&c.slow, pine::BLUE);
        }

        // Buy/sell circles under/over the bar; momentum triangles next to them.
        // Once bars are narrower than 2 pixels the markers would only pile up
        // into outlines along the price; the bar colors still show the zones.
        let bar_px = (self.scale * self.unit()) as f32;
        let markers = bar_px >= 2.0;
        let r = (bar_px * 0.4).clamp(1.5, 4.0);
        let gap = r.max(3.0);
        let momentum = s.momentum && c.momentum_buy.len() == c.buy.len();
        // Equivolume bars can still share a pixel column: one marker of a kind per column.
        let mut drawn = [i64::MIN; 4];
        for i in (first..=last).filter(|_| markers) {
            let buy = s.signals && c.buy[i];
            let sell = s.signals && c.sell[i];
            let mbuy = momentum && c.momentum_buy[i] > s.momentum_threshold;
            let msell = momentum && c.momentum_sell[i] > s.momentum_threshold;
            if !(buy || sell || mbuy || msell) {
                continue;
            }
            let x = self.sx(self.mid(i), plot);
            let col = (x * ppp).floor() as i64;
            let mut fresh = |k: usize, on: bool| on && std::mem::replace(&mut drawn[k], col) != col;
            let (buy, sell, mbuy, msell) = (
                fresh(0, buy),
                fresh(1, sell),
                fresh(2, mbuy),
                fresh(3, msell),
            );
            if !(buy || sell || mbuy || msell) {
                continue;
            }
            let candle = &data.candles[i];
            let mut below = ymap.y(candle.low) + gap;
            let mut above = ymap.y(candle.high) - gap;
            if buy {
                painter.circle_filled(pos2(x, below + r), r, pine::GREEN);
                below += 2.0 * r + 2.0;
            }
            if mbuy {
                triangle(painter, x, below, r * 1.2, true, pine::TEAL);
            }
            if sell {
                painter.circle_filled(pos2(x, above - r), r, pine::RED);
                above -= 2.0 * r + 2.0;
            }
            if msell {
                triangle(painter, x, above, r * 1.2, false, pine::ORANGE);
            }
        }

        if s.assistive_text && last + 1 == data.candles.len() {
            self.cdc_label(painter, c, data, plot, ymap);
        }
    }

    /// The script's "assistive text": a label at the last bar with the trend.
    fn cdc_label(
        &self,
        painter: &egui::Painter,
        c: &Cdc,
        data: &ChartData,
        plot: &Rect,
        ymap: &YMap,
    ) {
        let i = data.candles.len() - 1;
        let candle = &data.candles[i];
        let (buy, sell) = (c.buy[i], c.sell[i]);
        let head = if buy {
            "BUY next bar\n"
        } else if sell {
            "SELL next bar\n"
        } else {
            ""
        };
        let text = format!(
            "{head}{} {}\ncurrently in a {} trend",
            data.name,
            format_price(candle.close, data.decimals),
            c.trend[i].label()
        );
        let bg = if buy {
            pine::GREEN
        } else if sell {
            pine::RED
        } else {
            pine::SILVER
        };
        let rising = i > 0 && candle.close > data.candles[i - 1].close;
        let fg = if buy {
            Color32::BLACK
        } else if sell {
            Color32::WHITE
        } else if rising {
            pine::GREEN
        } else {
            pine::RED
        };
        // label.style_label_down above the bar when the close is above the slow EMA.
        let above = candle.close > c.slow[i];
        let g = painter.layout_no_wrap(text, FontId::proportional(11.5), fg);
        let size = g.size() + vec2(12.0, 8.0);
        let x = self.sx(self.mid(i), plot);
        let pointer = 6.0;
        let (top, tip) = if above {
            let tip = ymap.y(candle.high) - 4.0;
            (tip - pointer - size.y, tip)
        } else {
            let tip = ymap.y(candle.low) + 4.0;
            (tip + pointer, tip)
        };
        let left = (x - size.x / 2.0).clamp(
            plot.min.x + 2.0,
            (plot.max.x - size.x - 2.0).max(plot.min.x + 2.0),
        );
        let r = Rect::from_min_size(pos2(left, top), size);
        let base = if above { r.max.y } else { r.min.y };
        let arrow = if above {
            vec![
                pos2(x - pointer, base),
                pos2(x + pointer, base),
                pos2(x, tip),
            ]
        } else {
            vec![
                pos2(x - pointer, base),
                pos2(x, tip),
                pos2(x + pointer, base),
            ]
        };
        painter.add(Shape::convex_polygon(arrow, bg, Stroke::NONE));
        painter.rect_filled(r, 3.0, bg);
        painter.galley(r.min + vec2(6.0, 4.0), g, fg);
    }

    /// The RSI pane: 0–100 scale, overbought/oversold band, the RSI and its
    /// moving average, a legend and axis labels. Returns the widest axis label.
    #[allow(clippy::too_many_arguments)]
    fn rsi_pane(
        &self,
        painter: &egui::Painter,
        pane: Rect,
        axis: Rect,
        s: &RsiSettings,
        theme: &Theme,
        r: &Rsi,
        vis: Option<(usize, usize)>,
        cols: Option<&PixelColumns>,
        ppp: f32,
        idx: usize,
        hover: Option<Pos2>,
        font: &FontId,
    ) -> f32 {
        let map = YMap {
            lo: 0.0,
            hi: 100.0,
            top: pane.min.y + 8.0,
            bottom: pane.max.y - 8.0,
            log: false,
        };
        let clip = painter.with_clip_rect(pane);
        let (y_ob, y_os) = (map.y(s.overbought), map.y(s.oversold));
        clip.rect_filled(
            Rect::from_min_max(
                pos2(pane.min.x, y_ob.min(y_os)),
                pos2(pane.max.x, y_ob.max(y_os)),
            ),
            0.0,
            pine::transp(pine::RSI, 90),
        );
        let mut px = Pixels::new(ppp);
        px.hline(pane.min.x, pane.max.x, y_ob, pine::GRAY, Some(4.0));
        px.hline(
            pane.min.x,
            pane.max.x,
            map.y(50.0),
            pine::transp(pine::GRAY, 50),
            Some(1.0),
        );
        px.hline(pane.min.x, pane.max.x, y_os, pine::GRAY, Some(4.0));
        clip.add(px.into_shape());
        if let Some(range) = vis {
            let y = |v: f64| map.y(v);
            self.series(&clip, cols, &r.rsi, range, &pane, ppp, y, pine::RSI, false);
            if s.show_ma {
                self.series(
                    &clip,
                    cols,
                    &r.ma,
                    range,
                    &pane,
                    ppp,
                    y,
                    pine::YELLOW,
                    false,
                );
            }
        }

        let value = |v: f64| {
            if v.is_finite() {
                format!("{v:.2}")
            } else {
                "–".into()
            }
        };
        let legend_font = FontId::proportional(12.5);
        let mut line = vec![
            (
                format!("RSI {} {}  ", s.length, s.source.label()),
                theme.muted,
                legend_font.clone(),
            ),
            (value(r.rsi[idx]), pine::RSI, legend_font.clone()),
        ];
        if s.show_ma {
            line.push((
                format!("   MA {}  ", s.ma_length),
                theme.muted,
                legend_font.clone(),
            ));
            line.push((value(r.ma[idx]), pine::YELLOW, legend_font));
        }
        draw_legend(painter, pane.min.x, pane.min.y + 5.0, vec![line], theme);

        let mut widest: f32 = 0.0;
        for v in [s.overbought, 50.0, s.oversold] {
            let g = painter.layout_no_wrap(format!("{v:.2}"), font.clone(), theme.muted);
            widest = widest.max(g.size().x);
            painter.galley(
                pos2(axis.min.x + 8.0, map.y(v) - g.size().y / 2.0),
                g,
                theme.muted,
            );
        }
        if s.show_ma {
            if let Some(&v) = r.ma.last().filter(|v| v.is_finite()) {
                let w = axis_tag(
                    painter,
                    axis,
                    font,
                    format!("{v:.2}"),
                    map.y(v),
                    pine::YELLOW,
                    Color32::BLACK,
                );
                widest = widest.max(w);
            }
        }
        if let Some(&v) = r.rsi.last().filter(|v| v.is_finite()) {
            let w = axis_tag(
                painter,
                axis,
                font,
                format!("{v:.2}"),
                map.y(v),
                pine::RSI,
                Color32::WHITE,
            );
            widest = widest.max(w);
        }
        if let Some(p) = hover.filter(|p| pane.contains(*p)) {
            let w = axis_tag(
                painter,
                axis,
                font,
                format!("{:.2}", map.price(p.y)),
                p.y,
                theme.label_bg,
                theme.label_text,
            );
            widest = widest.max(w);
        }
        widest
    }

    /// Bars `first..=last` grouped by the physical pixel column of their center.
    fn columns(
        &self,
        first: usize,
        last: usize,
        plot: &Rect,
        ppp: f32,
    ) -> Vec<(f32, usize, usize)> {
        let (origin, per_unit) = (
            (f64::from(plot.max.x) - self.right * self.scale) * f64::from(ppp),
            self.scale * f64::from(ppp),
        );
        let mut out: Vec<(f32, usize, usize)> = Vec::new();
        for i in first..=last {
            let x = ((origin + self.mid(i) * per_unit) as f32).floor();
            match out.last_mut() {
                Some(c) if c.0 == x => c.2 = i,
                _ => out.push((x, i, i)),
            }
        }
        out
    }

    /// Draws `values` as a line: anti-aliased through the bar centers, or, when
    /// `cols` is given (bars denser than pixels), as one crisp span per column.
    #[allow(clippy::too_many_arguments)]
    fn series(
        &self,
        painter: &egui::Painter,
        cols: Option<&PixelColumns>,
        values: &[f64],
        (first, last): (usize, usize),
        plot: &Rect,
        ppp: f32,
        y: impl Fn(f64) -> f32,
        color: Color32,
        step: bool,
    ) {
        if let Some(cols) = cols {
            painter.add(dense_line(cols, values, y, color, ppp));
        } else {
            let segs = self.line_segments(values, first, last, plot, ppp, y);
            draw_line(painter, segs, color, step);
        }
    }

    /// Screen points of a line through `values` at the bar centers of
    /// `first..=last`, plus one bar beyond each side so the line runs off the
    /// edges. Where several bars share a pixel column only the column's first,
    /// highest, lowest and last points are kept. NaN breaks the line.
    fn line_segments(
        &self,
        values: &[f64],
        first: usize,
        last: usize,
        plot: &Rect,
        ppp: f32,
        y: impl Fn(f64) -> f32,
    ) -> Vec<Vec<Pos2>> {
        let n = values.len().min(self.n());
        let mut segs = Vec::new();
        if n == 0 {
            return segs;
        }
        let (a, b) = (first.saturating_sub(1), (last + 1).min(n - 1));
        let mut cur: Vec<Pos2> = Vec::new();
        let mut col: Option<Column> = None;
        for (i, &v) in values.iter().enumerate().take(b + 1).skip(a) {
            if !v.is_finite() {
                if let Some(c) = col.take() {
                    c.emit(&mut cur);
                }
                if cur.len() >= 2 {
                    segs.push(std::mem::take(&mut cur));
                } else {
                    cur.clear();
                }
                continue;
            }
            let p = pos2(self.sx(self.mid(i), plot), y(v));
            let c = (p.x * ppp).floor() as i64;
            match &mut col {
                Some(k) if k.col == c => k.add(i, p),
                _ => {
                    if let Some(k) = col.take() {
                        k.emit(&mut cur);
                    }
                    col = Some(Column::new(c, i, p));
                }
            }
        }
        if let Some(k) = col.take() {
            k.emit(&mut cur);
        }
        if cur.len() >= 2 {
            segs.push(cur);
        }
        segs
    }

    #[allow(clippy::too_many_arguments)]
    fn build_bars(
        &self,
        data: &ChartData,
        style: &Style,
        plot: &Rect,
        ppp: f32,
        first: usize,
        last: usize,
        vis_max: f64,
    ) -> Vec<Bar> {
        let candles = &data.candles;
        let mut bars: Vec<Bar> =
            Vec::with_capacity((last - first + 1).min(4 * plot.width() as usize));
        // World x → physical pixel x is affine; each bar's right edge is the next one's left.
        let (origin, per_unit) = (
            (f64::from(plot.max.x) - self.right * self.scale) * f64::from(ppp),
            self.scale * f64::from(ppp),
        );
        let px = |w: f64| (origin + w * per_unit) as f32;
        let mut x1 = px(self.edges[first]);
        for (i, c) in candles.iter().enumerate().take(last + 1).skip(first) {
            let x0 = x1;
            x1 = px(self.edges[i + 1]);
            let slot = x1 - x0;
            let up = if style.prev_close_colors && i > 0 {
                c.close >= candles[i - 1].close
            } else {
                c.close >= c.open
            };
            if slot < 2.5 {
                let col = ((x0 + x1) * 0.5).floor();
                match bars.last_mut().filter(|b| b.merged && b.wick == col) {
                    Some(b) => {
                        b.last = i;
                        b.high = b.high.max(c.high);
                        b.low = b.low.min(c.low);
                        b.close = c.close;
                        b.volume += c.volume;
                        b.up = b.close >= b.open;
                    }
                    None => bars.push(Bar {
                        last: i,
                        x0: col,
                        x1: col + 1.0,
                        left: col,
                        right: col + 1.0,
                        wick: col,
                        vleft: col,
                        vright: col + 1.0,
                        open: c.open,
                        high: c.high,
                        low: c.low,
                        close: c.close,
                        volume: c.volume,
                        up,
                        merged: true,
                    }),
                }
                continue;
            }
            let center = ((x0 + x1) * 0.5).floor();
            let centered = |w: f32| {
                let half = ((w.max(1.0) - 1.0) * 0.5).round();
                (center - half, center + half + 1.0)
            };
            let span = {
                let l = x0.round() + if slot >= 4.0 { 1.0 } else { 0.0 };
                (l, x1.round().max(l + 1.0))
            };
            let (vleft, vright) = if style.kind == ChartKind::Equivolume {
                span
            } else {
                centered(slot * 0.72)
            };
            let (left, right) = match style.kind {
                ChartKind::Candles | ChartKind::Equivolume => (vleft, vright),
                ChartKind::VolumeCandles => {
                    let reference = match style.width_ref {
                        WidthRef::Visible => vis_max,
                        WidthRef::Lookback => self.roll.get(i).copied().unwrap_or(vis_max),
                    };
                    let ratio = if reference > 0.0 {
                        (c.volume / reference).clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    centered(slot * 0.94 * ratio as f32)
                }
            };
            bars.push(Bar {
                last: i,
                x0: x0.round(),
                x1: x1.round(),
                left,
                right,
                wick: ((left + right) * 0.5 - 0.5).floor(),
                vleft,
                vright,
                open: c.open,
                high: c.high,
                low: c.low,
                close: c.close,
                volume: c.volume,
                up,
                merged: false,
            });
        }
        bars
    }

    /// Candidate time labels: (x, text, importance) wherever a period starts.
    fn time_marks(
        &self,
        data: &ChartData,
        first: usize,
        last: usize,
        plot: &Rect,
        tz: i64,
    ) -> Vec<(f32, String, u8)> {
        let c = &data.candles;
        let (xa, xb) = (
            self.sx(self.mid(first), plot),
            self.sx(self.mid(last), plot),
        );
        let secs_per_px = if xb - xa > 1.0 {
            (c[last].t - c[first].t) as f64 / f64::from(xb - xa)
        } else {
            data.interval.max(60) as f64
        };
        let step = axis::TimeStep::pick(secs_per_px, 90.0, data.interval);
        // With several bars per pixel, looking at one bar per pixel is enough:
        // a label then lands at most one pixel late.
        let stride = ((last - first) as f64 / f64::from(xb - xa).max(1.0))
            .floor()
            .max(1.0) as usize;
        let mut out = Vec::new();
        let mut prev: Option<(i64, datetime::Parts)> = None;
        let mut cached: (i64, (i64, u32, u32)) = (i64::MIN, (0, 0, 0));
        for (i, candle) in c
            .iter()
            .enumerate()
            .take(last + 1)
            .skip(first.saturating_sub(1))
            .step_by(stride)
        {
            let t = candle.t + tz;
            let day = t.div_euclid(DAY);
            if day != cached.0 {
                cached = (day, datetime::civil_from_days(day));
            }
            let secs = t.rem_euclid(DAY);
            let (year, month, dom) = cached.1;
            let p = datetime::Parts {
                year,
                month,
                day: dom,
                hour: (secs / 3600) as u32,
                minute: (secs % 3600 / 60) as u32,
                second: (secs % 60) as u32,
                weekday: (day + 3).rem_euclid(7) as u32,
            };
            let key = step.key(t, &p);
            if let Some((pk, pp)) = &prev {
                if key != *pk && i >= first {
                    let (text, level) = axis::boundary_label(pp, &p);
                    out.push((self.sx(self.mid(i), plot), text, level));
                }
            }
            prev = Some((key, p));
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn legend(
        &self,
        painter: &egui::Painter,
        plot: &Rect,
        data: &ChartData,
        style: &Style,
        theme: &Theme,
        idx: usize,
        vis_max: f64,
        tz: i64,
        cdc: Option<&Cdc>,
    ) {
        let c = &data.candles[idx];
        let prev = idx.checked_sub(1).map(|i| data.candles[i].close);
        let up = if style.prev_close_colors {
            c.close >= prev.unwrap_or(c.open)
        } else {
            c.close >= c.open
        };
        let col = if up { theme.up } else { theme.down };
        let d = data.decimals;
        let base = prev.unwrap_or(c.open);
        let change = c.close - base;
        let pct = if base != 0.0 {
            change / base * 100.0
        } else {
            0.0
        };
        let sign = if change >= 0.0 { "+" } else { "" };
        let big = FontId::proportional(15.0);
        let small = FontId::proportional(12.5);
        let line1 = vec![
            (data.name.clone(), theme.text, big.clone()),
            (format!(" · {}  ", data.timeframe), theme.muted, big),
            ("O ".into(), theme.muted, small.clone()),
            (format_price(c.open, d), col, small.clone()),
            ("  H ".into(), theme.muted, small.clone()),
            (format_price(c.high, d), col, small.clone()),
            ("  L ".into(), theme.muted, small.clone()),
            (format_price(c.low, d), col, small.clone()),
            ("  C ".into(), theme.muted, small.clone()),
            (format_price(c.close, d), col, small.clone()),
            (
                format!("  {sign}{} ({sign}{pct:.2}%)", format_price(change, d)),
                col,
                small.clone(),
            ),
        ];
        let mut line2 = vec![
            (
                datetime::format_long(c.t + tz, data.intraday),
                theme.muted,
                small.clone(),
            ),
            ("   Vol ".into(), theme.muted, small.clone()),
            (format_volume(c.volume), col, small.clone()),
        ];
        if !data.has_volume {
            line2.push((
                "   no volume in this data: all candles have the same width".into(),
                theme.warn,
                small.clone(),
            ));
        } else {
            match style.kind {
                ChartKind::VolumeCandles => {
                    let reference = match style.width_ref {
                        WidthRef::Visible => {
                            format!("largest on screen ({})", format_volume(vis_max))
                        }
                        WidthRef::Lookback => format!(
                            "largest of last {} bars ({})",
                            style.lookback,
                            format_volume(self.roll.get(idx).copied().unwrap_or(0.0))
                        ),
                    };
                    line2.push((
                        format!("   width = volume ÷ {reference}"),
                        theme.muted,
                        small.clone(),
                    ));
                }
                ChartKind::Equivolume => {
                    line2.push((
                        "   width ∝ volume (equivolume)".into(),
                        theme.muted,
                        small.clone(),
                    ));
                }
                ChartKind::Candles => {}
            }
        }
        let mut lines = vec![line1, line2];
        if let Some(cdc) = cdc {
            lines.push(cdc_legend(cdc, &style.cdc, idx, d, theme, &small));
        }
        draw_legend(painter, plot.min.x, plot.min.y + 8.0, lines, theme);
    }

    fn corner_buttons(
        &mut self,
        ui: &Ui,
        painter: &egui::Painter,
        corner: Rect,
        style: &mut Style,
        theme: &Theme,
    ) {
        let half = corner.width() / 2.0;
        let rects = [
            Rect::from_min_size(corner.min, vec2(half, corner.height())),
            Rect::from_min_size(corner.min + vec2(half, 0.0), vec2(half, corner.height())),
        ];
        let font = FontId::proportional(11.0);
        let labels = [
            ("log", style.log_scale, "Logarithmic price scale (L)"),
            (
                "auto",
                self.auto_y,
                "Fit prices to the visible bars (A).\nDrag the price axis to scale it yourself.",
            ),
        ];
        for (i, (r, (label, active, tip))) in rects.into_iter().zip(labels).enumerate() {
            let resp = ui
                .interact(r, ui.id().with(("corner", i)), Sense::click())
                .on_hover_text(tip);
            if resp.hovered() {
                painter.rect_filled(r.shrink(2.0), 3.0, theme.grid);
            }
            let color = if active { theme.accent } else { theme.muted };
            painter.text(
                r.center(),
                Align2::CENTER_CENTER,
                label,
                font.clone(),
                color,
            );
            if resp.clicked() {
                if i == 0 {
                    style.log_scale = !style.log_scale;
                } else {
                    self.auto_y = !self.auto_y;
                }
            }
        }
    }
}

/// Chooses which time labels to draw: the most important first (years, then
/// months, …), skipping any that would overlap. Returns (x, text, level).
fn place_time_labels(
    painter: &egui::Painter,
    marks: &[(f32, String, u8)],
    font: &FontId,
    plot: &Rect,
    color: Color32,
) -> Vec<(f32, String, u8)> {
    let mut order: Vec<usize> = (0..marks.len()).collect();
    order.sort_by(|&a, &b| {
        marks[b]
            .2
            .cmp(&marks[a].2)
            .then(marks[a].0.total_cmp(&marks[b].0))
    });
    let mut taken: Vec<(f32, f32)> = Vec::new();
    let mut out = Vec::new();
    for i in order {
        let (x, text, level) = &marks[i];
        let w = painter
            .layout_no_wrap(text.clone(), font.clone(), color)
            .size()
            .x;
        let (x0, x1) = (x - w / 2.0 - 10.0, x + w / 2.0 + 10.0);
        if x0 < plot.min.x || x1 > plot.max.x || taken.iter().any(|(a, b)| x0 < *b && x1 > *a) {
            continue;
        }
        taken.push((x0, x1));
        out.push((*x, text.clone(), *level));
    }
    out
}

type LegendLine = Vec<(String, Color32, FontId)>;

/// Draws legend lines (pieces of text in their colors) on a translucent
/// background, starting at (`x`, `y`). Returns the y below the last line.
fn draw_legend(
    painter: &egui::Painter,
    x: f32,
    mut y: f32,
    lines: Vec<LegendLine>,
    theme: &Theme,
) -> f32 {
    for line in lines {
        let galleys: Vec<_> = line
            .into_iter()
            .map(|(t, c, f)| painter.layout_no_wrap(t, f, c))
            .collect();
        let w: f32 = galleys.iter().map(|g| g.size().x).sum();
        let h = galleys.iter().map(|g| g.size().y).fold(0.0, f32::max);
        let bg = Rect::from_min_size(pos2(x + 6.0, y - 2.0), vec2(w + 10.0, h + 4.0));
        painter.rect_filled(bg, 3.0, theme.bg.gamma_multiply(0.8));
        let mut gx = x + 11.0;
        for g in galleys {
            let size = g.size();
            painter.galley(pos2(gx, y + h - size.y), g, theme.text);
            gx += size.x;
        }
        y += h + 5.0;
    }
    y
}

/// The CDC ActionZone legend line for bar `idx`.
fn cdc_legend(
    c: &Cdc,
    s: &CdcSettings,
    idx: usize,
    decimals: usize,
    theme: &Theme,
    font: &FontId,
) -> LegendLine {
    let value = |v: f64| {
        if v.is_finite() {
            format_price(v, decimals)
        } else {
            "–".into()
        }
    };
    let piece = |t: String, c: Color32| (t, c, font.clone());
    let mut line = vec![piece(
        format!("CDC ActionZone {} {}", s.fast, s.slow),
        theme.muted,
    )];
    if let Some(tf) = c.fixed {
        line.push(piece(format!(" · fixed {}", tf.label()), theme.muted));
    }
    line.push(piece("   Fast ".into(), theme.muted));
    line.push(piece(value(c.fast[idx]), pine::RED));
    line.push(piece("  Slow ".into(), theme.muted));
    line.push(piece(value(c.slow[idx]), pine::BLUE));
    if let Some(z) = c.zone[idx] {
        line.push(piece(
            format!("   {} zone ({})", z.name(), z.meaning()),
            zone_color(z),
        ));
    }
    let trend = c.trend[idx];
    let trend_text = match trend {
        Trend::Sideways => theme.muted,
        t => trend_color(t),
    };
    line.push(piece(format!(" · {} trend", trend.label()), trend_text));
    if c.buy[idx] {
        line.push(piece("   BUY".into(), pine::GREEN));
    }
    if c.sell[idx] {
        line.push(piece("   SELL".into(), pine::RED));
    }
    if c.fixed_ignored {
        line.push(piece(
            "   (fixed timeframe off: it must be above the chart's)".into(),
            theme.warn,
        ));
    }
    line
}

/// A value tag on an axis at height `y`, kept inside the axis. Returns the
/// width of its text.
fn axis_tag(
    painter: &egui::Painter,
    axis: Rect,
    font: &FontId,
    text: String,
    y: f32,
    bg: Color32,
    fg: Color32,
) -> f32 {
    let g = painter.layout_no_wrap(text, font.clone(), fg);
    let half = g.size().y / 2.0;
    let lo = axis.min.y + half + 2.0;
    let y = y.clamp(lo, (axis.max.y - half - 2.0).max(lo));
    let r = Rect::from_min_max(
        pos2(axis.min.x + 1.0, y - half - 3.0),
        pos2(axis.max.x, y + half + 3.0),
    );
    painter.rect_filled(r, 2.0, bg);
    let w = g.size().x;
    painter.galley(pos2(axis.min.x + 8.0, y - half), g, fg);
    w
}

/// The points of a line that fall into one pixel column.
struct Column {
    col: i64,
    first: Pos2,
    top: (usize, Pos2),
    bottom: (usize, Pos2),
    last: Pos2,
}

impl Column {
    fn new(col: i64, i: usize, p: Pos2) -> Self {
        Self {
            col,
            first: p,
            top: (i, p),
            bottom: (i, p),
            last: p,
        }
    }

    fn add(&mut self, i: usize, p: Pos2) {
        if p.y < self.top.1.y {
            self.top = (i, p);
        }
        if p.y > self.bottom.1.y {
            self.bottom = (i, p);
        }
        self.last = p;
    }

    /// First, highest/lowest in the order they occurred, last.
    fn emit(self, out: &mut Vec<Pos2>) {
        let (a, b) = if self.top.0 <= self.bottom.0 {
            (self.top.1, self.bottom.1)
        } else {
            (self.bottom.1, self.top.1)
        };
        for p in [self.first, a, b, self.last] {
            if out.last() != Some(&p) {
                out.push(p);
            }
        }
    }
}

/// Draws line segments 1 point wide, as steps (Pine's `plot.style_stepline`)
/// when `step` is set.
fn draw_line(painter: &egui::Painter, segments: Vec<Vec<Pos2>>, color: Color32, step: bool) {
    for seg in segments {
        let points = if step {
            let mut out: Vec<Pos2> = Vec::with_capacity(seg.len() * 2);
            for p in seg {
                if let Some(q) = out.last().copied() {
                    if q.y != p.y {
                        out.push(pos2(p.x, q.y));
                    }
                }
                out.push(p);
            }
            out
        } else {
            seg
        };
        painter.add(Shape::line(points, Stroke::new(1.0_f32, color)));
    }
}

/// Bars grouped by pixel column: (column x in physical pixels, first bar, last bar).
type PixelColumns = [(f32, usize, usize)];

/// A line where bars are denser than pixels: per column, a span from the
/// column's lowest to its highest value, joined to the previous column's last
/// value. NaN values are skipped.
fn dense_line(
    cols: &PixelColumns,
    values: &[f64],
    y: impl Fn(f64) -> f32,
    color: Color32,
    ppp: f32,
) -> Shape {
    let mut px = Pixels::new(ppp);
    let width = ppp.round().max(1.0);
    let mut prev: Option<f32> = None;
    for &(x, a, b) in cols {
        let (lo, hi) = values[a..=b]
            .iter()
            .filter(|v| v.is_finite())
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &v| {
                (lo.min(v), hi.max(v))
            });
        if lo > hi {
            prev = None; // nothing to draw in this column (indicator not ready yet)
            continue;
        }
        // y() grows downwards: the highest value is at the top.
        let (mut top, mut bottom) = (y(hi) * ppp, y(lo) * ppp);
        if let Some(p) = prev {
            top = top.min(p);
            bottom = bottom.max(p);
        }
        let top = top.floor();
        px.rect(x, top, x + width, bottom.floor().max(top) + width, color);
        prev = values[b].is_finite().then(|| y(values[b]) * ppp);
    }
    px.into_shape()
}

/// The EMA fill where bars are denser than pixels: per column, between the two
/// EMAs of the column's last bar.
fn dense_fill(c: &Cdc, cols: &PixelColumns, ymap: &YMap, ppp: f32) -> Shape {
    let mut px = Pixels::new(ppp);
    for &(x, _, b) in cols {
        let (f, s) = (c.fast[b], c.slow[b]);
        if f.is_finite() && s.is_finite() {
            let (top, bottom) = (ymap.y(f.max(s)) * ppp, ymap.y(f.min(s)) * ppp);
            px.rect(x, top.round(), x + 1.0, bottom.round(), fill_color(c, b));
        }
    }
    px.into_shape()
}

/// `fillcolor = Bull ? color.new(color.green, 90) : Bear ? color.new(color.red, 90) : …`
fn fill_color(c: &Cdc, i: usize) -> Color32 {
    let base = match c.bull(i) {
        Some(true) => pine::GREEN,
        Some(false) => pine::RED,
        None => pine::BLACK,
    };
    pine::transp(base, 90)
}

/// Fills between two lines from the previous point pair `prev` to (`f`, `s`).
/// Where the lines cross, the fill becomes two triangles meeting at the crossing.
fn fill_step(mesh: &mut Mesh, prev: &mut Option<(Pos2, Pos2)>, f: Pos2, s: Pos2, color: Color32) {
    if let Some((qf, qs)) = prev.replace((f, s)) {
        let (d0, d1) = (qf.y - qs.y, f.y - s.y);
        if d0 * d1 < 0.0 {
            let x = qf + (f - qf) * (d0 / (d0 - d1));
            triangle_mesh(mesh, qf, x, qs, color);
            triangle_mesh(mesh, x, f, s, color);
        } else {
            triangle_mesh(mesh, qf, f, s, color);
            triangle_mesh(mesh, qf, s, qs, color);
        }
    }
}

fn triangle_mesh(mesh: &mut Mesh, a: Pos2, b: Pos2, c: Pos2, color: Color32) {
    let i = mesh.vertices.len() as u32;
    mesh.colored_vertex(a, color);
    mesh.colored_vertex(b, color);
    mesh.colored_vertex(c, color);
    mesh.add_triangle(i, i + 1, i + 2);
}

/// A small triangle with its tip at (`x`, `y`), pointing up (under a bar) or
/// down (over a bar).
fn triangle(painter: &egui::Painter, x: f32, y: f32, size: f32, up: bool, color: Color32) {
    let h = size * 1.6;
    let points = if up {
        vec![pos2(x, y), pos2(x + size, y + h), pos2(x - size, y + h)]
    } else {
        vec![pos2(x, y), pos2(x - size, y - h), pos2(x + size, y - h)]
    };
    painter.add(Shape::convex_polygon(points, color, Stroke::NONE));
}

fn build_edges(candles: &[Candle], kind: ChartKind) -> Vec<f64> {
    let mut edges = Vec::with_capacity(candles.len() + 1);
    edges.push(0.0);
    let mut x = 0.0;
    match kind {
        ChartKind::Equivolume => {
            let (sum, count) = candles
                .iter()
                .filter(|c| c.volume > 0.0)
                .fold((0.0, 0usize), |(s, k), c| (s + c.volume, k + 1));
            let mean = if count > 0 { sum / count as f64 } else { 1.0 };
            for c in candles {
                x += (c.volume / mean).max(MIN_EQUIVOLUME_WIDTH);
                edges.push(x);
            }
        }
        ChartKind::VolumeCandles | ChartKind::Candles => {
            for _ in candles {
                x += 1.0;
                edges.push(x);
            }
        }
    }
    edges
}

/// World x → fractional bar index (extrapolated outside the data).
fn world_to_index(edges: &[f64], w: f64) -> f64 {
    let n = edges.len() - 1;
    if n == 0 {
        return 0.0;
    }
    let unit = edges[n] / n as f64;
    if w <= edges[0] {
        return (w - edges[0]) / unit;
    }
    if w >= edges[n] {
        return n as f64 + (w - edges[n]) / unit;
    }
    let i = edges.partition_point(|&e| e <= w) - 1;
    i as f64 + (w - edges[i]) / (edges[i + 1] - edges[i])
}

fn index_to_world(edges: &[f64], f: f64) -> f64 {
    let n = edges.len() - 1;
    if n == 0 {
        return 0.0;
    }
    let unit = edges[n] / n as f64;
    if f <= 0.0 {
        return edges[0] + f * unit;
    }
    if f >= n as f64 {
        return edges[n] + (f - n as f64) * unit;
    }
    let i = f.floor() as usize;
    edges[i] + (f - i as f64) * (edges[i + 1] - edges[i])
}

/// Largest volume of the last `len` bars, for every bar.
fn rolling_max(c: &[Candle], len: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(c.len());
    let mut window: VecDeque<usize> = VecDeque::new();
    for i in 0..c.len() {
        while window.back().is_some_and(|&j| c[j].volume <= c[i].volume) {
            window.pop_back();
        }
        window.push_back(i);
        if window.front().is_some_and(|&j| j + len <= i) {
            window.pop_front();
        }
        out.push(c[window[0]].volume);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candle(v: f64) -> Candle {
        Candle {
            t: 0,
            open: 1.0,
            high: 1.0,
            low: 1.0,
            close: 1.0,
            volume: v,
        }
    }

    #[test]
    fn equivolume_edges() {
        let c = [candle(1.0), candle(3.0), candle(0.0)];
        let e = build_edges(&c, ChartKind::Equivolume);
        assert_eq!(e, vec![0.0, 0.5, 2.0, 2.0 + MIN_EQUIVOLUME_WIDTH]);
        assert_eq!(
            build_edges(&c, ChartKind::Candles),
            vec![0.0, 1.0, 2.0, 3.0]
        );
    }

    #[test]
    fn index_world_round_trip() {
        let c = [candle(1.0), candle(3.0), candle(2.0)];
        let e = build_edges(&c, ChartKind::Equivolume);
        for f in [-2.0, 0.0, 0.25, 1.5, 2.9, 3.0, 4.5] {
            let w = index_to_world(&e, f);
            assert!((world_to_index(&e, w) - f).abs() < 1e-9, "{f}");
        }
    }

    #[test]
    fn rolling() {
        let c: Vec<Candle> = [5.0, 1.0, 2.0, 7.0, 1.0, 1.0, 1.0]
            .into_iter()
            .map(candle)
            .collect();
        assert_eq!(rolling_max(&c, 3), vec![5.0, 5.0, 5.0, 7.0, 7.0, 7.0, 1.0]);
    }

    #[test]
    fn visible_range_and_hit_test() {
        let mut chart = Chart::new();
        chart.edges = vec![0.0, 1.0, 2.0, 3.0, 4.0];
        chart.scale = 10.0;
        chart.right = 3.5;
        let plot = Rect::from_min_max(pos2(0.0, 0.0), pos2(20.0, 10.0)); // shows world 1.5..3.5
        assert_eq!(chart.visible(&plot), Some((1, 3)));
        assert_eq!(chart.bar_at(chart.world(5.0, &plot)), Some(2));
        assert_eq!(chart.bar_at(4.2), None);
    }
}
