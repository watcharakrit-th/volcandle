//! The application window: toolbar, file panel, chart and status bar. Files load
//! on a background thread, and the open file is reloaded when it changes on disk.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, Align2, Color32, FontId, RichText};
use serde::{Deserialize, Serialize};

use crate::browser::{self, Browser};
use crate::chart::{zone_color, Chart, ChartData, ChartKind, Style, Theme, WidthRef};
use crate::data::datetime::{self, DAY};
use crate::data::{self, resample, DateOrder, LoadOptions, Series, Timeframe};
use crate::demo;
use crate::indicators::{cdc, CdcSettings, Ribbon, RsiSettings, Source as PriceSource, Zone};

const SETTINGS_KEY: &str = "volcandle";

/// Display shifts offered in Settings, in minutes.
const TZ_CHOICES: [i32; 31] = [
    -720, -660, -600, -540, -480, -420, -360, -300, -240, -210, -180, -120, -60, 0, 60, 120, 180,
    210, 240, 270, 300, 330, 345, 360, 390, 420, 480, 540, 570, 600, 660,
];

pub fn run(file: Option<PathBuf>, date_order: Option<DateOrder>) -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("volcandle")
            .with_app_id("volcandle")
            .with_inner_size([1440.0, 880.0])
            .with_min_inner_size([760.0, 480.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "volcandle",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, file, date_order)))),
    )
}

/// Preferences, persisted between runs.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub style: Style,
    pub show_files: bool,
    pub auto_reload: bool,
    pub date_order: Option<DateOrder>,
    pub last_file: Option<PathBuf>,
    pub browse_dir: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            style: Style::default(),
            show_files: true,
            auto_reload: true,
            date_order: None,
            last_file: None,
            browse_dir: None,
        }
    }
}

struct Source {
    series: Series,
    path: Option<PathBuf>,
    modified: Option<SystemTime>,
    load_time: Duration,
}

struct Loading {
    rx: Receiver<Result<Series, String>>,
    path: PathBuf,
    keep_view: bool,
    started: Instant,
}

pub struct App {
    ctx: egui::Context,
    settings: Settings,
    chart: Chart,
    source: Option<Source>,
    view: Option<ChartData>,
    timeframe: Timeframe,
    generation: u64,
    loading: Option<Loading>,
    browser: Browser,
    error: Option<String>,
    last_poll: Instant,
    visuals_dark: Option<bool>,
    title: String,
}

fn file_name(p: &Path) -> String {
    p.file_name().map_or_else(
        || p.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn tz_label(min: i32) -> String {
    if min == 0 {
        return "No shift (times as in the file)".into();
    }
    let sign = if min > 0 { '+' } else { '−' };
    format!("{sign}{}:{:02} h", min.abs() / 60, min.abs() % 60)
}

fn order_label(order: Option<DateOrder>) -> &'static str {
    match order {
        None => "Detect automatically",
        Some(DateOrder::Dmy) => "Day/Month/Year (31/12/2024)",
        Some(DateOrder::Mdy) => "Month/Day/Year (12/31/2024)",
    }
}

fn source_combo(ui: &mut egui::Ui, id: &str, source: &mut PriceSource) {
    egui::ComboBox::from_id_salt(id)
        .width(90.0)
        .selected_text(source.label())
        .show_ui(ui, |ui| {
            for s in PriceSource::ALL {
                ui.selectable_value(source, s, s.label());
            }
        });
}

/// CDC ActionZone's inputs, named as in the original script.
fn cdc_settings(ui: &mut egui::Ui, s: &mut CdcSettings, interval: i64) {
    egui::Grid::new("cdc_inputs")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            ui.label("Source data");
            source_combo(ui, "cdc_source", &mut s.source);
            ui.end_row();
            ui.label("Fast EMA period");
            ui.add(egui::DragValue::new(&mut s.fast).range(1..=1000));
            ui.end_row();
            ui.label("Slow EMA period");
            ui.add(egui::DragValue::new(&mut s.slow).range(1..=1000));
            ui.end_row();
            ui.label("Smoothing period")
                .on_hover_text("1 = no smoothing");
            ui.add(egui::DragValue::new(&mut s.smoothing).range(1..=100));
            ui.end_row();
            ui.label("Fixed timeframe").on_hover_text(
                "Advanced: show a higher timeframe's EMAs (as of its last finished bar) on this chart",
            );
            let text = match s.fixed_tf {
                None => "Off".to_string(),
                Some(tf) if cdc::fixed_tf_usable(tf, interval) => tf.label(),
                Some(tf) => format!("{} (not above this chart)", tf.label()),
            };
            egui::ComboBox::from_id_salt("cdc_fixed_tf")
                .width(170.0)
                .selected_text(text)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut s.fixed_tf, None, "Off");
                    for tf in resample::choices(interval) {
                        ui.selectable_value(&mut s.fixed_tf, Some(tf), tf.label());
                    }
                });
            ui.end_row();
        });
    ui.checkbox(&mut s.paint_bars, "Paint bar colors");
    ui.horizontal(|ui| {
        ui.checkbox(&mut s.show_fast, "Fast EMA line");
        ui.checkbox(&mut s.show_slow, "Slow EMA line");
    });
    ui.checkbox(&mut s.signals, "Buy/Sell signals")
        .on_hover_text(
            "Green dot: the first green bar after a bearish trend.\n\
         Red dot: the first red bar after a bullish trend.",
        );
    ui.checkbox(
        &mut s.assistive_text,
        "Assistive text (label at the last bar)",
    );
    ui.horizontal(|ui| {
        ui.label("Buy/Sell ribbon");
        ui.selectable_value(&mut s.ribbon, Ribbon::Off, "Off");
        ui.selectable_value(&mut s.ribbon, Ribbon::Top, "Top");
        ui.selectable_value(&mut s.ribbon, Ribbon::Bottom, "Bottom");
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut s.momentum, "Momentum signals (StochRSI)")
            .on_hover_text(
                "Off by default in the original script (\"only use with caution\").\n\
                 Teal/orange triangles where StochRSI crosses in the trend's direction.\n\
                 Strict: only crosses from oversold/overbought.",
            );
        ui.add_enabled_ui(s.momentum, |ui| {
            ui.selectable_value(&mut s.momentum_threshold, 1, "strict");
            ui.selectable_value(&mut s.momentum_threshold, 0, "all");
        });
    });
    ui.add_space(2.0);
    egui::Grid::new("cdc_zones")
        .num_columns(3)
        .spacing([14.0, 2.0])
        .show(ui, |ui| {
            for (k, z) in Zone::ALL.into_iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(RichText::new(z.name()).color(zone_color(z)).strong());
                    ui.weak(z.meaning());
                });
                if k % 3 == 2 {
                    ui.end_row();
                }
            }
        });
    if ui.button("Defaults").clicked() {
        *s = CdcSettings {
            enabled: s.enabled,
            ..CdcSettings::default()
        };
    }
}

fn rsi_settings(ui: &mut egui::Ui, s: &mut RsiSettings) {
    egui::Grid::new("rsi_inputs")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            ui.label("Length");
            ui.add(egui::DragValue::new(&mut s.length).range(1..=500));
            ui.end_row();
            ui.label("Source");
            source_combo(ui, "rsi_source", &mut s.source);
            ui.end_row();
            ui.label("Overbought");
            ui.add(egui::DragValue::new(&mut s.overbought).range(50.0..=100.0));
            ui.end_row();
            ui.label("Oversold");
            ui.add(egui::DragValue::new(&mut s.oversold).range(0.0..=50.0));
            ui.end_row();
        });
    ui.horizontal(|ui| {
        ui.checkbox(&mut s.show_ma, "RSI-based MA (SMA)");
        ui.add_enabled(
            s.show_ma,
            egui::DragValue::new(&mut s.ma_length).range(1..=500),
        );
    });
    if ui.button("Defaults").clicked() {
        *s = RsiSettings {
            enabled: s.enabled,
            ..RsiSettings::default()
        };
    }
}

impl App {
    fn new(
        cc: &eframe::CreationContext<'_>,
        file: Option<PathBuf>,
        date_order: Option<DateOrder>,
    ) -> Self {
        let mut settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, SETTINGS_KEY))
            .unwrap_or_default();
        if date_order.is_some() {
            settings.date_order = date_order;
        }
        crate::fonts::install(&cc.egui_ctx);
        cc.egui_ctx.style_mut(|s| {
            s.spacing.item_spacing = egui::vec2(6.0, 4.0);
            s.spacing.button_padding = egui::vec2(8.0, 3.0);
        });
        let file = file.map(|f| std::path::absolute(&f).unwrap_or(f));
        let cwd = std::env::current_dir().unwrap_or_default();
        let start_dir = file
            .as_ref()
            .and_then(|f| f.parent().map(Path::to_path_buf))
            .or_else(|| settings.browse_dir.clone())
            .filter(|d| d.is_dir())
            .or_else(|| Some(cwd.join("data")).filter(|d| d.is_dir()))
            .unwrap_or(cwd);
        let to_open = file.or_else(|| settings.last_file.clone().filter(|p| p.is_file()));
        let mut app = Self {
            ctx: cc.egui_ctx.clone(),
            settings,
            chart: Chart::new(),
            source: None,
            view: None,
            timeframe: Timeframe::Native,
            generation: 0,
            loading: None,
            browser: Browser::new(start_dir),
            error: None,
            last_poll: Instant::now(),
            visuals_dark: None,
            title: String::new(),
        };
        app.show_demo();
        if let Some(path) = to_open {
            app.open(path);
        }
        app
    }

    fn open(&mut self, path: PathBuf) {
        self.start_load(path, false);
    }

    fn start_load(&mut self, path: PathBuf, keep_view: bool) {
        let (tx, rx) = mpsc::channel();
        let ctx = self.ctx.clone();
        let opts = LoadOptions {
            date_order: self.settings.date_order,
        };
        let p = path.clone();
        std::thread::spawn(move || {
            let _ = tx.send(data::load_file(&p, &opts));
            ctx.request_repaint();
        });
        self.loading = Some(Loading {
            rx,
            path,
            keep_view,
            started: Instant::now(),
        });
    }

    fn show_demo(&mut self) {
        self.loading = None;
        self.settings.last_file = None;
        let source = Source {
            series: demo::series(),
            path: None,
            modified: None,
            load_time: Duration::ZERO,
        };
        self.set_source(source, false);
    }

    fn poll_loading(&mut self) {
        let Some(loading) = &self.loading else { return };
        let result = match loading.rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                Err("the loader stopped unexpectedly (please report this file)".into())
            }
        };
        let Some(loading) = self.loading.take() else {
            return;
        };
        match result {
            Ok(series) => {
                let modified = std::fs::metadata(&loading.path)
                    .and_then(|m| m.modified())
                    .ok();
                self.error = None;
                self.settings.last_file = Some(loading.path.clone());
                let source = Source {
                    series,
                    path: Some(loading.path),
                    modified,
                    load_time: loading.started.elapsed(),
                };
                self.set_source(source, loading.keep_view);
            }
            Err(e) => {
                self.error = Some(format!("Could not open {}: {e}", file_name(&loading.path)))
            }
        }
    }

    /// Reloads the open file when its modification time changes.
    fn poll_file_change(&mut self) {
        if !self.settings.auto_reload
            || self.loading.is_some()
            || self.last_poll.elapsed() < Duration::from_secs(1)
        {
            return;
        }
        self.last_poll = Instant::now();
        let Some(src) = &mut self.source else { return };
        let Some(path) = src.path.clone() else { return };
        let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if modified.is_some() && modified != src.modified {
            src.modified = modified; // also on failure, so a broken file is not retried in a loop
            self.start_load(path, true);
        }
    }

    fn set_source(&mut self, source: Source, keep_view: bool) {
        if !keep_view {
            self.timeframe = Timeframe::Native;
        }
        self.source = Some(source);
        self.rebuild_view(keep_view);
    }

    /// Resamples the source to the current timeframe and hands it to the chart.
    fn rebuild_view(&mut self, keep_view: bool) {
        let Some(src) = &self.source else { return };
        let s = &src.series;
        if self.timeframe != Timeframe::Native
            && !resample::choices(s.interval).contains(&self.timeframe)
        {
            self.timeframe = Timeframe::Native;
        }
        let native = self.timeframe == Timeframe::Native;
        let candles = resample::resample(&s.candles, self.timeframe);
        let interval = if native {
            s.interval
        } else {
            self.timeframe.seconds()
        };
        let intraday = interval < DAY && candles.iter().any(|c| c.t.rem_euclid(DAY) != 0);
        self.generation += 1;
        let view = ChartData {
            candles,
            name: s.name.clone(),
            timeframe: if native {
                resample::interval_label(s.interval)
            } else {
                self.timeframe.label()
            },
            interval,
            decimals: s.decimals,
            has_volume: s.has_volume,
            intraday,
            generation: self.generation,
        };
        match &self.view {
            Some(old) if keep_view => self.chart.keep_view(old),
            _ => self.chart.reset_view(),
        }
        self.view = Some(view);
    }

    fn update_title(&mut self) {
        let title = match &self.source {
            Some(Source { path: Some(p), .. }) => format!("{} — volcandle", file_name(p)),
            _ => "volcandle".to_string(),
        };
        if title != self.title {
            self.ctx
                .send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    fn apply_visuals(&mut self, ctx: &egui::Context) {
        let dark = self.settings.style.dark;
        if self.visuals_dark == Some(dark) {
            return;
        }
        self.visuals_dark = Some(dark);
        let t = Theme::new(dark);
        let mut v = if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        v.panel_fill = t.panel;
        v.window_fill = t.panel;
        v.extreme_bg_color = t.bg;
        v.selection.bg_fill = t.accent;
        v.selection.stroke.color = Color32::WHITE;
        v.hyperlink_color = t.accent;
        ctx.set_visuals(v);
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.iter().find_map(|f| f.path.clone()));
        if let Some(path) = dropped {
            if let Some(dir) = path.parent() {
                self.browser.navigate(dir.to_path_buf());
            }
            self.open(path);
        }
    }

    fn drop_overlay(&self, ctx: &egui::Context) {
        if ctx.input(|i| i.raw.hovered_files.is_empty()) {
            return;
        }
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("drop"),
        ));
        let rect = ctx.screen_rect();
        painter.rect_filled(rect, 0.0, Color32::from_black_alpha(170));
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "Drop a price file to chart it",
            FontId::proportional(26.0),
            Color32::WHITE,
        );
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.toggle_value(&mut self.settings.show_files, "☰ Files")
                .on_hover_text("Show or hide the file list");
            ui.separator();
            if let Some(src) = &self.source {
                ui.label(RichText::new(&src.series.name).strong().size(15.0));
                ui.separator();
            }

            let interval = self.source.as_ref().map_or(0, |s| s.series.interval);
            let mut tf = self.timeframe;
            ui.selectable_value(
                &mut tf,
                Timeframe::Native,
                resample::interval_label(interval),
            )
            .on_hover_text("Bars exactly as in the file");
            for choice in resample::choices(interval) {
                ui.selectable_value(&mut tf, choice, choice.label())
                    .on_hover_text(format!("Combine the bars into {} bars", choice.label()));
            }
            if tf != self.timeframe {
                self.timeframe = tf;
                self.rebuild_view(true);
            }
            ui.separator();

            for kind in ChartKind::ALL {
                ui.selectable_value(&mut self.settings.style.kind, kind, kind.label())
                    .on_hover_text(kind.help());
            }
            ui.separator();
            ui.toggle_value(&mut self.settings.style.show_volume, "Volume")
                .on_hover_text("Volume bars along the bottom (V)");
            ui.toggle_value(&mut self.settings.style.log_scale, "Log")
                .on_hover_text("Logarithmic price scale (L)");
            ui.separator();
            ui.toggle_value(&mut self.settings.style.cdc.enabled, "CDC")
                .on_hover_text(
                    "CDC ActionZone V3 2020 by piriya33: EMA 12/26 zones as bar colors, \
                     buy/sell signals (settings under Indicators)",
                );
            ui.toggle_value(&mut self.settings.style.rsi.enabled, "RSI")
                .on_hover_text(
                    "Relative Strength Index in a pane under the chart (settings under Indicators)",
                );
            self.indicators_menu(ui);
            ui.separator();
            self.settings_menu(ui);
            if ui
                .button("⟲ Reset view")
                .on_hover_text("Latest bars at the default zoom (R, or double-click the chart)")
                .clicked()
            {
                self.chart.reset_view();
            }
        });
    }

    fn indicators_menu(&mut self, ui: &mut egui::Ui) {
        let interval = self.view.as_ref().map_or(0, |v| v.interval);
        ui.menu_button("Indicators", |ui| {
            ui.set_min_width(380.0);
            let st = &mut self.settings.style;
            ui.horizontal(|ui| {
                ui.checkbox(
                    &mut st.cdc.enabled,
                    RichText::new("CDC ActionZone V3 2020").strong(),
                );
                ui.weak("by piriya33");
            });
            ui.add_enabled_ui(st.cdc.enabled, |ui| cdc_settings(ui, &mut st.cdc, interval));
            ui.separator();
            ui.checkbox(
                &mut st.rsi.enabled,
                RichText::new("RSI (Relative Strength Index)").strong(),
            );
            ui.add_enabled_ui(st.rsi.enabled, |ui| rsi_settings(ui, &mut st.rsi));
        });
    }

    fn settings_menu(&mut self, ui: &mut egui::Ui) {
        let order_before = self.settings.date_order;
        ui.menu_button("⚙ Settings", |ui| {
            ui.set_min_width(330.0);
            let st = &mut self.settings.style;
            ui.label(RichText::new("Theme").strong());
            ui.horizontal(|ui| {
                ui.selectable_value(&mut st.dark, true, "🌙 Dark");
                ui.selectable_value(&mut st.dark, false, "☀ Light");
            });
            ui.checkbox(&mut st.prev_close_colors, "Color candles by previous close")
                .on_hover_text("Green when the close is above the previous close, instead of above the open");
            ui.separator();

            ui.label(RichText::new("Volume candle width").strong());
            ui.radio_value(&mut st.width_ref, WidthRef::Visible, "Widest = largest volume on screen");
            ui.horizontal(|ui| {
                ui.radio_value(&mut st.width_ref, WidthRef::Lookback, "Widest = largest of the last");
                ui.add(egui::DragValue::new(&mut st.lookback).range(2..=5000));
                ui.label("bars");
            });
            ui.weak("“On screen” rescales while you scroll; “last N bars” stays fixed.");
            ui.separator();

            ui.label(RichText::new("Time").strong());
            egui::ComboBox::from_id_salt("tz")
                .width(220.0)
                .selected_text(tz_label(st.tz_offset_min))
                .show_ui(ui, |ui| {
                    for m in TZ_CHOICES {
                        ui.selectable_value(&mut st.tz_offset_min, m, tz_label(m));
                    }
                });
            ui.weak("Shifts the times shown. Crypto exchanges export UTC;\nchoose +7:00 to see Thailand time.");
            ui.separator();

            ui.label(RichText::new("Reading files").strong());
            egui::ComboBox::from_id_salt("date_order")
                .width(220.0)
                .selected_text(order_label(self.settings.date_order))
                .show_ui(ui, |ui| {
                    for order in [None, Some(DateOrder::Dmy), Some(DateOrder::Mdy)] {
                        ui.selectable_value(&mut self.settings.date_order, order, order_label(order));
                    }
                });
            ui.weak("For dates like 03/04/2024 when detection guesses wrong.");
            ui.checkbox(&mut self.settings.auto_reload, "Reload the file when it changes on disk");
        });
        if self.settings.date_order != order_before {
            if let Some(path) = self.source.as_ref().and_then(|s| s.path.clone()) {
                self.start_load(path, true);
            }
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let theme = Theme::new(self.settings.style.dark);
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            if let Some(l) = &self.loading {
                ui.add(egui::Spinner::new().size(12.0));
                ui.label(format!("Loading {}…", file_name(&l.path)));
                ui.separator();
            }
            if let Some(err) = &self.error {
                ui.colored_label(theme.down, format!("✖ {err}"));
                if ui.small_button("Dismiss").clicked() {
                    self.error = None;
                }
                return;
            }
            let (Some(src), Some(view)) = (&self.source, &self.view) else {
                return;
            };
            let s = &src.series;
            let mut bars = format!("{} bars", thousands(s.candles.len()));
            if view.candles.len() != s.candles.len() {
                bars += &format!(" → {} × {}", thousands(view.candles.len()), view.timeframe);
            }
            ui.label(bars);
            if let (Some(a), Some(b)) = (s.candles.first(), s.candles.last()) {
                ui.label(format!(
                    "{} → {}",
                    datetime::format_datetime(a.t),
                    datetime::format_datetime(b.t)
                ));
            }
            ui.separator();
            ui.label(&s.format);
            if !s.columns.is_empty() {
                ui.separator();
                ui.weak(&s.columns);
            }
            if src.path.is_some() {
                ui.separator();
                ui.weak(format!("read in {} ms", src.load_time.as_millis()));
            }
        });
        if let Some(src) = &self.source {
            if !src.series.notes.is_empty() && self.error.is_none() {
                ui.colored_label(theme.warn, format!("⚠ {}", src.series.notes.join(" · ")));
            }
        }
        ui.add_space(1.0);
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.apply_visuals(ctx);
        self.poll_loading();
        self.poll_file_change();
        self.handle_drops(ctx);
        self.update_title();
        if self.settings.auto_reload && self.source.as_ref().is_some_and(|s| s.path.is_some()) {
            ctx.request_repaint_after(Duration::from_secs(1));
        }

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.add_space(3.0);
            self.toolbar(ui);
            ui.add_space(1.0);
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| self.status_bar(ui));
        if self.settings.show_files {
            let current = self.source.as_ref().and_then(|s| s.path.clone());
            let action = egui::SidePanel::left("files")
                .resizable(true)
                .default_width(250.0)
                .min_width(170.0)
                .show(ctx, |ui| self.browser.ui(ui, current.as_deref()))
                .inner;
            match action {
                Some(browser::Action::Open(path)) => self.open(path),
                Some(browser::Action::Demo) => self.show_demo(),
                None => {}
            }
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                if let Some(view) = &self.view {
                    self.chart.ui(ui, view, &mut self.settings.style);
                }
            });
        self.drop_overlay(ctx);
        self.settings.browse_dir = Some(self.browser.dir().to_path_buf());
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, SETTINGS_KEY, &self.settings);
    }
}
