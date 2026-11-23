//! Palette and hand-drawn charts for the Guardian dashboard.
//! Colours follow the `data-terminal` direction: colour lives on data and status,
//! never on chrome. One accent, used only for selection and the live line.

use eframe::egui::{self, Color32, Pos2, Stroke, Ui, Vec2};
use crate::ui_strings::Theme;

/// One theme's worth of colour roles.
pub struct Palette {
    pub surface: Color32,
    pub border: Color32,
    pub ink: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub green: Color32,
    pub red: Color32,
    pub amber: Color32,
    pub track: Color32,
}

/// App chrome ground — one tone below surfaces, never navy or slate.
pub fn ground(theme: Theme) -> Color32 {
    match theme {
        Theme::Light => Color32::from_rgb(245, 246, 248),
        Theme::Dark => Color32::from_rgb(23, 23, 25),
    }
}

pub fn palette(theme: Theme) -> Palette {
    match theme {
        Theme::Light => Palette {
            surface: Color32::from_rgb(255, 255, 255),
            border: Color32::from_rgb(229, 231, 235),
            ink: Color32::from_rgb(22, 24, 29),
            muted: Color32::from_rgb(95, 99, 104),
            accent: Color32::from_rgb(47, 111, 192),
            green: Color32::from_rgb(36, 123, 80),
            red: Color32::from_rgb(196, 36, 42),
            amber: Color32::from_rgb(146, 64, 14),
            track: Color32::from_rgb(232, 234, 238),
        },
        Theme::Dark => Palette {
            surface: Color32::from_rgb(19, 18, 23),
            border: Color32::from_rgb(34, 34, 36),
            ink: Color32::from_gray(255),
            muted: Color32::from_rgb(138, 140, 146),
            accent: Color32::from_rgb(106, 166, 232),
            green: Color32::from_rgb(68, 162, 130),
            red: Color32::from_rgb(229, 86, 111),
            amber: Color32::from_rgb(217, 164, 65),
            track: Color32::from_rgb(43, 43, 43),
        },
    }
}

/// Apply palette to egui's own widgets so theme switching covers chrome too.
pub fn apply_visuals(ctx: &egui::Context, theme: Theme) {
    let p = palette(theme);
    let mut visuals = match theme {
        Theme::Light => egui::Visuals::light(),
        Theme::Dark => egui::Visuals::dark(),
    };
    visuals.selection.bg_fill = p.accent;
    visuals.hyperlink_color = p.accent;
    // The panel/window ground drives menu bar, central panel and title backgrounds.
    visuals.panel_fill = ground(theme);
    visuals.window_fill = ground(theme);
    visuals.widgets.noninteractive.bg_fill = p.surface;
    visuals.widgets.noninteractive.fg_stroke.color = p.ink;
    // ponytail: one stroke width for all noninteractive text; no per-role tuning needed.
    ctx.set_visuals(visuals);
}

/// One container split by hairlines: a row of KPI cells.
pub fn stat_strip(ui: &mut Ui, theme: Theme, cells: &[(&str, &str, Color32)]) {
    let p = palette(theme);
    let height = 56.0;
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), height), egui::Sense::hover());
    let paint = ui.painter();
    paint.rect_filled(rect, 6.0, p.surface);
    paint.rect_stroke(rect, 6.0, Stroke::new(1.0_f32, p.border), egui::StrokeKind::Inside);
    let col = rect.width() / cells.len() as f32;
    for (i, (label, value, color)) in cells.iter().enumerate() {
        let x = rect.left() + col * i as f32 + 12.0;
        if i > 0 {
            let dx = rect.left() + col * i as f32;
            paint.line_segment(
                [egui::pos2(dx, rect.top() + 10.0), egui::pos2(dx, rect.bottom() - 10.0)],
                Stroke::new(1.0_f32, p.border),
            );
        }
        paint.text(egui::pos2(x, rect.top() + 14.0), egui::Align2::LEFT_TOP,
            *label, egui::FontId::proportional(11.0), p.muted);
        paint.text(egui::pos2(x, rect.top() + 24.0), egui::Align2::LEFT_TOP,
            *value, egui::FontId::proportional(20.0), *color);
    }
}

/// Restart trend: one line, quiet horizontal gridlines, round ticks, no markers.
/// Values are real restart counts per bucket from crash history.
pub fn line_chart(ui: &mut Ui, theme: Theme, values: &[f32], labels: &[String]) {
    let p = palette(theme);
    let height = 150.0;
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let paint = ui.painter();
    let pad_l = 26.0;
    let pad_b = 18.0;
    let plot = egui::Rect::from_min_max(
        rect.min + Vec2::new(pad_l, 4.0),
        rect.max - Vec2::new(4.0, pad_b),
    );

    let top = values.iter().copied().fold(1.0f32, f32::max).ceil().max(2.0);
    let ticks = 4;
    let grid = Stroke::new(1.0_f32, p.track);
    for i in 0..=ticks {
        let y = plot.bottom() - (plot.height() * i as f32 / ticks as f32);
        paint.line_segment([egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)], grid);
        let v = top * i as f32 / ticks as f32;
        paint.text(
            egui::pos2(plot.left() - 6.0, y),
            egui::Align2::RIGHT_CENTER,
            format!("{:.0}", v),
            egui::FontId::proportional(10.0),
            p.muted,
        );
    }

    if values.len() > 1 {
        let step = plot.width() / (values.len() - 1) as f32;
        let points: Vec<Pos2> = values
            .iter()
            .enumerate()
            .map(|(i, v)| {
                egui::pos2(
                    plot.left() + step * i as f32,
                    plot.bottom() - (v / top) * plot.height(),
                )
            })
            .collect();
        paint.line(points, Stroke::new(2.0_f32, p.accent));
    }

    // x labels: first, middle, last only — one step, never rotated.
    let denom = values.len().saturating_sub(1).max(1) as f32;
    for &i in &[0usize, values.len() / 2, values.len().saturating_sub(1)] {
        if let Some(label) = labels.get(i) {
            let x = plot.left() + (plot.width() * i as f32 / denom);
            paint.text(
                egui::pos2(x, rect.bottom() - 6.0),
                if i == 0 { egui::Align2::LEFT_BOTTOM }
                else if i == values.len() - 1 { egui::Align2::RIGHT_BOTTOM }
                else { egui::Align2::CENTER_BOTTOM },
                label,
                egui::FontId::proportional(10.0),
                p.muted,
            );
        }
    }
}

/// Online-health ring: one filled arc on a quiet track, percentage in the middle.
pub fn ring_chart(ui: &mut Ui, theme: Theme, ratio: f32) {
    let p = palette(theme);
    let size = 96.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(size, size), egui::Sense::hover());
    let center = rect.center();
    let radius = size / 2.0 - 8.0;
    let paint = ui.painter();

    // track: a full faint circle via a polygon (egui has no arc primitive).
    paint.circle_stroke(center, radius, Stroke::new(8.0_f32, p.track));
    let ratio = ratio.clamp(0.0, 1.0);
    if ratio > 0.0 {
        // ponytail: 64 straight segments read as a smooth ring at this size; no arc helper needed.
        let segments = (64.0 * ratio).ceil() as usize;
        let pts: Vec<Pos2> = (0..=segments)
            .map(|i| {
                let a = -std::f32::consts::FRAC_PI_2
                    + 2.0 * std::f32::consts::PI * (i as f32 / 64.0);
                egui::pos2(center.x + radius * a.cos(), center.y + radius * a.sin())
            })
            .collect();
        paint.line(pts, Stroke::new(8.0_f32, p.green));
    }
    paint.text(
        center,
        egui::Align2::CENTER_CENTER,
        format!("{:.0}%", ratio * 100.0),
        egui::FontId::proportional(20.0),
        p.ink,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palettes_are_distinct_per_theme() {
        assert_ne!(palette(Theme::Light).surface, palette(Theme::Dark).surface);
        assert_ne!(palette(Theme::Light).ink, palette(Theme::Dark).ink);
    }
}
