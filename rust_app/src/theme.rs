use eframe::egui::{self, Color32, CornerRadius, FontId, Stroke, TextStyle, Theme};

pub struct Palette {
    pub background: Color32,
    pub surface: Color32,
    pub line: Color32,
    pub muted: Color32,
    pub soft: Color32,
    pub accent: Color32,
    pub accent_text: Color32,
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette { background: Color32::from_rgb(22, 24, 29), surface: Color32::from_rgb(33, 36, 43),
            line: Color32::from_rgb(69, 74, 85), muted: Color32::from_rgb(180, 187, 199),
            soft: Color32::from_rgb(39, 55, 76), accent: Color32::from_rgb(128, 183, 255), accent_text: Color32::from_rgb(15, 32, 53) }
    } else {
        Palette { background: Color32::from_rgb(244, 246, 249), surface: Color32::WHITE,
            line: Color32::from_rgb(218, 224, 232), muted: Color32::from_rgb(83, 96, 112),
            soft: Color32::from_rgb(229, 239, 251), accent: Color32::from_rgb(38, 93, 164), accent_text: Color32::WHITE }
    }
}

pub fn setup(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.text_styles.insert(TextStyle::Body, FontId::proportional(15.0));
        style.text_styles.insert(TextStyle::Button, FontId::proportional(15.0));
        style.text_styles.insert(TextStyle::Heading, FontId::proportional(24.0));
        style.text_styles.insert(TextStyle::Small, FontId::proportional(13.0));
        style.spacing.interact_size.y = 38.0;
        style.spacing.item_spacing = egui::vec2(10.0, 10.0);
        style.spacing.button_padding = egui::vec2(14.0, 7.0);
    });
    for theme in [Theme::Light, Theme::Dark] {
        ctx.style_mut_of(theme, |style| {
            let p = palette(theme == Theme::Dark);
            let v = &mut style.visuals;
            v.panel_fill = p.background;
            v.window_fill = p.surface;
            v.extreme_bg_color = p.surface;
            v.text_edit_bg_color = Some(p.surface);
            v.weak_text_color = Some(p.muted);
            v.selection.bg_fill = p.soft;
            v.selection.stroke = Stroke::new(1.5, p.accent);
            v.widgets.inactive.bg_fill = p.surface;
            v.widgets.inactive.weak_bg_fill = p.surface;
            v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.line);
            v.widgets.hovered.bg_fill = p.soft;
            v.widgets.hovered.weak_bg_fill = p.soft;
            v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.accent);
            v.widgets.active.bg_fill = p.soft;
            v.widgets.active.bg_stroke = Stroke::new(1.5, p.accent);
            v.widgets.noninteractive.bg_fill = p.line;
            v.widgets.noninteractive.weak_bg_fill = p.line;
            let text = if theme == Theme::Dark { Color32::from_rgb(238, 241, 245) } else { Color32::from_rgb(28, 37, 49) };
            v.widgets.noninteractive.fg_stroke.color = text;
            v.widgets.inactive.fg_stroke.color = text;
            v.widgets.hovered.fg_stroke.color = text;
            v.widgets.active.fg_stroke.color = text;
            for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.noninteractive] {
                w.corner_radius = CornerRadius::same(7);
            }
        });
    }
}

pub fn card(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) {
    let p = palette(ui.visuals().dark_mode);
    egui::Frame::new().fill(if ui.visuals().dark_mode { Color32::from_rgba_unmultiplied(33, 36, 43, 242) } else { Color32::from_rgba_unmultiplied(255, 255, 255, 239) }).stroke(Stroke::new(1.0, p.line))
        .corner_radius(10).inner_margin(20).show(ui, |ui| {
            ui.set_width(ui.available_width());
            content(ui);
        });
}

pub fn primary(label: &str, dark: bool) -> egui::Button<'_> {
    let p = palette(dark);
    egui::Button::new(egui::RichText::new(label).strong().color(p.accent_text))
        .fill(p.accent).stroke(Stroke::NONE).corner_radius(7)
}
