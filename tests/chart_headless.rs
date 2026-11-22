//! Renders the chart without a window (egui only) to exercise every drawing path:
//! all chart types, themes, scales, zoom extremes and awkward data.

use std::time::{Duration, Instant};

use eframe::egui::{self, epaint::Primitive, pos2, vec2, Event, Key, Modifiers};
use volcandle::chart::{Chart, ChartData, ChartKind, Style};
use volcandle::data::{Candle, Series};

struct Harness {
    ctx: egui::Context,
    chart: Chart,
    style: Style,
    data: ChartData,
}

impl Harness {
    fn new(candles: Vec<Candle>) -> Self {
        let has_volume = candles.iter().any(|c| c.volume > 0.0);
        let data = ChartData {
            candles,
            name: "TEST".into(),
            timeframe: "1h".into(),
            interval: 3600,
            decimals: 2,
            has_volume,
            intraday: true,
            generation: 1,
        };
        Self {
            ctx: egui::Context::default(),
            chart: Chart::new(),
            style: Style::default(),
            data,
        }
    }

    fn from_series(s: Series) -> Self {
        Self::new(s.candles)
    }

    /// Runs one frame; returns the number of vertices drawn and the CPU time.
    fn frame(&mut self, events: Vec<Event>) -> (usize, Duration) {
        let mut all = vec![Event::PointerMoved(pos2(640.0, 300.0))];
        all.extend(events);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                vec2(1400.0, 800.0),
            )),
            events: all,
            ..Default::default()
        };
        let started = Instant::now();
        let (chart, data, style) = (&mut self.chart, &self.data, &mut self.style);
        let out = self.ctx.run(input, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| chart.ui(ui, data, style));
        });
        let prims = self.ctx.tessellate(out.shapes, out.pixels_per_point);
        let vertices = prims
            .iter()
            .map(|p| match &p.primitive {
                Primitive::Mesh(m) => m.vertices.len(),
                Primitive::Callback(_) => 0,
            })
            .sum();
        (vertices, started.elapsed())
    }

    fn keys(&mut self, key: Key, times: usize) -> (usize, Duration) {
        let press = |pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        let events = (0..times)
            .flat_map(|_| [press(true), press(false)])
            .collect();
        self.frame(events)
    }
}

fn walk(n: usize) -> Vec<Candle> {
    let mut price = 100.0;
    (0..n)
        .map(|i| {
            let x = i as f64;
            let open = price;
            let close = open * (1.0 + 0.01 * (x * 0.7).sin() * (x * 0.013).cos());
            price = close;
            Candle {
                t: 1_704_067_200 + i as i64 * 3600,
                open,
                high: open.max(close) * 1.002,
                low: open.min(close) * 0.998,
                close,
                volume: 10.0 + 50.0 * (x * 0.37).sin().abs(),
            }
        })
        .collect()
}

#[test]
fn every_mode_theme_and_scale_draws() {
    let mut h = Harness::from_series(volcandle::demo::series());
    for kind in ChartKind::ALL {
        for dark in [true, false] {
            for log in [false, true] {
                h.style.kind = kind;
                h.style.dark = dark;
                h.style.log_scale = log;
                h.frame(vec![]);
                let (vertices, _) = h.frame(vec![]);
                assert!(
                    vertices > 1000,
                    "{kind:?} dark={dark} log={log}: only {vertices} vertices"
                );
            }
        }
    }
}

#[test]
fn indicators_draw_in_every_mode() {
    use volcandle::data::Timeframe;
    use volcandle::indicators::Ribbon;
    let mut h = Harness::from_series(volcandle::demo::series());
    h.style.cdc.enabled = true;
    h.style.cdc.momentum = true;
    h.style.cdc.momentum_threshold = 0;
    h.style.rsi.enabled = true;
    // The demo is hourly: 1D is a usable fixed timeframe, 1m is not (ignored).
    for (ribbon, fixed) in [
        (Ribbon::Top, None),
        (Ribbon::Bottom, Some(Timeframe::Day)),
        (Ribbon::Off, Some(Timeframe::Minutes(1))),
    ] {
        h.style.cdc.ribbon = ribbon;
        h.style.cdc.fixed_tf = fixed;
        for kind in ChartKind::ALL {
            for dark in [true, false] {
                for log in [false, true] {
                    h.style.kind = kind;
                    h.style.dark = dark;
                    h.style.log_scale = log;
                    h.frame(vec![]);
                    let (vertices, _) = h.frame(vec![]);
                    assert!(vertices > 1000, "{kind:?} {ribbon:?} {fixed:?}");
                }
            }
        }
    }
    assert!(h.keys(Key::ArrowUp, 40).0 > 0, "max zoom in");
    assert!(h.keys(Key::ArrowDown, 80).0 > 0, "max zoom out");
    assert!(
        h.keys(Key::Home, 1).0 > 0,
        "start (indicators still warming up)"
    );
    // Turning them off again leaves a plain chart.
    h.style.cdc.enabled = false;
    h.style.rsi.enabled = false;
    assert!(h.keys(Key::End, 1).0 > 0);
}

#[test]
fn zoom_and_pan_extremes() {
    let mut h = Harness::new(walk(5_000));
    h.frame(vec![]);
    for kind in ChartKind::ALL {
        h.style.kind = kind;
        assert!(h.keys(Key::ArrowUp, 40).0 > 0, "max zoom in");
        assert!(h.keys(Key::ArrowDown, 80).0 > 0, "max zoom out");
        assert!(h.keys(Key::Home, 1).0 > 0, "start");
        assert!(h.keys(Key::ArrowLeft, 50).0 > 0, "past the start");
        assert!(h.keys(Key::End, 1).0 > 0, "end");
        assert!(h.keys(Key::ArrowRight, 5_000).0 > 0, "past the end");
        assert!(h.keys(Key::R, 1).0 > 0, "reset");
    }
}

#[test]
fn awkward_data() {
    let one = vec![Candle {
        t: 0,
        open: 1.0,
        high: 2.0,
        low: 0.5,
        close: 1.5,
        volume: 3.0,
    }];
    let flat: Vec<Candle> = (0..50)
        .map(|i| Candle {
            t: i * 60,
            open: 5.0,
            high: 5.0,
            low: 5.0,
            close: 5.0,
            volume: 0.0,
        })
        .collect();
    let negative: Vec<Candle> = (0..50)
        .map(|i| {
            let p = -40.0 + i as f64; // WTI oil went negative in April 2020
            Candle {
                t: i * 86_400,
                open: p,
                high: p + 2.0,
                low: p - 2.0,
                close: p + 1.0,
                volume: 100.0,
            }
        })
        .collect();
    let huge: Vec<Candle> = (0..50)
        .map(|i| {
            let p = 1e12 * (1.0 + i as f64 / 10.0);
            Candle {
                t: i * 3600,
                open: p,
                high: p * 1.1,
                low: p * 0.9,
                close: p,
                volume: 1e15,
            }
        })
        .collect();
    for candles in [one, flat, negative, huge] {
        let mut h = Harness::new(candles);
        h.style.cdc.enabled = true;
        h.style.cdc.momentum = true;
        h.style.rsi.enabled = true;
        for kind in ChartKind::ALL {
            for log in [false, true] {
                h.style.kind = kind;
                h.style.log_scale = log;
                h.frame(vec![]);
                h.keys(Key::ArrowDown, 30);
                assert!(h.keys(Key::ArrowUp, 60).0 > 0);
            }
        }
    }
}

#[test]
fn half_a_million_bars_zoomed_out() {
    let mut h = Harness::new(walk(525_600));
    h.frame(vec![]);
    h.keys(Key::ArrowDown, 120); // all the way out: ~400 bars per pixel column
    let mut worst = Duration::ZERO;
    for kind in ChartKind::ALL {
        h.style.kind = kind;
        h.frame(vec![]);
        let runs: Vec<(usize, Duration)> = (0..5).map(|_| h.frame(vec![])).collect();
        let (vertices, took) = runs.iter().copied().min_by_key(|r| r.1).unwrap();
        assert!(vertices > 1000);
        println!("{kind:?}: best of 5 = {took:?} ({vertices} vertices)");
        worst = worst.max(took);
    }
    println!("525,600 bars, fully zoomed out: slowest frame {worst:?}");

    h.style.cdc.enabled = true;
    h.style.rsi.enabled = true;
    let started = Instant::now();
    h.frame(vec![]); // computes both indicators once
    println!("indicators computed in {:?}", started.elapsed());
    for kind in ChartKind::ALL {
        h.style.kind = kind;
        h.frame(vec![]);
        let runs: Vec<(usize, Duration)> = (0..5).map(|_| h.frame(vec![])).collect();
        let (vertices, took) = runs.iter().copied().min_by_key(|r| r.1).unwrap();
        println!("{kind:?} + CDC + RSI: best of 5 = {took:?} ({vertices} vertices)");
    }
}
