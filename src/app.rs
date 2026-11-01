use eframe::egui::{
    self, Align, Button, Color32, CornerRadius, FontFamily, Frame, Layout, Margin, RichText,
    Sense, Stroke, Ui, vec2,
};
use std::sync::{Arc, Mutex};
use tray_icon::{
    MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem},
};

use crate::{
    config, dots, update,
    engine::{self, Card, Handle, Progress, Shared, State},
};

const BG: Color32 = Color32::from_rgb(0, 0, 0);
const PANEL: Color32 = Color32::from_rgb(14, 14, 14);
const LINE: Color32 = Color32::from_rgb(38, 38, 38);
const DIM: Color32 = Color32::from_rgb(34, 34, 34);
const MUTED: Color32 = Color32::from_rgb(130, 130, 130);
const FG: Color32 = Color32::from_rgb(237, 237, 237);
const RED: Color32 = Color32::from_rgb(215, 25, 33);

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Home,
    Settings,
}

pub struct App {
    h: Handle,
    tab: Tab,
    _tray: TrayIcon,
    hide_first: bool,
}

pub fn icon_rgba(size: u32) -> Vec<u8> {
    let mut px = vec![0u8; (size * size * 4) as usize];
    let cell = size as f32 / 5.0;
    for y in 0..size {
        for x in 0..size {
            let (gx, gy) = (x as f32 / cell, y as f32 / cell);
            let (cx, cy) = (gx.floor() + 0.5, gy.floor() + 0.5);
            let d = ((gx - cx).powi(2) + (gy - cy).powi(2)).sqrt();
            if d < 0.3 {
                let center = gx.floor() == 2.0 && gy.floor() == 2.0;
                let c = if center { [215, 25, 33] } else { [237, 237, 237] };
                let i = ((y * size + x) * 4) as usize;
                px[i..i + 3].copy_from_slice(&c);
                px[i + 3] = 255;
            }
        }
    }
    px
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, hidden: bool) -> Self {
        let ctx = cc.egui_ctx.clone();
        apply_style(&ctx);

        let settings = config::load();
        if !cfg!(debug_assertions) {
            config::set_autostart(settings.autostart);
        }
        let h: Handle = Arc::new(Mutex::new(Shared {
            cards: Vec::new(),
            progress: Progress::default(),
            settings,
            update: Default::default(),
        }));
        engine::spawn_watcher(h.clone(), ctx.clone());
        update::spawn_checker(h.clone(), ctx.clone());

        let open = MenuItem::with_id("open", "Öffnen", true, None);
        let quit = MenuItem::with_id("quit", "Beenden", true, None);
        let menu = Menu::new();
        let _ = menu.append_items(&[&open, &quit]);
        let tray = TrayIconBuilder::new()
            .with_tooltip("SWORTA")
            .with_icon(tray_icon::Icon::from_rgba(icon_rgba(64), 64, 64).unwrap())
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .expect("tray icon");

        let c = ctx.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| match e.id.0.as_str() {
            "open" => engine::show_window(&c),
            "quit" => update::quit(&c),
            _ => {}
        }));
        let c = ctx.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = e
            {
                engine::show_window(&c);
            }
        }));

        Self { h, tab: Tab::Home, _tray: tray, hide_first: hidden }
    }
}

fn apply_style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = BG;
    v.override_text_color = Some(FG);
    v.selection.bg_fill = RED;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        for (_, f) in s.text_styles.iter_mut() {
            f.family = FontFamily::Monospace;
        }
        s.spacing.item_spacing = vec2(10.0, 10.0);
        s.spacing.button_padding = vec2(18.0, 9.0);
    });
}

fn pill(ui: &mut Ui, text: &str, fill: Color32, fg: Color32, stroke: Color32, enabled: bool) -> egui::Response {
    ui.add_enabled(
        enabled,
        Button::new(RichText::new(text).color(fg).size(12.0))
            .fill(fill)
            .stroke(Stroke::new(1.0, stroke))
            .corner_radius(CornerRadius::same(20)),
    )
}

fn label(ui: &mut Ui, t: &str) {
    ui.label(RichText::new(t).color(MUTED).size(11.0));
}

fn panel() -> Frame {
    Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(CornerRadius::same(22))
        .inner_margin(Margin::same(18))
}

fn toggle(ui: &mut Ui, on: &mut bool) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(48.0, 26.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(13), if *on { RED } else { Color32::from_gray(32) });
    let x = if *on { rect.right() - 13.0 } else { rect.left() + 13.0 };
    p.circle_filled(egui::pos2(x, rect.center().y), 9.0, FG);
    resp
}

fn fmt_bytes(b: u64) -> String {
    let b = b as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else {
        format!("{:.0} MB", b / 1e6)
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if std::mem::take(&mut self.hide_first) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        if ctx.input(|i| i.viewport().close_requested()) && !update::QUITTING.load(std::sync::atomic::Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }

        let (cards, prog) = {
            let g = self.h.lock().unwrap();
            (g.cards.clone(), g.progress.clone())
        };

        Frame::new().inner_margin(Margin::same(22)).show(ui, |ui| {
            ui.horizontal(|ui| {
                let size = dots::text_size("SWORTA", 5.0);
                let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                dots::text(ui.painter(), rect.min, "SWORTA", 5.0, FG, DIM);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                    let c = if prog.state == State::Running {
                        RED
                    } else if !cards.is_empty() {
                        FG
                    } else {
                        Color32::from_gray(50)
                    };
                    ui.painter().circle_filled(r.center(), 5.0, c);
                });
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                for (t, name) in [(Tab::Home, "START"), (Tab::Settings, "EINSTELLUNGEN")] {
                    let on = self.tab == t;
                    let (fill, fg, st) = if on { (FG, BG, FG) } else { (BG, FG, LINE) };
                    if pill(ui, name, fill, fg, st, true).clicked() {
                        self.tab = t;
                    }
                }
            });
            ui.add_space(8.0);

            match self.tab {
                Tab::Home => self.home(ui, &ctx, &cards, &prog),
                Tab::Settings => self.settings(ui, &ctx),
            }
        });
    }
}

impl App {
    fn home(&mut self, ui: &mut Ui, ctx: &egui::Context, cards: &[Card], prog: &Progress) {
        let upd = self.h.lock().unwrap().update.clone();
        match &upd {
            update::Update::Available(r) => {
                panel().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(format!("UPDATE {}", r.tag)).size(14.0).strong());
                    ui.label(RichText::new("Neue Version auf GitHub.").color(MUTED).size(12.0));
                    if pill(ui, "INSTALLIEREN", RED, FG, RED, true).clicked() {
                        update::install(&self.h, ctx, r.clone(), false);
                    }
                });
            }
            update::Update::Installing => {
                ui.label(RichText::new("Update wird installiert …").color(MUTED).size(12.0));
            }
            _ => {}
        }
        label(ui, "SPEICHERKARTE");
        if cards.is_empty() {
            panel().show(ui, |ui| {
                ui.set_width(ui.available_width());
                let t = "NO CARD";
                let (rect, _) = ui.allocate_exact_size(dots::text_size(t, 4.0), Sense::hover());
                dots::text(ui.painter(), rect.min, t, 4.0, MUTED, DIM);
                ui.label(RichText::new("Karte mit DCIM-Ordner einstecken.").color(MUTED).size(12.0));
            });
        }
        for c in cards {
            panel().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(&c.root).size(26.0).strong());
                ui.label(
                    RichText::new(format!(
                        "JPG {}  ·  RAW {}  ·  VID {}  ·  {}",
                        c.jpg, c.raw, c.vid, fmt_bytes(c.bytes)
                    ))
                    .color(MUTED)
                    .size(12.0),
                );
                let busy = prog.state == State::Running;
                if pill(ui, "IMPORTIEREN", RED, FG, RED, !busy && !c.items.is_empty()).clicked() {
                    engine::start_import(&self.h, ctx, &c.root);
                }
            });
        }

        if prog.state != State::Idle {
            ui.add_space(4.0);
            label(ui, "IMPORT");
            panel().show(ui, |ui| {
                ui.set_width(ui.available_width());
                let done = prog.state == State::Done;
                let frac = if prog.bytes_total == 0 {
                    1.0
                } else {
                    (prog.bytes_done as f32 / prog.bytes_total as f32).min(1.0)
                };
                let t = if done { "FERTIG".to_string() } else { format!("{}%", (frac * 100.0) as u32) };
                let (rect, _) = ui.allocate_exact_size(dots::text_size(&t, 7.0), Sense::hover());
                dots::text(ui.painter(), rect.min, &t, 7.0, if done { RED } else { FG }, DIM);
                ui.add_space(4.0);
                let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 12.0), Sense::hover());
                dots::bar(ui.painter(), bar, frac, RED, DIM);
                let line = if done {
                    format!("{} kopiert · {} übersprungen · {} Fehler", prog.copied, prog.skipped, prog.failed)
                } else {
                    format!("{}/{} Dateien · {}", prog.files_done, prog.files_total, prog.current)
                };
                ui.label(RichText::new(line).color(MUTED).size(12.0));
                if done {
                    ui.horizontal(|ui| {
                        let dest = self.h.lock().unwrap().settings.dest.clone();
                        if pill(ui, "ORDNER ÖFFNEN", FG, BG, FG, true).clicked() {
                            let _ = std::process::Command::new("explorer").arg(dest).spawn();
                        }
                        if pill(ui, "OK", BG, FG, LINE, true).clicked() {
                            self.h.lock().unwrap().progress = Progress::default();
                        }
                    });
                }
            });
        }
    }

    fn settings(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let mut s = self.h.lock().unwrap().settings.clone();
        let mut changed = false;

        label(ui, "ZIELORDNER");
        panel().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(s.dest.display().to_string()).size(12.0));
            ui.label(RichText::new("Ziel / JJMMTT / JPG · RAW · VID").color(MUTED).size(11.0));
            if pill(ui, "ÄNDERN", BG, FG, FG, true).clicked() {
                if let Some(p) = rfd::FileDialog::new().set_directory(&s.dest).pick_folder() {
                    s.dest = p;
                    changed = true;
                }
            }
        });

        label(ui, "SYSTEM");
        panel().show(ui, |ui| {
            ui.set_width(ui.available_width());
            for (text, val) in [
                ("Mit Windows starten", &mut s.autostart),
                ("Benachrichtigung bei Karte", &mut s.notify),
                ("Updates automatisch laden", &mut s.auto_update),
            ] {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(text).size(13.0));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        changed |= toggle(ui, val).changed();
                    });
                });
            }
        });

        label(ui, "VERSION");
        panel().show(ui, |ui| {
            ui.set_width(ui.available_width());
            let status = match self.h.lock().unwrap().update.clone() {
                update::Update::Checking => "Suche …".to_string(),
                update::Update::UpToDate => "Aktuell".to_string(),
                update::Update::Available(r) => format!("{} verfügbar", r.tag),
                update::Update::Installing => "Installiere …".to_string(),
                update::Update::Failed(e) => format!("Fehler: {e}"),
                update::Update::Idle => String::new(),
            };
            ui.label(RichText::new(format!("v{}  {}", env!("CARGO_PKG_VERSION"), status)).size(12.0));
            if pill(ui, "NACH UPDATES SUCHEN", BG, FG, FG, true).clicked() {
                let (h, c) = (self.h.clone(), ctx.clone());
                std::thread::spawn(move || update::check(&h, &c, true));
            }
        });

        if changed {
            if s.autostart != self.h.lock().unwrap().settings.autostart {
                config::set_autostart(s.autostart);
            }
            config::save(&s);
            self.h.lock().unwrap().settings = s;
        }
    }
}
