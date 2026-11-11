//! Small drawing helpers and controls, placed by absolute position.

use std::{f32::consts::TAU, sync::Arc};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, FontId, Galley, Image, ImageSource, Pos2, Rect,
    Response, Sense, Stroke, StrokeKind, TextFormat, Ui, Vec2, WidgetInfo, epaint,
    text::{LayoutJob, TextWrapping},
    vec2,
};

use crate::theme;

/// Lays out one line, shortened with an ellipsis when it does not fit in `max_width`.
pub fn line(ui: &Ui, text: &str, font: FontId, color: Color32, max_width: f32) -> Arc<Galley> {
    spaced_line(ui, text, font, color, max_width, 0.0)
}

pub fn spaced_line(
    ui: &Ui,
    text: &str,
    font: FontId,
    color: Color32,
    max_width: f32,
    letter_spacing: f32,
) -> Arc<Galley> {
    let mut job = LayoutJob::default();
    job.append(
        text,
        0.0,
        TextFormat { font_id: font, color, extra_letter_spacing: letter_spacing, ..TextFormat::default() },
    );
    job.wrap = TextWrapping::truncate_at_width(max_width);
    ui.painter().layout_job(job)
}

pub fn paint_text(ui: &Ui, anchor: Align2, position: Pos2, galley: Arc<Galley>) {
    let rect = anchor.anchor_size(position, galley.size());
    ui.painter().galley(rect.min, galley, Color32::PLACEHOLDER);
}

/// Paints a title over a subtitle, vertically centered on `center_y`.
pub fn paint_two_lines(ui: &Ui, left: f32, center_y: f32, gap: f32, top: Arc<Galley>, bottom: Arc<Galley>) {
    let first = center_y - (top.size().y + gap + bottom.size().y) / 2.0;
    let second = first + top.size().y + gap;
    ui.painter().galley(egui::pos2(left, first), top, Color32::PLACEHOLDER);
    ui.painter().galley(egui::pos2(left, second), bottom, Color32::PLACEHOLDER);
}

pub fn paint_icon(ui: &Ui, rect: Rect, icon: ImageSource<'static>, color: Color32) {
    Image::new(icon).tint(color).fit_to_exact_size(rect.size()).paint_at(ui, rect);
}

pub fn paint_texture(ui: &Ui, rect: Rect, texture: egui::TextureId, radius: impl Into<CornerRadius>) {
    Image::new((texture, rect.size())).corner_radius(radius).paint_at(ui, rect);
}

/// `offset_y` and `blur` follow the CSS `box-shadow` values of the same name.
pub fn paint_shadow(ui: &Ui, rect: Rect, radius: u8, offset_y: i8, blur: u8, alpha: u8) {
    let shadow = epaint::Shadow {
        offset: [0, offset_y],
        blur,
        spread: 0,
        color: Color32::from_black_alpha(alpha),
    };
    ui.painter().add(shadow.as_shape(rect, radius));
}

fn interact(ui: &Ui, rect: Rect, label: &str, sense: Sense) -> Response {
    ui.interact(rect, ui.scope_id().with(label), sense).on_hover_cursor(CursorIcon::PointingHand)
}

fn paint_focus(ui: &Ui, response: &Response, radius: f32) {
    if response.has_focus() {
        let stroke = Stroke::new(1.5, theme::ACCENT);
        ui.painter().rect_stroke(response.rect, radius, stroke, StrokeKind::Inside);
    }
}

/// A clickable area without its own look. The label is what screen readers announce.
pub fn click_area(ui: &Ui, rect: Rect, label: &str) -> Response {
    let response = interact(ui, rect, label, Sense::click());
    response.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, label));
    response
}

pub struct IconButton {
    pub icon: ImageSource<'static>,
    pub icon_size: f32,
    pub color: Color32,
    pub radius: f32,
}

impl IconButton {
    pub fn round(icon: ImageSource<'static>, icon_size: f32, color: Color32) -> Self {
        Self { icon, icon_size, color, radius: f32::INFINITY }
    }

    pub fn show(self, ui: &Ui, rect: Rect, label: &str) -> Response {
        let radius = self.radius.min(rect.height() / 2.0);
        let response = click_area(ui, rect, label);
        if response.hovered() {
            ui.painter().rect_filled(rect, radius, theme::ICON_HOVER);
        }
        let icon = Rect::from_center_size(rect.center(), Vec2::splat(self.icon_size));
        paint_icon(ui, icon, self.icon, self.color);
        paint_focus(ui, &response, radius);
        response.on_hover_text(label)
    }
}

/// The round accent button that plays and pauses.
pub fn accent_button(
    ui: &Ui,
    rect: Rect,
    icon: ImageSource<'static>,
    icon_size: f32,
    label: &str,
) -> Response {
    let response = click_area(ui, rect, label);
    let fill = if response.hovered() { theme::ACCENT_HOVER } else { theme::ACCENT };
    let radius = rect.height() / 2.0;
    ui.painter().rect_filled(rect, radius, fill);
    let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(icon_size));
    paint_icon(ui, icon_rect, icon, theme::ON_ACCENT);
    paint_focus(ui, &response, radius);
    response
}

/// Shows and edits a value from 0 to 1. Reports `changed` while the pointer moves it.
pub fn slider(ui: &Ui, rect: Rect, label: &str, value: &mut f32) -> Response {
    const TRACK: f32 = 4.0;
    const THUMB: f32 = 7.0;

    let mut response = interact(ui, rect, label, Sense::click_and_drag());
    let travel = (rect.width() - 2.0 * THUMB).max(1.0);
    if response.is_pointer_button_down_on()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let target = ((pointer.x - rect.left() - THUMB) / travel).clamp(0.0, 1.0);
        if target != *value {
            *value = target;
            response.mark_changed();
        }
    }
    let shown = f64::from(*value);
    response.widget_info(|| WidgetInfo::slider(true, shown, label));

    let center_y = rect.center().y;
    let thumb_x = rect.left() + THUMB + value.clamp(0.0, 1.0) * travel;
    let track = Rect::from_x_y_ranges(rect.x_range(), center_y - TRACK / 2.0..=center_y + TRACK / 2.0);
    ui.painter().rect_filled(track, TRACK / 2.0, theme::MINI_LINE);
    let filled = Rect::from_x_y_ranges(rect.left()..=thumb_x, track.y_range());
    ui.painter().rect_filled(filled, TRACK / 2.0, theme::ACCENT);
    let thumb = if response.hovered() || response.dragged() { theme::ACCENT_HOVER } else { theme::ACCENT };
    ui.painter().circle_filled(egui::pos2(thumb_x, center_y), THUMB, thumb);
    paint_focus(ui, &response, rect.height() / 2.0);
    response
}

/// Three small bars that show which playlist or row is playing.
pub fn paint_equalizer(ui: &Ui, left: f32, center_y: f32, moving: bool, color: Color32) {
    const DELAYS: [f64; 3] = [0.0, 0.2, 0.45];
    let time = ui.input(|input| input.time);
    let bottom = center_y + 5.5;
    for (index, delay) in DELAYS.into_iter().enumerate() {
        let height = if moving {
            let phase = ((time - delay) / 0.9) as f32 * TAU;
            3.0 + 8.0 * (0.5 - 0.5 * phase.cos())
        } else {
            3.0
        };
        let x = left + index as f32 * 4.0;
        let bar = Rect::from_min_max(egui::pos2(x, bottom - height), egui::pos2(x + 2.0, bottom));
        ui.painter().rect_filled(bar, 0.0, color);
    }
}

/// Fades text out toward the panel color at the top and bottom of `rect`.
pub fn paint_edge_fades(ui: &Ui, rect: Rect, fade: f32, color: Color32) {
    let clear = color.gamma_multiply(0.0);
    let mut mesh = epaint::Mesh::default();
    let mut band = |top: f32, bottom: f32, top_color: Color32, bottom_color: Color32| {
        let first = mesh.vertices.len() as u32;
        mesh.colored_vertex(egui::pos2(rect.left(), top), top_color);
        mesh.colored_vertex(egui::pos2(rect.right(), top), top_color);
        mesh.colored_vertex(egui::pos2(rect.right(), bottom), bottom_color);
        mesh.colored_vertex(egui::pos2(rect.left(), bottom), bottom_color);
        mesh.add_triangle(first, first + 1, first + 2);
        mesh.add_triangle(first, first + 2, first + 3);
    };
    band(rect.top(), rect.top() + fade, color, clear);
    band(rect.bottom() - fade, rect.bottom(), clear, color);
    ui.painter().add(mesh);
}

const MARQUEE_PAUSE: f64 = 2.5;
const MARQUEE_SPEED: f64 = 28.0;
const MARQUEE_GAP: f32 = 48.0;
const MARQUEE_FADE: f32 = 14.0;

/// How far a marquee has moved. It rests, then moves one full `cycle`, where the second copy
/// of the text lands exactly on the first, and rests again.
fn marquee_offset(elapsed: f64, cycle: f32) -> f32 {
    let moving = f64::from(cycle) / MARQUEE_SPEED;
    let phase = elapsed.max(0.0) % (MARQUEE_PAUSE + moving);
    ((phase - MARQUEE_PAUSE).max(0.0) * MARQUEE_SPEED) as f32
}

/// Fades one end of `rect` into `color`, where moving text gets cut off.
fn paint_side_fade(painter: &egui::Painter, rect: Rect, at_left: bool, color: Color32) {
    let clear = color.gamma_multiply(0.0);
    let (first, second, first_color, second_color) = if at_left {
        (rect.left(), rect.left() + MARQUEE_FADE, color, clear)
    } else {
        (rect.right() - MARQUEE_FADE, rect.right(), clear, color)
    };
    let mut mesh = epaint::Mesh::default();
    mesh.colored_vertex(egui::pos2(first, rect.top()), first_color);
    mesh.colored_vertex(egui::pos2(second, rect.top()), second_color);
    mesh.colored_vertex(egui::pos2(second, rect.bottom()), second_color);
    mesh.colored_vertex(egui::pos2(first, rect.bottom()), first_color);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(mesh);
}

pub struct MarqueeText<'a> {
    pub text: &'a str,
    pub font: FontId,
    pub color: Color32,
    /// What the text is painted on. The ends of moving text fade into it.
    pub background: Color32,
}

/// One line of text at its full length. Text that does not fit in `width` scrolls slowly
/// instead of being cut short. Returns the height of the line.
pub fn marquee(ui: &Ui, top_left: Pos2, width: f32, text: MarqueeText<'_>, elapsed: f64) -> f32 {
    let MarqueeText { text, font, color, background } = text;
    let galley = line(ui, text, font, color, f32::INFINITY);
    let size = galley.size();
    if size.x <= width {
        ui.painter().galley(top_left, galley, Color32::PLACEHOLDER);
        return size.y;
    }

    let rect = Rect::from_min_size(top_left, vec2(width, size.y));
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    let cycle = size.x + MARQUEE_GAP;
    let offset = marquee_offset(elapsed, cycle);
    painter.galley(top_left - vec2(offset, 0.0), galley.clone(), Color32::PLACEHOLDER);
    painter.galley(top_left + vec2(cycle - offset, 0.0), galley, Color32::PLACEHOLDER);
    if offset > 0.0 {
        paint_side_fade(&painter, rect, true, background);
    }
    paint_side_fade(&painter, rect, false, background);
    ui.ctx().request_repaint_after_secs(1.0 / 30.0);
    size.y
}

/// A [`marquee`] with parts that can be clicked one by one, such as each artist of a song.
/// `links` are byte ranges of the text, each with what screen readers announce for it.
/// Returns the height of the line and the index of the link clicked.
pub fn marquee_links(
    ui: &Ui,
    top_left: Pos2,
    width: f32,
    text: MarqueeText<'_>,
    elapsed: f64,
    links: &[(std::ops::Range<usize>, String)],
) -> (f32, Option<usize>) {
    let (whole, font, color) = (text.text, text.font.clone(), text.color);
    let height = marquee(ui, top_left, width, text, elapsed);
    let text_width = |text: &str| line(ui, text, font.clone(), color, f32::INFINITY).size().x;
    let full = text_width(whole);
    let visible = Rect::from_min_size(top_left, vec2(width, height));
    // The same sums as `marquee`, so the links move with the text.
    let (offset, copies) = if full <= width { (0.0, 1) } else { (marquee_offset(elapsed, full + MARQUEE_GAP), 2) };
    let mut clicked = None;
    for (index, (range, label)) in links.iter().enumerate() {
        let end = text_width(&whole[..range.end]);
        let start = end - text_width(&whole[range.clone()]);
        for copy in 0..copies {
            let left = top_left.x + copy as f32 * (full + MARQUEE_GAP) - offset;
            let rect = Rect::from_x_y_ranges(left + start..=left + end, visible.y_range()).intersect(visible);            if rect.width() < 1.0 {
                continue;
            }
            let response = ui
                .interact(rect, ui.scope_id().with((label, copy)), Sense::click())
                .on_hover_cursor(CursorIcon::PointingHand);
            response.widget_info(|| WidgetInfo::labeled(egui::Role::Link, true, label));
            if response.hovered() {
                ui.painter().hline(rect.x_range(), rect.bottom() - 1.0, Stroke::new(1.0, color));
            }
            if response.clicked() {
                clicked = Some(index);
            }
        }
    }
    (height, clicked)
}

/// Where a part of a laid-out text is, relative to the galley, with one rectangle for each
/// row the part spans. `chars` counts characters, not bytes. Text cut off by an ellipsis
/// has no place.
pub fn text_rects(galley: &Galley, chars: std::ops::Range<usize>) -> Vec<Rect> {
    let mut rects = Vec::new();
    let mut index = 0;
    // The ellipsis is the last glyph, in place of the characters cut off.
    let glyphs: usize = galley.rows.iter().map(|row| row.glyphs.len()).sum();
    let shown = if galley.elided { glyphs.saturating_sub(1) } else { glyphs };
    let mut glyph_number = 0;
    for row in &galley.rows {
        let mut rect = Rect::NOTHING;
        for glyph in &row.glyphs {
            if chars.contains(&index) && glyph_number < shown {
                rect = rect.union(glyph.logical_rect().translate(row.pos.to_vec2()));
            }
            index += 1;
            glyph_number += 1;
        }
        if row.ends_with_newline {
            index += 1;
        }
        if rect.is_positive() {
            rects.push(rect);
        }
    }
    rects
}

/// A rounded button with an icon and a word, such as Play beside a playlist's title.
/// `filled` gives it the accent color, and otherwise it has an outline.
pub fn pill_button(
    ui: &Ui,
    rect: Rect,
    icon: ImageSource<'static>,
    icon_size: f32,
    text: Arc<Galley>,
    filled: bool,
    label: &str,
) -> Response {
    let response = click_area(ui, rect, label);
    let radius = rect.height() / 2.0;
    let (fill, content) = match (filled, response.hovered()) {
        (true, false) => (theme::ACCENT, theme::ON_ACCENT),
        (true, true) => (theme::ACCENT_HOVER, theme::ON_ACCENT),
        (false, false) => (Color32::TRANSPARENT, theme::TEXT),
        (false, true) => (theme::ICON_HOVER, theme::TEXT),
    };
    let stroke = if filled { Stroke::NONE } else { Stroke::new(1.0, theme::PILL_LINE) };
    ui.painter().rect(rect, radius, fill, stroke, StrokeKind::Inside);

    let gap = 8.0;
    let width = icon_size + gap + text.size().x;
    let left = rect.center().x - width / 2.0;
    let icon_rect = Rect::from_center_size(egui::pos2(left + icon_size / 2.0, rect.center().y), Vec2::splat(icon_size));
    paint_icon(ui, icon_rect, icon, content);
    let text_pos = egui::pos2(left + icon_size + gap, rect.center().y - text.size().y / 2.0);
    ui.painter().galley(text_pos, text, content);
    paint_focus(ui, &response, radius);
    response
}

/// A rounded button with a text, filled with the accent color or outlined.
pub fn text_button(ui: &Ui, rect: Rect, text: &str, filled: bool) -> Response {
    let response = click_area(ui, rect, text);
    let (fill, color) = match (filled, response.hovered()) {
        (true, false) => (theme::ACCENT, theme::ON_ACCENT),
        (true, true) => (theme::ACCENT_HOVER, theme::ON_ACCENT),
        (false, false) => (Color32::TRANSPARENT, theme::TEXT),
        (false, true) => (theme::ICON_HOVER, theme::TEXT),
    };
    let radius = rect.height() / 2.0;
    let stroke = if filled { Stroke::NONE } else { Stroke::new(1.0, theme::PILL_LINE) };
    ui.painter().rect(rect, radius, fill, stroke, StrokeKind::Inside);
    let galley = line(ui, text, theme::inter_semibold(13.0), color, rect.width());
    paint_text(ui, Align2::CENTER_CENTER, rect.center(), galley);
    paint_focus(ui, &response, radius);
    response
}

/// The width that `text_button` needs for `text`.
pub fn button_width(ui: &Ui, text: &str) -> f32 {
    line(ui, text, theme::inter_semibold(13.0), theme::TEXT, f32::INFINITY).size().x + 32.0
}

pub fn at(origin: Pos2, x: f32, y: f32, width: f32, height: f32) -> Rect {
    Rect::from_min_size(origin + vec2(x, y), vec2(width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marquee_rests_then_moves_one_cycle_and_rests_again() {
        let cycle = 280.0;
        let moving = f64::from(cycle) / MARQUEE_SPEED;
        assert_eq!(marquee_offset(0.0, cycle), 0.0);
        assert_eq!(marquee_offset(MARQUEE_PAUSE, cycle), 0.0);
        assert!((marquee_offset(MARQUEE_PAUSE + 1.0, cycle) - 28.0).abs() < 0.01);
        assert!(marquee_offset(MARQUEE_PAUSE + moving - 0.001, cycle) <= cycle);
        assert_eq!(marquee_offset(MARQUEE_PAUSE + moving, cycle), 0.0, "the next round starts at rest");
    }
}
