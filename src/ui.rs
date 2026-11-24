//! Palette and hand-drawn charts for the Guardian dashboard.
//! Colours follow the `data-terminal` direction: colour lives on data and status,
//! never on chrome. One accent, used only for selection and the live line.

use eframe::egui::{self, Color32, Pos2, Stroke, Ui, Vec2};
use crate::ui_strings::Theme;

/// Vector icons drawn from primitives. The CJK system font has no glyphs for the
/// geometric symbols the prototype uses, so text glyphs render as tofu boxes —
/// drawing them as shapes is the only reliable fix across Windows fonts.
#[derive(Debug, Clone, Copy)]
pub enum Icon {
    Shield, Grid, List, Clock, Gear, Info, Minimize, Maximize, Restore, Close, Plus,
}

/// Draw a vector icon centered in `size` x `size` points, allocating its own space.
pub fn icon(ui: &mut Ui, kind: Icon, color: Color32, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(size, size), egui::Sense::hover());
    icon_at(ui.painter(), rect.center(), kind, color, size);
}

/// Draw a vector icon at an explicit center — lets callers overlay it on a button rect.
pub fn icon_at(p: &egui::Painter, c: Pos2, kind: Icon, color: Color32, size: f32) {
    let e = size * 0.5;                 // half extent
    let st = Stroke::new(1.4_f32, color);
    let at = |x: f32, y: f32| egui::pos2(c.x + x * e, c.y + y * e);
    match kind {
        Icon::Shield => {
            p.line(vec![at(0.0, -0.9), at(0.7, -0.5), at(0.7, 0.25), at(0.0, 0.9),
                    at(-0.7, 0.25), at(-0.7, -0.5), at(0.0, -0.9)], st);
            p.line(vec![at(-0.3, 0.0), at(-0.05, 0.28), at(0.38, -0.3)], st);
        }
        Icon::Grid => {
            for (x, y) in [(-0.55, -0.55), (0.1, -0.55), (-0.55, 0.1), (0.1, 0.1)] {
                p.rect_stroke(egui::Rect::from_min_max(at(x, y), at(x + 0.45, y + 0.45)), 0.0, st, egui::StrokeKind::Outside);
            }
        }
        Icon::List => {
            for y in [-0.5, 0.0, 0.5] {
                p.line_segment([at(-0.7, y), at(0.7, y)], st);
            }
            p.circle_filled(at(-0.45, -0.5), size * 0.055, color);
            p.circle_filled(at(0.2, 0.0), size * 0.055, color);
        }
        Icon::Clock => {
            p.circle_stroke(c, e * 0.78, st);
            p.line_segment([at(0.0, 0.0), at(0.0, -0.45)], st);
            p.line_segment([at(0.0, 0.0), at(0.35, 0.22)], st);
        }
        Icon::Gear => {
            p.circle_stroke(c, e * 0.45, st);
            for i in 0..8 {
                let a = std::f32::consts::FRAC_PI_4 * i as f32;
                let (sx, sy) = (a.cos(), a.sin());
                p.line_segment([at(sx * 0.6, sy * 0.6), at(sx * 0.9, sy * 0.9)], st);
            }
        }
        Icon::Info => {
            p.circle_stroke(c, e * 0.8, st);
            p.circle_filled(at(0.0, -0.42), size * 0.06, color);
            p.line_segment([at(0.0, -0.15), at(0.0, 0.5)], st);
        }
        Icon::Minimize => { p.line_segment([at(-0.6, 0.0), at(0.6, 0.0)], st); }
        Icon::Maximize => { p.rect_stroke(egui::Rect::from_min_max(at(-0.6, -0.6), at(0.6, 0.6)), 0.0, st, egui::StrokeKind::Outside); }
        Icon::Restore => {
            p.rect_stroke(egui::Rect::from_min_max(at(-0.6, -0.35), at(0.35, 0.6)), 0.0, st, egui::StrokeKind::Outside);
            p.rect_stroke(egui::Rect::from_min_max(at(-0.35, -0.6), at(0.6, 0.35)), 0.0, st, egui::StrokeKind::Outside);
        }
        Icon::Close => {
            p.line_segment([at(-0.5, -0.5), at(0.5, 0.5)], st);
            p.line_segment([at(0.5, -0.5), at(-0.5, 0.5)], st);
        }
        Icon::Plus => {
            p.line_segment([at(-0.55, 0.0), at(0.55, 0.0)], st);
            p.line_segment([at(0.0, -0.55), at(0.0, 0.55)], st);
        }
    }
}

/// One theme's worth of colour roles.
pub struct Palette {
    pub surface: Color32,
    pub border: Color32,
    pub ink: Color32,
    pub muted: Color32,
    pub accent: Color32,
    /// Soft tint for selected/pressed widgets and text selection. Kept much
    /// lighter than `accent` so settings toggles stay quiet; the strong accent
    /// is reserved for data (charts, live line, links).
    pub selection_bg: Color32,
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
            border: Color32::from_rgb(224, 228, 234),
            ink: Color32::from_rgb(26, 32, 44),
            muted: Color32::from_rgb(106, 114, 128),
            accent: Color32::from_rgb(37, 99, 235),
            selection_bg: Color32::from_rgb(226, 236, 252),
            green: Color32::from_rgb(22, 138, 88),
            red: Color32::from_rgb(208, 48, 60),
            amber: Color32::from_rgb(161, 98, 7),
            track: Color32::from_rgb(234, 238, 243),
        },
        Theme::Dark => Palette {
            surface: Color32::from_rgb(19, 18, 23),
            border: Color32::from_rgb(34, 34, 36),
            ink: Color32::from_gray(255),
            muted: Color32::from_rgb(138, 140, 146),
            accent: Color32::from_rgb(106, 166, 232),
            selection_bg: Color32::from_rgb(40, 60, 82),
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
    // Selected toggles / text selection: soft tint with accent text, not a solid
    // cobalt block. Keeps the strong accent for data marks (charts, live line).
    visuals.selection.bg_fill = p.selection_bg;
    visuals.selection.stroke = Stroke::new(1.0_f32, p.accent);
    visuals.hyperlink_color = p.accent;
    // The panel/window ground drives menu bar, central panel and title backgrounds.
    visuals.panel_fill = ground(theme);
    visuals.window_fill = ground(theme);
    visuals.widgets.noninteractive.bg_fill = p.surface;
    visuals.widgets.noninteractive.fg_stroke.color = p.ink;
    // Rounded corners everywhere the prototype shows pills/buttons.
    visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(6);
    visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(6);
    visuals.widgets.active.corner_radius = egui::CornerRadius::same(6);
    visuals.widgets.open.corner_radius = egui::CornerRadius::same(6);
    // Keep button text on the normal ink colour in every interaction state.
    // Selected toggles get their accent text from `selection.stroke` above.
    visuals.widgets.inactive.fg_stroke.color = p.ink;
    visuals.widgets.hovered.fg_stroke.color = p.ink;
    visuals.widgets.active.fg_stroke.color = p.ink;
    visuals.widgets.open.fg_stroke.color = p.ink;
    // Roomy buttons, matching the prototype's pill padding.
    ctx.style_mut(|style| style.spacing.button_padding = egui::vec2(12.0, 6.0));
    ctx.set_visuals(visuals);
}

/// One container split by hairlines: a row of KPI cells, each with a small note.
pub fn stat_strip(ui: &mut Ui, theme: Theme, cells: &[(&str, &str, &str, Color32)]) {
    let p = palette(theme);
    let height = 72.0;
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), height), egui::Sense::hover());
    let paint = ui.painter();
    paint.rect_filled(rect, 6.0, p.surface);
    paint.rect_stroke(rect, 6.0, Stroke::new(1.0_f32, p.border), egui::StrokeKind::Inside);
    let col = rect.width() / cells.len() as f32;
    for (i, (label, value, note, color)) in cells.iter().enumerate() {
        let x = rect.left() + col * i as f32 + 12.0;
        if i > 0 {
            let dx = rect.left() + col * i as f32;
            paint.line_segment(
                [egui::pos2(dx, rect.top() + 12.0), egui::pos2(dx, rect.bottom() - 12.0)],
                Stroke::new(1.0_f32, p.border),
            );
        }
        paint.text(egui::pos2(x, rect.top() + 12.0), egui::Align2::LEFT_TOP,
            *label, egui::FontId::proportional(11.0), p.muted);
        paint.text(egui::pos2(x, rect.top() + 26.0), egui::Align2::LEFT_TOP,
            *value, egui::FontId::proportional(20.0), *color);
        paint.text(egui::pos2(x, rect.top() + 54.0), egui::Align2::LEFT_TOP,
            *note, egui::FontId::proportional(10.0), p.muted);
    }
}

/// Restart trend: one line, quiet horizontal gridlines, round ticks, no markers.
/// Values are real restart counts per bucket from crash history.
pub fn line_chart(ui: &mut Ui, theme: Theme, values: &[f32], labels: &[String], peak_note: Option<&str>) {
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

        // Annotate the busiest bucket with a descriptive callout near the top.
        if let Some(note) = peak_note {
            if let Some(peak) = values.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()) {
                if *peak.1 > 0.0 {
                    let px = plot.left() + step * peak.0 as f32;
                    paint.text(
                        egui::pos2(px, plot.top() + 4.0),
                        egui::Align2::CENTER_TOP,
                        note,
                        egui::FontId::proportional(10.0),
                        p.accent,
                    );
                }
            }
        }
    }

    // x labels: five evenly spaced ticks like the prototype — never rotated.
    let denom = values.len().saturating_sub(1).max(1) as f32;
    let n = values.len();
    let ticks = [0usize, n / 4, n / 2, (n * 3) / 4, n.saturating_sub(1)];
    for &i in &ticks {
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
pub fn ring_chart(ui: &mut Ui, theme: Theme, ratio: f32, sub_label: &str) {
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
        center + Vec2::new(0.0, -6.0),
        egui::Align2::CENTER_CENTER,
        format!("{:.0}%", ratio * 100.0),
        egui::FontId::proportional(18.0),
        p.ink,
    );
    paint.text(
        center + Vec2::new(0.0, 11.0),
        egui::Align2::CENTER_CENTER,
        sub_label,
        egui::FontId::proportional(9.0),
        p.muted,
    );
}

/// Restart pulse: one lane per process, restart events as ticks on a 60-minute axis.
/// `lanes` is (name, minutes-ago of each restart in the last hour), newest last.
/// `axis` holds four evenly spaced clock labels, oldest first, last is "now".
pub fn pulse_chart(ui: &mut Ui, theme: Theme, lanes: &[(String, Vec<f32>)], axis: &[String; 4]) {
    let p = palette(theme);
    let row_h = 22.0;
    let label_w = 96.0;
    let height = (lanes.len() as f32 * row_h).max(row_h) + 28.0;
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), height), egui::Sense::hover());
    let paint = ui.painter();
    let track = egui::Rect::from_min_max(
        egui::pos2(rect.left() + label_w, rect.top()),
        egui::pos2(rect.right() - 4.0, rect.bottom() - 20.0),
    );
    // Time axis under the lanes: oldest .. now, evenly spaced.
    for (i, label) in axis.iter().enumerate() {
        let t = i as f32 / 3.0;
        let x = track.left() + t * track.width();
        paint.text(
            egui::pos2(x, rect.bottom() - 8.0),
            if i == 0 { egui::Align2::LEFT_BOTTOM }
            else if i == 3 { egui::Align2::RIGHT_BOTTOM }
            else { egui::Align2::CENTER_BOTTOM },
            label,
            egui::FontId::proportional(9.0),
            p.muted,
        );
    }
    for (i, (name, events)) in lanes.iter().enumerate() {
        let cy = rect.top() + i as f32 * row_h + row_h / 2.0;
        paint.text(egui::pos2(rect.left(), cy), egui::Align2::LEFT_CENTER,
            name, egui::FontId::proportional(11.0), p.muted);
        paint.line_segment(
            [egui::pos2(track.left(), cy), egui::pos2(track.right(), cy)],
            Stroke::new(1.0_f32, p.track),
        );
        for &mins_ago in events {
            if mins_ago < 0.0 || mins_ago > 60.0 { continue; }
            let x = track.right() - (mins_ago / 60.0) * track.width();
            let hot = events.iter().filter(|&&m| m < 20.0).count() >= 3;
            let color = if hot { p.red } else { p.muted };
            paint.line_segment(
                [egui::pos2(x, cy - 5.0), egui::pos2(x, cy + 5.0)],
                Stroke::new(2.0_f32, color),
            );
        }
    }
    let now_x = track.right();
    paint.line_segment(
        [egui::pos2(now_x, track.top()), egui::pos2(now_x, track.bottom())],
        Stroke::new(1.0_f32, p.accent),
    );
    paint.text(egui::pos2(now_x, rect.bottom()), egui::Align2::RIGHT_BOTTOM,
        "now", egui::FontId::proportional(9.0), p.accent);
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
