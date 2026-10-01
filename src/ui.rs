//! The interface, built to the mockup in `design/`.
//!
//! Dark theme only, fixed 720×520 window with its own title bar, four tabs and
//! an assign dialog. Colours and sizes come from the design tokens in
//! `design/.../_ds/.../styles.css`; the fonts are the system ones the tokens
//! name as fallbacks, since Inter is not installed here.

use std::collections::BTreeMap;
use std::sync::Arc;

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontFamily, FontId, Frame, Layout, Margin, Pos2,
    Rect, RichText, Sense, Stroke, TextStyle, Vec2, ViewportCommand,
};

use crate::ec::{FAN_FULL, FAN_IDLE};
use crate::shared::{button_name, Shared, BUTTONS};
use crate::{actions, ec};

pub const WINDOW: [f32; 2] = [720.0, 520.0];

// Design tokens, dark theme.
const BG: Color32 = Color32::from_rgb(0x16, 0x18, 0x26);
const SURFACE: Color32 = Color32::from_rgb(0x23, 0x25, 0x32);
const TEXT: Color32 = Color32::from_rgb(0xe9, 0xe9, 0xed);
const ACCENT: Color32 = Color32::from_rgb(0x91, 0x84, 0xd9);
const RADIUS: u8 = 4;

/// The design writes its muted text as `color-mix(text N%, transparent)` over
/// the surface it sits on; against these two backgrounds a straight blend of
/// text into bg reads the same.
fn dim(percent: u8) -> Color32 {
    let f = f32::from(percent) / 100.0;
    let blend = |a: u8, b: u8| (f32::from(a) * f + f32::from(b) * (1.0 - f)) as u8;
    Color32::from_rgb(
        blend(TEXT.r(), BG.r()),
        blend(TEXT.g(), BG.g()),
        blend(TEXT.b(), BG.b()),
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Buttons,
    Log,
    Curve,
    Diagnostics,
}

struct Dialog {
    code: u32,
    draft: String,
    saved: bool,
}

pub struct App {
    shared: Shared,
    tab: Tab,
    actions: BTreeMap<u32, String>,
    dialog: Option<Dialog>,
    /// Kept alive: dropping the tray icon removes it from the notification area.
    _tray: Option<tray_icon::TrayIcon>,
    menu: crate::tray::Menu,
}

impl App {
    pub fn new(
        ctx: &egui::Context,
        shared: Shared,
        tray: Option<tray_icon::TrayIcon>,
        menu: crate::tray::Menu,
    ) -> Self {
        install_fonts(ctx);
        install_style(ctx);
        Self {
            shared,
            tab: Tab::Buttons,
            actions: actions::load(),
            dialog: None,
            _tray: tray,
            menu,
        }
    }
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let read = |name: &str| std::fs::read(format!(r"C:\Windows\Fonts\{name}")).ok();

    // The tokens ask for Inter and fall back to system-ui; on Windows that is
    // Segoe UI. Codes are shown in Cascadia Mono, as the mockup specifies.
    if let Some(bytes) = read("segoeui.ttf") {
        fonts
            .font_data
            .insert("ui".into(), Arc::new(egui::FontData::from_owned(bytes)));
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "ui".into());
    }
    if let Some(bytes) = read("seguisb.ttf") {
        fonts
            .font_data
            .insert("ui-medium".into(), Arc::new(egui::FontData::from_owned(bytes)));
        fonts
            .families
            .insert(FontFamily::Name("medium".into()), vec!["ui-medium".into()]);
    }
    if let Some(bytes) = read("CascadiaMono.ttf") {
        fonts
            .font_data
            .insert("mono".into(), Arc::new(egui::FontData::from_owned(bytes)));
        fonts
            .families
            .entry(FontFamily::Monospace)
            .or_default()
            .insert(0, "mono".into());
    }

    ctx.set_fonts(fonts);
}

fn install_style(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.visuals.dark_mode = true;
        style.visuals.panel_fill = BG;
        style.visuals.window_fill = BG;
        style.visuals.extreme_bg_color = SURFACE;
        style.visuals.override_text_color = Some(TEXT);
        style.visuals.selection.bg_fill = ACCENT.gamma_multiply(0.35);
        style.visuals.window_corner_radius = CornerRadius::ZERO;
        style.visuals.widgets.inactive.corner_radius = CornerRadius::same(RADIUS);
        style.visuals.widgets.hovered.corner_radius = CornerRadius::same(RADIUS);
        style.visuals.widgets.active.corner_radius = CornerRadius::same(RADIUS);
        style.spacing.item_spacing = Vec2::new(6.0, 4.0);
        style.spacing.button_padding = Vec2::new(10.0, 4.0);

        style.text_styles = [
            (TextStyle::Body, FontId::proportional(12.0)),
            (TextStyle::Small, FontId::proportional(11.0)),
            (TextStyle::Button, FontId::proportional(12.0)),
            (TextStyle::Heading, FontId::proportional(18.0)),
            (TextStyle::Monospace, FontId::monospace(11.5)),
        ]
        .into();
    });
}

/// 500-weight text, as the design marks its emphasis.
fn medium(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).font(FontId::new(size, FontFamily::Name("medium".into())))
}

fn mono(text: impl Into<String>) -> RichText {
    RichText::new(text).font(FontId::monospace(11.5))
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        BG.to_normalized_gamma_f32()
    }

    /// eframe hands over the root `Ui` rather than the context, so the window is
    /// drawn into it directly — no panel of our own.
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();

        // The window is one of two faces of a background program, so closing it
        // hides it instead of ending the process. Leaving is done from the tray.
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        }

        if let Some(action) = self.menu.poll() {
            match action {
                crate::tray::Action::Open => {
                    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                }
                crate::tray::Action::OpenLog => {
                    let _ = actions::open_in_shell(&actions::log_path());
                }
                crate::tray::Action::Quit => {
                    crate::leave();
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }

        // A press caught while the dialog was listening becomes the selection.
        let caught = {
            let mut state = self.shared.lock().unwrap();
            state.caught.take()
        };
        if let (Some(code), Some(dialog)) = (caught, self.dialog.as_mut()) {
            dialog.code = code;
            dialog.draft = self.actions.get(&code).cloned().unwrap_or_default();
            dialog.saved = false;
        }

        // The whole window, with none of the root margin: the design does its
        // own spacing, down to the 30px title bar at the very top.
        let rect = root.max_rect();
        root.scope_builder(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::top_down(Align::Min)),
            |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                self.title_bar(ui);
                self.banner(ui);
                self.readings(ui);
                self.tabs(ui);
                self.body(ui);
                self.status_bar(ui);
            },
        );

        self.dialog(&ctx);

        // The readings come from a polling thread; a second is the rate it
        // publishes at.
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
}

impl App {
    fn title_bar(&mut self, ui: &mut egui::Ui) {
        let height = 30.0;
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click_and_drag());
        if response.drag_started() {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }

        let mut bar = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(
            Layout::left_to_right(Align::Center),
        ));
        bar.add_space(10.0);
        bar.label(RichText::new("msi-hotkeys").size(12.0));

        bar.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // No maximise: the window is a fixed 720×520, and a button that
            // cannot do anything is worse than no button.
            if caption_button(ui, "✕") {
                ui.ctx().send_viewport_cmd(ViewportCommand::Visible(false));
            }
            if caption_button(ui, "—") {
                ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
            }
        });
    }

    fn banner(&mut self, ui: &mut egui::Ui) {
        let armed = self.shared.lock().unwrap().snapshot.armed;

        Frame::new()
            .fill(SURFACE)
            .inner_margin(Margin::symmetric(14, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    match armed {
                        Some(true) => {
                            let (dot, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                            ui.painter().circle_filled(dot.center(), 4.0, ACCENT);
                            ui.add_space(4.0);
                            ui.label(medium("Канал событий вооружён.", 12.0));
                            ui.label(
                                RichText::new("Аппаратные кнопки доходят до программы.")
                                    .color(dim(65)),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("Снять").clicked() {
                                    self.set_armed(false);
                                }
                            });
                        }
                        Some(false) => {
                            ui.label(RichText::new("⚠").size(15.0));
                            ui.label(medium("Канал событий не вооружён.", 12.0));
                            ui.label(
                                RichText::new(
                                    "Кнопки не доходят до программы. Вооружение — запись одного байта в контроллер.",
                                )
                                .color(dim(65)),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("Вооружить").clicked() {
                                    self.set_armed(true);
                                }
                            });
                        }
                        None => {
                            ui.label(RichText::new("⚠").size(15.0));
                            ui.label(medium("Состояние канала неизвестно.", 12.0));
                            ui.label(
                                RichText::new("root\\WMI не читается — нужны права администратора.")
                                    .color(dim(65)),
                            );
                        }
                    }
                });
            });
    }

    fn set_armed(&mut self, armed: bool) {
        let note = match ec::set_armed_standalone(armed) {
            Ok(_) => None,
            Err(e) => Some(format!("{e:#}")),
        };
        let mut state = self.shared.lock().unwrap();
        state.snapshot.armed = Some(armed);
        state.note = note;
    }

    fn readings(&mut self, ui: &mut egui::Ui) {
        let snapshot = self.shared.lock().unwrap().snapshot.clone();

        Frame::new()
            .inner_margin(Margin {
                left: 14,
                right: 14,
                top: 10,
                bottom: 4,
            })
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    ui.allocate_ui(Vec2::new(120.0, 48.0), |ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new("CPU").size(11.0).color(dim(60)));
                            ui.label(medium(
                                match snapshot.cpu {
                                    Some(t) => format!("{t} °C"),
                                    None => "—".into(),
                                },
                                18.0,
                            ));
                        });
                    });
                    ui.add_space(20.0);

                    ui.allocate_ui(Vec2::new(200.0, 48.0), |ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new("Тахометр вентилятора").size(11.0).color(dim(60)),
                            );
                            ui.horizontal(|ui| {
                                ui.label(medium(
                                    match snapshot.fan {
                                        Some(v) => v.to_string(),
                                        None => "—".into(),
                                    },
                                    18.0,
                                ));
                                ui.label(
                                    RichText::new(if snapshot.boost_running() {
                                        "у максимума — идёт Cooler Boost"
                                    } else {
                                        "покой"
                                    })
                                    .color(dim(65)),
                                );
                            });
                            fan_bar(ui, snapshot.fan);
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!("{FAN_IDLE} покой"))
                                        .size(10.5)
                                        .color(dim(55)),
                                );
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.label(
                                        RichText::new(format!("{FAN_FULL} макс."))
                                            .size(10.5)
                                            .color(dim(55)),
                                    );
                                });
                            });
                        });
                    });
                    ui.add_space(20.0);

                    ui.vertical(|ui| {
                        ui.label(RichText::new("Питание").size(11.0).color(dim(60)));
                        ui.label(medium(
                            match snapshot.on_mains {
                                Some(true) => "От сети",
                                Some(false) => "От батареи",
                                None => "—",
                            },
                            18.0,
                        ));
                        let mut parts = Vec::new();
                        if let Some(p) = snapshot.percent {
                            parts.push(format!("{p} %"));
                        }
                        if let Some(v) = snapshot.volts {
                            parts.push(format!("{v:.2} В"));
                        }
                        if snapshot.charging {
                            parts.push("заряд".into());
                        } else if snapshot.discharging {
                            parts.push("разряд".into());
                        }
                        ui.label(RichText::new(parts.join(" · ")).color(dim(65)));
                    });
                });
            });
    }

    fn tabs(&mut self, ui: &mut egui::Ui) {
        let presses = self.shared.lock().unwrap().presses.len();

        Frame::new()
            .inner_margin(Margin {
                left: 10,
                right: 10,
                top: 6,
                bottom: 0,
            })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let tabs = [
                        (Tab::Buttons, "Кнопки", BUTTONS.len().to_string()),
                        (Tab::Log, "Журнал", presses.to_string()),
                        (Tab::Curve, "Кривая вентилятора", String::new()),
                        (Tab::Diagnostics, "Диагностика", String::new()),
                    ];
                    for (tab, label, count) in tabs {
                        let active = self.tab == tab;
                        let text = if active {
                            medium(label, 12.0)
                        } else {
                            RichText::new(label).color(dim(65))
                        };

                        let response = ui
                            .scope(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                ui.horizontal(|ui| {
                                    ui.label(text);
                                    if !count.is_empty() {
                                        ui.label(
                                            RichText::new(count).size(10.5).color(dim(50)),
                                        );
                                    }
                                })
                                .response
                            })
                            .inner
                            .interact(Sense::click());

                        if active {
                            let rect = response.rect;
                            ui.painter().hline(
                                rect.x_range(),
                                rect.bottom() + 3.0,
                                Stroke::new(2.0, ACCENT),
                            );
                        }
                        if response.clicked() {
                            self.tab = tab;
                        }
                        ui.add_space(8.0);
                    }
                });
            });

        let y = ui.cursor().top() + 3.0;
        ui.painter()
            .hline(ui.max_rect().x_range(), y, Stroke::new(1.0, dim(16)));
        ui.add_space(6.0);
    }

    fn body(&mut self, ui: &mut egui::Ui) {
        // Everything below the tabs down to the status bar.
        let height = ui.available_height() - 24.0;
        egui::ScrollArea::vertical()
            .max_height(height)
            .auto_shrink([false, false])
            .show(ui, |ui| match self.tab {
                Tab::Buttons => self.tab_buttons(ui),
                Tab::Log => self.tab_log(ui),
                Tab::Curve => self.tab_curve(ui),
                Tab::Diagnostics => self.tab_diagnostics(ui),
            });
    }

    fn tab_buttons(&mut self, ui: &mut egui::Ui) {
        let mut open: Option<u32> = None;

        egui::Grid::new("buttons")
            .num_columns(4)
            .spacing([6.0, 5.0])
            .min_col_width(0.0)
            .show(ui, |ui| {
                for (label, width) in [
                    ("КОД", 96.0),
                    ("КНОПКА", 210.0),
                    ("КОМАНДА", 220.0),
                    ("", 92.0),
                ] {
                    ui.allocate_ui(Vec2::new(width, 14.0), |ui| {
                        ui.label(RichText::new(label).size(10.5).color(dim(55)));
                    });
                }
                ui.end_row();

                for (code, name) in BUTTONS {
                    ui.label(mono(format!("0x{code:06X}")));

                    match name {
                        Some(name) => ui.label(name),
                        None => ui.label(RichText::new("неопознанная").color(dim(55))),
                    };

                    match self.actions.get(&code) {
                        Some(command) if !command.is_empty() => ui.label(mono(command.clone())),
                        Some(_) => ui.label(RichText::new("пусто").color(dim(55))),
                        None => ui.label(RichText::new("—").color(dim(35))),
                    };

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let label = if self.actions.contains_key(&code) {
                            "Изменить…"
                        } else {
                            "Назначить…"
                        };
                        if ui.button(label).clicked() {
                            open = Some(code);
                        }
                    });
                    ui.end_row();
                }
            });

        if let Some(code) = open {
            self.dialog = Some(Dialog {
                code,
                draft: self.actions.get(&code).cloned().unwrap_or_default(),
                saved: false,
            });
        }
    }

    fn tab_log(&mut self, ui: &mut egui::Ui) {
        let presses = self.shared.lock().unwrap().presses.clone();

        if presses.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new("Нажатий пока не было.").color(dim(60)));
            return;
        }

        for press in presses {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                sized(ui, 64.0, |ui| {
                    ui.label(RichText::new(press.at.clone()).color(dim(60)));
                });
                sized(ui, 74.0, |ui| {
                    ui.label(mono(format!("0x{:06X}", press.code)));
                });
                sized(ui, 168.0, |ui| match button_name(press.code) {
                    Some(name) => {
                        ui.label(name);
                    }
                    None => {
                        ui.label(RichText::new("неопознанная").color(dim(55)));
                    }
                });
                sized(ui, 82.0, |ui| {
                    ui.label(match press.cpu {
                        Some(t) => format!("CPU ~{t} °C"),
                        None => "CPU —".into(),
                    });
                });
                sized(ui, 100.0, |ui| {
                    ui.label(match press.fan {
                        Some(v) => format!("вентилятор {v}"),
                        None => "вентилятор —".into(),
                    });
                });
                ui.label(
                    RichText::new(match press.on_mains {
                        Some(true) => "от сети",
                        Some(false) => "от батареи",
                        None => "",
                    })
                    .color(dim(65)),
                );
            });
            ui.add_space(1.0);
        }
    }

    fn tab_curve(&mut self, ui: &mut egui::Ui) {
        let snapshot = self.shared.lock().unwrap().snapshot.clone();

        ui.horizontal_top(|ui| {
            ui.add_space(14.0);
            ui.vertical(|ui| {
                curve_chart(ui, &snapshot.curve_cpu, &snapshot.curve_gpu);
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let (dash, _) = ui.allocate_exact_size(Vec2::new(14.0, 8.0), Sense::hover());
                    ui.painter().hline(
                        dash.x_range(),
                        dash.center().y,
                        Stroke::new(2.0, ACCENT),
                    );
                    ui.label(RichText::new("CPU").size(11.0).color(dim(65)));
                    ui.add_space(8.0);
                    let (dash, _) = ui.allocate_exact_size(Vec2::new(14.0, 8.0), Sense::hover());
                    ui.painter().hline(
                        dash.x_range(),
                        dash.center().y,
                        Stroke::new(2.0, dim(55)),
                    );
                    ui.label(RichText::new("GPU").size(11.0).color(dim(65)));
                });
                ui.label(
                    RichText::new("по оси X — °C, по оси Y — значение оборотов из контроллера")
                        .size(11.0)
                        .color(dim(65)),
                );
            });
            ui.add_space(20.0);

            ui.vertical(|ui| {
                for (name, points) in [("CPU", &snapshot.curve_cpu), ("GPU", &snapshot.curve_gpu)] {
                    ui.label(medium(name, 12.0));
                    ui.horizontal_wrapped(|ui| {
                        for (t, v) in points {
                            sized(ui, 44.0, |ui| {
                                ui.label(mono(format!("{t}:{v}")));
                            });
                        }
                    });
                    ui.add_space(8.0);
                }
                ui.label(
                    RichText::new(
                        "Только чтение: кривую задаёт контроллер, программа её показывает.",
                    )
                    .size(11.0)
                    .color(dim(55)),
                );
            });
        });
    }

    fn tab_diagnostics(&mut self, ui: &mut egui::Ui) {
        let snapshot = self.shared.lock().unwrap().snapshot.clone();

        Frame::new()
            .inner_margin(Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "Сырые байты контроллера · снимок {} · значения без расшифровки",
                            snapshot.taken_at
                        ))
                        .color(dim(60)),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Копировать").clicked() {
                            ui.ctx().copy_text(raw_text(&snapshot));
                        }
                    });
                });
                ui.add_space(6.0);

                Frame::new()
                    .fill(SURFACE)
                    .inner_margin(Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        for (name, values) in &snapshot.raw {
                            ui.horizontal(|ui| {
                                sized(ui, 120.0, |ui| {
                                    ui.label(mono(format!("{name}=")).color(dim(60)));
                                });
                                ui.label(mono(format!("[{}]", hex(values))));
                            });
                        }
                    });
            });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let (note, armed, last) = {
            let state = self.shared.lock().unwrap();
            (
                state.note.clone(),
                state.snapshot.armed,
                state.presses.first().map(|p| p.at.clone()),
            )
        };

        ui.painter()
            .hline(ui.max_rect().x_range(), ui.cursor().top(), Stroke::new(1.0, dim(16)));

        Frame::new()
            .inner_margin(Margin::symmetric(14, 4))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("MSI GL72 6QD · штатный SCM не запущен")
                            .size(11.0)
                            .color(dim(55)),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let tail = match (note, armed, last) {
                            (Some(note), _, _) => note,
                            (None, Some(true), Some(at)) => format!("последнее нажатие {at}"),
                            (None, Some(true), None) => "нажатий ещё не было".into(),
                            _ => "нажатия не принимаются".into(),
                        };
                        ui.label(RichText::new(tail).size(11.0).color(dim(55)));
                    });
                });
            });
    }

    fn dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };

        let code = dialog.code;
        let mut close = false;
        let mut save = false;
        let mut listen = false;

        let (listening, armed, last) = {
            let state = self.shared.lock().unwrap();
            (
                state.listening,
                state.snapshot.armed.unwrap_or(false),
                state
                    .presses
                    .iter()
                    .find(|p| p.code == code)
                    .map(|p| p.at.clone()),
            )
        };

        egui::Window::new("Назначить действие")
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .frame(Frame::new().fill(BG).inner_margin(Margin::same(16)))
            .show(ctx, |ui| {
                Frame::new()
                    .fill(SURFACE)
                    .inner_margin(Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(format!("0x{code:06X}"))
                                            .font(FontId::monospace(15.0)),
                                    );
                                    match button_name(code) {
                                        Some(name) => ui.label(medium(name, 12.0)),
                                        None => ui.label(
                                            RichText::new("неопознанная кнопка").color(dim(60)),
                                        ),
                                    };
                                });
                                let line = if listening {
                                    "Нажмите кнопку на ноутбуке…".to_string()
                                } else {
                                    match last {
                                        Some(at) => format!("Последнее нажатие: {at}"),
                                        None => "В журнале нажатий нет".into(),
                                    }
                                };
                                ui.label(RichText::new(line).size(11.0).color(dim(60)));
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let label = if listening {
                                    "Жду нажатия…"
                                } else {
                                    "Выбрать нажатием"
                                };
                                if ui
                                    .add_enabled(armed, egui::Button::new(label))
                                    .clicked()
                                {
                                    listen = true;
                                }
                            });
                        });
                    });

                if !armed {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "Чтобы выбрать кнопку нажатием, сначала вооружите канал событий.",
                        )
                        .size(11.0)
                        .color(dim(65)),
                    );
                }

                ui.add_space(12.0);
                ui.label("Команда");
                ui.add_space(2.0);
                ui.add(
                    egui::TextEdit::singleline(&mut dialog.draft)
                        .desired_width(f32::INFINITY)
                        .font(FontId::monospace(12.0))
                        .hint_text(r"например: C:\Windows\System32\SnippingTool.exe"),
                );
                ui.add_space(5.0);
                ui.label(
                    RichText::new("Пустая команда — нажатие только записывается в журнал.")
                        .size(11.0)
                        .color(dim(60)),
                );

                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    let empty = dialog.draft.trim().is_empty();
                    if ui
                        .add_enabled(!empty, egui::Button::new("Проверить"))
                        .clicked()
                    {
                        let _ = actions::run(dialog.draft.trim());
                    }
                    if ui.button("Очистить").clicked() {
                        dialog.draft.clear();
                        dialog.saved = false;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Сохранить").clicked() {
                            save = true;
                        }
                        if ui.button("Отмена").clicked() {
                            close = true;
                        }
                        if dialog.saved {
                            ui.label(RichText::new("Сохранено").size(11.0).color(dim(60)));
                        }
                    });
                });
            });

        if listen {
            let mut state = self.shared.lock().unwrap();
            state.listening = !state.listening;
        }
        if save {
            let draft = self.dialog.as_ref().map(|d| d.draft.trim().to_string());
            if let Some(draft) = draft {
                self.actions.insert(code, draft);
                let note = actions::save(&self.actions)
                    .err()
                    .map(|e| format!("не удалось записать actions.txt: {e:#}"));
                self.shared.lock().unwrap().note = note;
            }
            if let Some(dialog) = self.dialog.as_mut() {
                dialog.saved = true;
            }
        }
        if close {
            self.dialog = None;
            self.shared.lock().unwrap().listening = false;
        }
    }
}

/// Runs a closure in a fixed-width slot, so columns line up without a grid.
fn sized(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(
        Vec2::new(width, ui.spacing().interact_size.y),
        Layout::left_to_right(Align::Center),
        add,
    );
}

fn caption_button(ui: &mut egui::Ui, glyph: &str) -> bool {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(46.0, 30.0), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::ZERO, SURFACE);
    }
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(12.0),
        TEXT,
    );
    response.clicked()
}

fn fan_bar(ui: &mut egui::Ui, fan: Option<u64>) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(200.0, 3.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::ZERO, dim(14));

    if let Some(fan) = fan {
        let span = (FAN_FULL - FAN_IDLE) as f32;
        let part = ((fan.saturating_sub(FAN_IDLE)) as f32 / span).clamp(0.0, 1.0);
        if part > 0.0 {
            let filled = Rect::from_min_size(rect.min, Vec2::new(rect.width() * part, rect.height()));
            ui.painter().rect_filled(filled, CornerRadius::ZERO, dim(55));
        }
    }
}

/// The chart from the mockup: temperature across, controller fan value up, CPU
/// solid in the accent and GPU dashed in a muted tone.
fn curve_chart(ui: &mut egui::Ui, cpu: &[(u64, u64)], gpu: &[(u64, u64)]) {
    let size = Vec2::new(340.0, 190.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);

    // Same mapping the mockup uses: 50..80 °C across, 40..90 up.
    let at = |t: f64, v: f64| {
        Pos2::new(
            rect.left() + 30.0 + ((t - 50.0) / 30.0 * 300.0) as f32,
            rect.top() + 170.0 - ((v - 40.0) / 50.0 * 160.0) as f32,
        )
    };

    let grid = Stroke::new(1.0, dim(10));
    for t in [50, 55, 60, 65, 70, 75, 80] {
        let x = at(f64::from(t), 0.0).x;
        painter.vline(x, rect.top() + 10.0..=rect.top() + 170.0, grid);
        painter.text(
            Pos2::new(x, rect.top() + 176.0),
            Align2::CENTER_TOP,
            t.to_string(),
            FontId::proportional(10.0),
            dim(55),
        );
    }
    for v in [40, 50, 60, 70, 80, 90] {
        let y = at(50.0, f64::from(v)).y;
        painter.hline(rect.left() + 30.0..=rect.left() + 330.0, y, grid);
        painter.text(
            Pos2::new(rect.left() + 24.0, y),
            Align2::RIGHT_CENTER,
            v.to_string(),
            FontId::proportional(10.0),
            dim(55),
        );
    }

    let plot = |points: &[(u64, u64)], colour: Color32, dashed: bool| {
        let path: Vec<Pos2> = points
            .iter()
            .map(|(t, v)| at(*t as f64, *v as f64))
            .collect();
        if path.len() >= 2 {
            if dashed {
                for pair in path.windows(2) {
                    painter.add(egui::Shape::dashed_line(
                        pair,
                        Stroke::new(1.5, colour),
                        4.0,
                        3.0,
                    ));
                }
            } else {
                painter.add(egui::Shape::line(path.clone(), Stroke::new(1.5, colour)));
            }
        }
        for point in path {
            painter.circle_filled(point, 2.5, colour);
        }
    };

    plot(gpu, dim(55), true);
    plot(cpu, ACCENT, false);
}

fn hex(values: &[u64]) -> String {
    values
        .iter()
        .map(|v| format!("{v:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn raw_text(snapshot: &ec::Snapshot) -> String {
    snapshot
        .raw
        .iter()
        .map(|(name, values)| format!("{name}=[{}]", hex(values)))
        .collect::<Vec<_>>()
        .join("\n")
}
