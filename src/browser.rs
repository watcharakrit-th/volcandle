//! Side-panel file browser. Native file dialogs need a desktop portal, which the
//! dev container does not have, so data files are listed inside the window.
//! The folder is rescanned every few seconds, so new downloads show up.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};

/// Extensions shown by default.
pub const EXTENSIONS: &[&str] = &[
    "csv", "tsv", "txt", "prn", "dat", "json", "jsonl", "ndjson", "xlsx", "xlsm", "xlsb", "xls",
    "ods",
];

pub enum Action {
    Open(PathBuf),
    Demo,
}

struct Entry {
    name: String,
    path: PathBuf,
    is_dir: bool,
    size: u64,
}

pub struct Browser {
    dir: PathBuf,
    dir_text: String,
    filter: String,
    show_all: bool,
    entries: Vec<Entry>,
    error: Option<String>,
    scanned: Instant,
}

impl Browser {
    pub fn new(dir: PathBuf) -> Self {
        let mut b = Self {
            dir: PathBuf::new(),
            dir_text: String::new(),
            filter: String::new(),
            show_all: false,
            entries: Vec::new(),
            error: None,
            scanned: Instant::now(),
        };
        b.navigate(dir);
        b
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn navigate(&mut self, dir: PathBuf) {
        match std::fs::read_dir(&dir) {
            Ok(_) => {
                self.dir = dir;
                self.error = None;
                self.rescan();
            }
            Err(e) => self.error = Some(format!("{}: {e}", dir.display())),
        }
        self.dir_text = self.dir.display().to_string();
    }

    fn matches(&self, path: &Path) -> bool {
        self.show_all
            || path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
    }

    fn rescan(&mut self) {
        self.scanned = Instant::now();
        let Ok(read) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let mut entries: Vec<Entry> = read
            .filter_map(Result::ok)
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let path = e.path();
                let meta = std::fs::metadata(&path).ok()?;
                let keep = !name.starts_with('.') && (meta.is_dir() || self.matches(&path));
                keep.then_some(Entry {
                    name,
                    path,
                    is_dir: meta.is_dir(),
                    size: meta.len(),
                })
            })
            .collect();
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        self.entries = entries;
    }

    fn places() -> Vec<(&'static str, PathBuf)> {
        let mut places = Vec::new();
        if let Ok(cwd) = std::env::current_dir() {
            for sub in ["data", "samples"] {
                if cwd.join(sub).is_dir() {
                    places.push((
                        if sub == "data" {
                            "📂 data"
                        } else {
                            "📂 samples"
                        },
                        cwd.join(sub),
                    ));
                }
            }
            places.push(("📁 project", cwd));
        }
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            places.push(("🏠 home", PathBuf::from(home)));
        }
        places
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, current: Option<&Path>) -> Option<Action> {
        if self.scanned.elapsed() > Duration::from_secs(3) {
            self.rescan();
        }
        let mut action = None;
        let mut go: Option<PathBuf> = None;
        ui.horizontal(|ui| {
            ui.heading("Files");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("⟳").on_hover_text("Refresh").clicked() {
                    go = Some(self.dir.clone());
                }
                if ui
                    .small_button("⬆")
                    .on_hover_text("Parent folder")
                    .clicked()
                {
                    go = self.dir.parent().map(Path::to_path_buf);
                }
            });
        });
        let r = ui.add(
            egui::TextEdit::singleline(&mut self.dir_text)
                .desired_width(f32::INFINITY)
                .hint_text("Folder"),
        );
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            let typed = PathBuf::from(self.dir_text.trim());
            if typed.is_file() {
                action = Some(Action::Open(typed));
            } else {
                go = Some(typed);
            }
        }
        ui.horizontal_wrapped(|ui| {
            for (label, path) in Self::places() {
                if ui
                    .small_button(label)
                    .on_hover_text(path.display().to_string())
                    .clicked()
                {
                    go = Some(path);
                }
            }
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.filter)
                .desired_width(f32::INFINITY)
                .hint_text("🔍 Filter, e.g. btc"),
        );
        if let Some(err) = &self.error {
            ui.colored_label(egui::Color32::from_rgb(240, 100, 100), err);
        }
        ui.separator();

        let filter = self.filter.to_lowercase();
        let bottom_h = 64.0;
        egui::ScrollArea::vertical()
            .max_height((ui.available_height() - bottom_h).max(80.0))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut shown = 0;
                for e in self
                    .entries
                    .iter()
                    .filter(|e| filter.is_empty() || e.name.to_lowercase().contains(&filter))
                {
                    shown += 1;
                    let is_current = current.is_some_and(|c| c == e.path);
                    let text = if e.is_dir {
                        RichText::new(format!("📁 {}", e.name))
                    } else {
                        RichText::new(format!("📈 {}", e.name)).strong()
                    };
                    let resp = ui.selectable_label(is_current, text);
                    let resp = if e.is_dir {
                        resp
                    } else {
                        resp.on_hover_text(format!("{}  ·  {}", e.name, human_size(e.size)))
                    };
                    if resp.clicked() {
                        if e.is_dir {
                            go = Some(e.path.clone());
                        } else {
                            action = Some(Action::Open(e.path.clone()));
                        }
                    }
                }
                if shown == 0 {
                    ui.weak("No data files here.\nSupported: CSV, TSV, TXT, JSON, XLSX, XLS, ODS.");
                }
            });
        ui.separator();
        if ui.checkbox(&mut self.show_all, "Show all files").changed() {
            self.rescan();
        }
        if ui
            .button("✨ Demo data")
            .on_hover_text("Synthetic prices, to try the chart")
            .clicked()
        {
            action = Some(Action::Demo);
        }
        if let Some(dir) = go {
            self.navigate(dir);
        }
        action
    }
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = bytes as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit + 1 < UNITS.len() {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}
