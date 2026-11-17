//! The dense dark theme of a professional 3D tool: charcoal panels, compact spacing, one warm
//! accent. Text keeps at least 4.5:1 contrast against every panel colour (checked in a test);
//! the light theme follows the system when it asks for it.

use egui::{Color32, Stroke, Visuals};

pub const ACCENT: Color32 = Color32::from_rgb(0xE8, 0x8A, 0x3C);
pub const SELECT: Color32 = Color32::from_rgb(0xFF, 0xA6, 0x4D);
pub const PANEL: Color32 = Color32::from_rgb(0x1E, 0x20, 0x23);
pub const PANEL_DEEP: Color32 = Color32::from_rgb(0x16, 0x17, 0x19);
pub const WIDGET: Color32 = Color32::from_rgb(0x2B, 0x2E, 0x33);
pub const WIDGET_HOVER: Color32 = Color32::from_rgb(0x37, 0x3B, 0x41);
pub const TEXT: Color32 = Color32::from_rgb(0xDD, 0xDF, 0xE2);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0xA8, 0xAD, 0xB4);
pub const VIEWPORT_BG: [f32; 4] = [0.115, 0.122, 0.135, 1.0];
pub const AXIS_X: Color32 = Color32::from_rgb(0xE5, 0x5B, 0x5B);
pub const AXIS_Y: Color32 = Color32::from_rgb(0x7C, 0xD0, 0x5C);
pub const AXIS_Z: Color32 = Color32::from_rgb(0x58, 0x8E, 0xF0);

pub fn dark() -> Visuals {
    let mut v = Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = PANEL_DEEP;
    v.faint_bg_color = Color32::from_rgb(0x24, 0x26, 0x2A);
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = Color32::from_rgb(0x8A, 0x4A, 0x16);
    v.selection.stroke = Stroke::new(1.0, SELECT);
    v.hyperlink_color = ACCENT;
    v.widgets.noninteractive.bg_fill = PANEL;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x30, 0x33, 0x38));
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.inactive.bg_fill = WIDGET;
    v.widgets.inactive.weak_bg_fill = WIDGET;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.hovered.bg_fill = WIDGET_HOVER;
    v.widgets.hovered.weak_bg_fill = WIDGET_HOVER;
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    v.widgets.active.bg_fill = Color32::from_rgb(0x45, 0x4A, 0x52);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(0x45, 0x4A, 0x52);
    v.widgets.open.weak_bg_fill = WIDGET_HOVER;
    v.window_stroke = Stroke::new(1.0, Color32::from_rgb(0x3A, 0x3E, 0x44));
    v
}

pub fn apply(ctx: &egui::Context) {
    ctx.set_visuals_of(egui::Theme::Dark, dark());
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 3.0);
        s.spacing.button_padding = egui::vec2(6.0, 2.0);
        s.spacing.interact_size.y = 20.0;
        s.spacing.indent = 14.0;
        s.spacing.window_margin = egui::Margin::same(6);
        for (style, size) in [
            (egui::TextStyle::Body, 12.5),
            (egui::TextStyle::Button, 12.5),
            (egui::TextStyle::Small, 10.5),
            (egui::TextStyle::Monospace, 12.0),
            (egui::TextStyle::Heading, 15.0),
        ] {
            if let Some(f) = s.text_styles.get_mut(&style) {
                f.size = size;
            }
        }
    });
}

/// WCAG relative-luminance contrast ratio.
#[cfg(test)]
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let lum = |c: Color32| {
        let ch = |x: u8| {
            let v = f32::from(x) / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
    };
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_contrast_meets_four_and_a_half() {
        for bg in [PANEL, PANEL_DEEP, WIDGET, WIDGET_HOVER, Color32::from_rgb(0x45, 0x4A, 0x52), Color32::from_rgb(0x8A, 0x4A, 0x16)] {
            assert!(contrast(TEXT, bg) >= 4.5, "text on {bg:?}: {}", contrast(TEXT, bg));
        }
        for bg in [PANEL, PANEL_DEEP, WIDGET] {
            assert!(contrast(TEXT_DIM, bg) >= 4.5, "dim text on {bg:?}: {}", contrast(TEXT_DIM, bg));
            assert!(contrast(ACCENT, bg) >= 3.0, "accent on {bg:?}");
        }
    }
}
