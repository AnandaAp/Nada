use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use egui::{
    Align, Color32, ColorImage, CornerRadius, CursorIcon, FontId, Frame, Id, Key, Layout, Margin,
    Order, PointerButton, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Ui, Vec2, ViewportBuilder, ViewportCommand, ViewportId,
};

use crate::media::{MediaEngine, MediaSnapshot, PlayRequest, StreamInput};
use crate::{Backend, HomeItem, StreamsResponse, Track};

// ============================================================================
// iLaras v2.0 + Glisse Liquid Glass Design Tokens
// ============================================================================

const BG_STUDIO: Color32 = Color32::from_rgb(17, 17, 16); // #111110
const SURFACE_PANEL: Color32 = Color32::from_rgb(24, 24, 32); // #181820
const SURFACE_CARD: Color32 = Color32::from_rgb(34, 34, 43); // #22222b
const TEXT_MAIN: Color32 = Color32::from_rgb(245, 245, 247); // #f5f5f7
const TEXT_MUTED: Color32 = Color32::from_rgb(172, 172, 182); // #acacb6
const TEXT_DIMMED: Color32 = Color32::from_rgb(148, 148, 158); // #94949e
const ACCENT_BRASS: Color32 = Color32::from_rgb(217, 119, 6); // #d97706
const ACCENT_GOLD: Color32 = Color32::from_rgb(245, 158, 11); // #f59e0b
const GLASS_TERRACOTTA: Color32 = Color32::from_rgb(214, 90, 56); // #d65a38
const STATUS_JADE: Color32 = Color32::from_rgb(16, 185, 129); // #10b981

// Properly premultiplied RGBA (every RGB channel <= alpha, avoiding additive white bloom)
const GLASS_BASE: Color32 = Color32::from_rgba_premultiplied(28, 28, 34, 195);
const GLASS_HOVER: Color32 = Color32::from_rgba_premultiplied(48, 48, 58, 225);
const GLASS_EDGE: Color32 = Color32::from_rgba_premultiplied(22, 22, 22, 22);
const GLASS_RIM_LIGHT: Color32 = Color32::from_rgba_premultiplied(34, 33, 32, 34);

// ============================================================================
// Crisp Vector Icons & Studio Precision Controls (Zero Missing-Glyph Tofu)
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum IconKind {
    Search,
    Clear,
    Prev,
    Play,
    Pause,
    Loading,
    Next,
    Repeat,
    RepeatOne,
    Shuffle,
    Queue,
    Pip,
    Volume,
    VolumeMuted,
}

fn paint_rounded_image(
    painter: &egui::Painter,
    rect: Rect,
    tex_id: egui::TextureId,
    radius: u8,
) {
    let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    let shape = egui::epaint::RectShape::filled(rect, CornerRadius::same(radius), Color32::WHITE)
        .with_texture(tex_id, uv);
    painter.add(Shape::Rect(shape));
}

fn paint_icon(
    ui: &Ui,
    painter: &egui::Painter,
    center: Pos2,
    size: f32,
    kind: IconKind,
    color: Color32,
) {
    let s = size * 0.5;
    let stroke = Stroke::new((size * 0.115).clamp(1.4, 2.2), color);

    match kind {
        IconKind::Search => {
            let r = s * 0.56;
            let c = Pos2::new(center.x - s * 0.16, center.y - s * 0.16);
            painter.circle_stroke(c, r, stroke);
            let dir = Vec2::new(0.707, 0.707);
            painter.line_segment([c + dir * r, center + Vec2::new(s * 0.78, s * 0.78)], stroke);
        }
        IconKind::Clear => {
            let d = s * 0.56;
            painter.line_segment(
                [Pos2::new(center.x - d, center.y - d), Pos2::new(center.x + d, center.y + d)],
                stroke,
            );
            painter.line_segment(
                [Pos2::new(center.x + d, center.y - d), Pos2::new(center.x - d, center.y + d)],
                stroke,
            );
        }
        IconKind::Play => {
            // Optically centered right-pointing triangle
            let ox = s * 0.12;
            let pts = vec![
                Pos2::new(center.x - s * 0.52 + ox, center.y - s * 0.68),
                Pos2::new(center.x + s * 0.72 + ox, center.y),
                Pos2::new(center.x - s * 0.52 + ox, center.y + s * 0.68),
            ];
            painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
        }
        IconKind::Pause => {
            let bar_w = (s * 0.36).max(2.6);
            let bar_h = s * 1.24;
            let gap = s * 0.24;
            let r = CornerRadius::same(2);
            painter.rect_filled(
                Rect::from_center_size(
                    Pos2::new(center.x - gap - bar_w * 0.5, center.y),
                    Vec2::new(bar_w, bar_h),
                ),
                r,
                color,
            );
            painter.rect_filled(
                Rect::from_center_size(
                    Pos2::new(center.x + gap + bar_w * 0.5, center.y),
                    Vec2::new(bar_w, bar_h),
                ),
                r,
                color,
            );
        }
        IconKind::Loading => {
            let t = ui.input(|i| i.time) as f32;
            ui.ctx().request_repaint();
            let r = s * 0.66;
            let segs = 14;
            let start = t * 5.0;
            let sweep = std::f32::consts::PI * 1.35;
            let pts: Vec<Pos2> = (0..=segs)
                .map(|i| {
                    let a = start + sweep * (i as f32 / segs as f32);
                    Pos2::new(center.x + a.cos() * r, center.y + a.sin() * r)
                })
                .collect();
            painter.add(Shape::line(pts, Stroke::new(2.0, color)));
        }
        IconKind::Prev => {
            let bar_w = (s * 0.24).max(2.0);
            let h = s * 1.1;
            painter.rect_filled(
                Rect::from_center_size(
                    Pos2::new(center.x - s * 0.58, center.y),
                    Vec2::new(bar_w, h),
                ),
                CornerRadius::same(1),
                color,
            );
            let pts = vec![
                Pos2::new(center.x + s * 0.62, center.y - h * 0.5),
                Pos2::new(center.x - s * 0.36, center.y),
                Pos2::new(center.x + s * 0.62, center.y + h * 0.5),
            ];
            painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
        }
        IconKind::Next => {
            let bar_w = (s * 0.24).max(2.0);
            let h = s * 1.1;
            painter.rect_filled(
                Rect::from_center_size(
                    Pos2::new(center.x + s * 0.58, center.y),
                    Vec2::new(bar_w, h),
                ),
                CornerRadius::same(1),
                color,
            );
            let pts = vec![
                Pos2::new(center.x - s * 0.62, center.y - h * 0.5),
                Pos2::new(center.x + s * 0.36, center.y),
                Pos2::new(center.x - s * 0.62, center.y + h * 0.5),
            ];
            painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
        }
        IconKind::Repeat | IconKind::RepeatOne => {
            let w = s * 0.78;
            let h = s * 0.52;
            let arr = s * 0.28;
            // Top path -> right
            painter.add(Shape::line(
                vec![
                    Pos2::new(center.x - w, center.y - h * 0.2),
                    Pos2::new(center.x - w, center.y - h),
                    Pos2::new(center.x + w, center.y - h),
                ],
                stroke,
            ));
            // Top-right arrowhead
            painter.add(Shape::line(
                vec![
                    Pos2::new(center.x + w - arr, center.y - h - arr),
                    Pos2::new(center.x + w, center.y - h),
                    Pos2::new(center.x + w - arr, center.y - h + arr),
                ],
                stroke,
            ));
            // Bottom path -> left
            painter.add(Shape::line(
                vec![
                    Pos2::new(center.x + w, center.y + h * 0.2),
                    Pos2::new(center.x + w, center.y + h),
                    Pos2::new(center.x - w, center.y + h),
                ],
                stroke,
            ));
            // Bottom-left arrowhead
            painter.add(Shape::line(
                vec![
                    Pos2::new(center.x - w + arr, center.y + h - arr),
                    Pos2::new(center.x - w, center.y + h),
                    Pos2::new(center.x - w + arr, center.y + h + arr),
                ],
                stroke,
            ));
            if kind == IconKind::RepeatOne {
                painter.text(
                    center,
                    egui::Align2::CENTER_CENTER,
                    "1",
                    FontId::proportional((size * 0.5).max(8.0)),
                    color,
                );
            }
        }
        IconKind::Shuffle => {
            let w = s * 0.78;
            let h = s * 0.52;
            let arr = s * 0.26;
            // Bottom-left to top-right
            painter.add(Shape::line(
                vec![
                    Pos2::new(center.x - w, center.y + h),
                    Pos2::new(center.x - w * 0.25, center.y + h),
                    Pos2::new(center.x + w * 0.25, center.y - h),
                    Pos2::new(center.x + w, center.y - h),
                ],
                stroke,
            ));
            // Top-left to bottom-right
            painter.add(Shape::line(
                vec![
                    Pos2::new(center.x - w, center.y - h),
                    Pos2::new(center.x - w * 0.25, center.y - h),
                    Pos2::new(center.x + w * 0.25, center.y + h),
                    Pos2::new(center.x + w, center.y + h),
                ],
                stroke,
            ));
            // Arrowheads on right
            painter.add(Shape::line(
                vec![
                    Pos2::new(center.x + w - arr, center.y - h - arr),
                    Pos2::new(center.x + w, center.y - h),
                    Pos2::new(center.x + w - arr, center.y - h + arr),
                ],
                stroke,
            ));
            painter.add(Shape::line(
                vec![
                    Pos2::new(center.x + w - arr, center.y + h - arr),
                    Pos2::new(center.x + w, center.y + h),
                    Pos2::new(center.x + w - arr, center.y + h + arr),
                ],
                stroke,
            ));
        }
        IconKind::Queue => {
            let w = s * 0.76;
            let dy = s * 0.48;
            painter.line_segment(
                [Pos2::new(center.x - w, center.y - dy), Pos2::new(center.x + w, center.y - dy)],
                stroke,
            );
            painter.line_segment(
                [Pos2::new(center.x - w, center.y), Pos2::new(center.x + w, center.y)],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(center.x - w, center.y + dy),
                    Pos2::new(center.x + w * 0.45, center.y + dy),
                ],
                stroke,
            );
        }
        IconKind::Pip => {
            let outer = Rect::from_center_size(center, Vec2::new(s * 1.75, s * 1.32));
            painter.rect_stroke(outer, CornerRadius::same(2), stroke, StrokeKind::Middle);
            let inner = Rect::from_min_max(
                Pos2::new(center.x + s * 0.05, center.y + s * 0.02),
                Pos2::new(outer.right() - s * 0.22, outer.bottom() - s * 0.20),
            );
            painter.rect_filled(inner, CornerRadius::same(1), color);
        }
        IconKind::Volume | IconKind::VolumeMuted => {
            let pts = vec![
                Pos2::new(center.x - s * 0.75, center.y - s * 0.28),
                Pos2::new(center.x - s * 0.35, center.y - s * 0.28),
                Pos2::new(center.x + s * 0.08, center.y - s * 0.65),
                Pos2::new(center.x + s * 0.08, center.y + s * 0.65),
                Pos2::new(center.x - s * 0.35, center.y + s * 0.28),
                Pos2::new(center.x - s * 0.75, center.y + s * 0.28),
            ];
            painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
            if kind == IconKind::Volume {
                let r1 = s * 0.38;
                let r2 = s * 0.66;
                for r in [r1, r2] {
                    let pts: Vec<Pos2> = (-3..=3)
                        .map(|k| {
                            let a = (k as f32 / 3.0) * 0.72;
                            Pos2::new(center.x + s * 0.14 + a.cos() * r, center.y + a.sin() * r)
                        })
                        .collect();
                    painter.add(Shape::line(pts, stroke));
                }
            } else {
                painter.line_segment(
                    [
                        Pos2::new(center.x + s * 0.28, center.y - s * 0.32),
                        Pos2::new(center.x + s * 0.80, center.y + s * 0.32),
                    ],
                    stroke,
                );
                painter.line_segment(
                    [
                        Pos2::new(center.x + s * 0.80, center.y - s * 0.32),
                        Pos2::new(center.x + s * 0.28, center.y + s * 0.32),
                    ],
                    stroke,
                );
            }
        }
    }
}

fn studio_slider(
    ui: &mut Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    value: &mut f64,
    range: std::ops::RangeInclusive<f64>,
    width: f32,
    accent: Color32,
) -> egui::Response {
    let height = 18.0;
    let (rect, mut response) =
        ui.allocate_exact_size(Vec2::new(width.max(40.0), height), Sense::click_and_drag());

    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }

    let min_v = *range.start();
    let max_v = *range.end();
    let span = (max_v - min_v).max(1e-6);

    if response.dragged() || response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            let t = ((pos.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0) as f64;
            let new_val = min_v + t * span;
            if (new_val - *value).abs() > 1e-6 {
                *value = new_val;
                response.mark_changed();
            }
        }
    }

    let t = ((*value - min_v) / span).clamp(0.0, 1.0) as f32;
    let id = ui.make_persistent_id(id_salt);
    let active_t = ui.ctx().animate_bool_with_time(
        id.with("slider_act"),
        response.hovered() || response.dragged(),
        0.12,
    );

    let track_h = 3.0 + active_t * 1.5;
    let track_rect = Rect::from_center_size(rect.center(), Vec2::new(rect.width(), track_h));
    let track_r = CornerRadius::same((track_h * 0.5).ceil() as u8);

    let painter = ui.painter();
    painter.rect_filled(
        track_rect,
        track_r,
        Color32::from_rgba_unmultiplied(255, 255, 255, 24),
    );

    let fill_w = (track_rect.width() * t).max(0.0);
    if fill_w > 0.5 {
        let fill_rect =
            Rect::from_min_size(track_rect.min, Vec2::new(fill_w, track_rect.height()));
        painter.rect_filled(fill_rect, track_r, accent);
    }

    let knob_x = track_rect.left() + fill_w;
    let knob_r = 3.6 + active_t * 2.0;
    let knob_c = Pos2::new(knob_x, track_rect.center().y);
    if active_t > 0.05 {
        painter.circle_filled(
            knob_c,
            knob_r + 3.0,
            Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), (48.0 * active_t) as u8),
        );
    }
    painter.circle_filled(knob_c, knob_r, TEXT_MAIN);
    painter.circle_stroke(knob_c, knob_r, Stroke::new(1.2, accent));

    response
}

fn paint_glisse_chrome(
    ui: &mut Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    size: Vec2,
    variant: GlisseVariant,
) -> (Rect, Color32, egui::Response) {
    let id = ui.make_persistent_id(id_salt);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());

    if !ui.is_rect_visible(rect) {
        return (rect, TEXT_MAIN, response);
    }

    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }

    let target_press = if response.is_pointer_button_down_on() {
        1.0f32
    } else {
        0.0f32
    };
    let press_t = ui
        .ctx()
        .animate_value_with_time(id.with("press"), target_press, 0.09);
    let hover_t = ui
        .ctx()
        .animate_bool_with_time(id.with("hover"), response.hovered(), 0.13);

    // Subtle Apple-style tactile press-in shrink (~scale 0.96)
    let shrink_px = 1.4 * press_t;
    let draw_rect = rect.shrink(shrink_px);
    let radius = CornerRadius::same((draw_rect.height() * 0.5).round().min(255.0) as u8);

    let (fill, fg, edge_stroke, draw_shadow, draw_inner_rim) = match variant {
        GlisseVariant::Ghost => {
            let alpha = ((hover_t * 22.0) + (press_t * 14.0)).min(40.0) as u8;
            let bg = Color32::from_rgba_unmultiplied(255, 255, 255, alpha);
            let text_col = lerp_color(TEXT_MUTED, TEXT_MAIN, (hover_t + press_t).min(1.0));
            let edge_a = (hover_t * 26.0) as u8;
            (
                bg,
                text_col,
                Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, edge_a)),
                false,
                false,
            )
        }
        GlisseVariant::Glass => {
            let bg = lerp_color(GLASS_BASE, GLASS_HOVER, (hover_t * 0.75 + press_t * 0.5).min(1.0));
            let text_col = lerp_color(TEXT_MUTED, TEXT_MAIN, hover_t);
            let edge = lerp_color(
                GLASS_EDGE,
                Color32::from_rgba_unmultiplied(255, 255, 255, 42),
                hover_t,
            );
            (bg, text_col, Stroke::new(1.0, edge), true, true)
        }
        GlisseVariant::Filled(tint) => {
            let [tr, tg, tb, _] = tint.to_srgba_unmultiplied();
            let bright = Color32::from_rgb(
                tr.saturating_add(18),
                tg.saturating_add(18),
                tb.saturating_add(12),
            );
            let dark = Color32::from_rgb(
                (tr as u16 * 88 / 100) as u8,
                (tg as u16 * 88 / 100) as u8,
                (tb as u16 * 88 / 100) as u8,
            );
            let bg = if press_t > 0.1 {
                lerp_color(tint, dark, press_t)
            } else {
                lerp_color(tint, bright, hover_t)
            };
            let fg_col = if tint == ACCENT_BRASS || tint == ACCENT_GOLD {
                BG_STUDIO
            } else {
                TEXT_MAIN
            };
            (
                bg,
                fg_col,
                Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 38)),
                true,
                true,
            )
        }
        GlisseVariant::ActiveTint(tint) => {
            let [tr, tg, tb, _] = tint.to_srgba_unmultiplied();
            let base = Color32::from_rgba_unmultiplied(tr, tg, tb, 42);
            let hov = Color32::from_rgba_unmultiplied(tr, tg, tb, 72);
            let bg = lerp_color(base, hov, hover_t);
            let border = Color32::from_rgba_unmultiplied(tr, tg, tb, 110);
            (bg, tint, Stroke::new(1.0, border), false, false)
        }
    };

    let painter = ui.painter();
    if draw_shadow {
        let shadow_rect = draw_rect.translate(Vec2::new(0.0, 1.5));
        painter.rect_filled(
            shadow_rect,
            radius,
            Color32::from_rgba_unmultiplied(0, 0, 0, 56),
        );
    }
    if fill != Color32::TRANSPARENT {
        painter.rect_filled(draw_rect, radius, fill);
    }
    if edge_stroke.color.a() > 0 {
        painter.rect_stroke(draw_rect, radius, edge_stroke, StrokeKind::Inside);
    }

    if draw_inner_rim {
        let inner_rim = draw_rect.shrink(1.0);
        if inner_rim.width() > 4.0 && inner_rim.height() > 4.0 {
            let rim_radius = CornerRadius::same((inner_rim.height() * 0.5).round().min(255.0) as u8);
            painter.rect_stroke(
                inner_rim,
                rim_radius,
                Stroke::new(0.6, GLASS_RIM_LIGHT),
                StrokeKind::Inside,
            );
        }
    }

    (draw_rect, fg, response)
}

// ============================================================================
// Glisse Liquid Glass Button (Spring Physics + Bezel Rim Highlight)
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GlisseVariant {
    Ghost,
    Glass,
    Filled(Color32),
    ActiveTint(Color32),
}

pub fn glisse_button(
    ui: &mut Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    label: &str,
    size: Vec2,
    variant: GlisseVariant,
) -> egui::Response {
    let (draw_rect, fg, response) = paint_glisse_chrome(ui, id_salt, size, variant);
    if ui.is_rect_visible(draw_rect) {
        ui.painter().text(
            draw_rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            FontId::proportional(12.0),
            fg,
        );
    }
    response
}

pub fn glisse_icon_button(
    ui: &mut Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    icon: IconKind,
    size: Vec2,
    variant: GlisseVariant,
) -> egui::Response {
    let (draw_rect, fg, response) = paint_glisse_chrome(ui, id_salt, size, variant);
    if ui.is_rect_visible(draw_rect) {
        let icon_sz = (size.x.min(size.y) * 0.44).clamp(11.0, 18.0);
        paint_icon(ui, ui.painter(), draw_rect.center(), icon_sz, icon, fg);
    }
    response
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let inv = 1.0 - t;
    let [ar, ag, ab, aa] = a.to_srgba_unmultiplied();
    let [br, bg, bb, ba] = b.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(
        (ar as f32 * inv + br as f32 * t).round() as u8,
        (ag as f32 * inv + bg as f32 * t).round() as u8,
        (ab as f32 * inv + bb as f32 * t).round() as u8,
        (aa as f32 * inv + ba as f32 * t).round() as u8,
    )
}

// ============================================================================
// Application View & Playback State
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RepeatMode {
    Off,
    All,
    One,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SearchTab {
    Tracks,
    Videos,
}

#[derive(Clone, PartialEq, Eq)]
enum ActiveView {
    Home,
    Search,
    Collection { title: String },
}

#[derive(Clone)]
struct HomeShelf {
    key: &'static str,
    title: &'static str,
    subtitle: &'static str,
    loading: bool,
    error: Option<String>,
    items: Vec<HomeItem>,
}

enum AsyncEvent {
    HomeLoaded {
        view_id: u64,
        key: &'static str,
        res: Result<Vec<HomeItem>, String>,
    },
    SearchLoaded {
        view_id: u64,
        res: Result<Vec<Track>, String>,
    },
    CollectionLoaded {
        view_id: u64,
        res: Result<Vec<Track>, String>,
    },
    StreamsResolved {
        playback_id: u64,
        start_pos: f64,
        autoplay: bool,
        res: Result<StreamsResponse, String>,
    },
    RadioLoaded {
        seed_id: String,
        res: Result<Vec<Track>, String>,
    },
    CoverLoaded {
        url: String,
        image: Option<ColorImage>,
    },
}

pub enum TrayCmd {
    ShowWindow,
    TogglePlay,
    Quit,
}

pub struct NadaApp {
    backend: Arc<Backend>,
    rt: tokio::runtime::Handle,
    media: MediaEngine,
    ev_tx: mpsc::Sender<AsyncEvent>,
    ev_rx: mpsc::Receiver<AsyncEvent>,
    tray_rx: mpsc::Receiver<TrayCmd>,
    tray_tooltip_cb: Option<Box<dyn Fn(&str) + Send>>,
    allow_quit: Arc<AtomicBool>,
    has_tray: bool,

    // Navigation & data state
    view: ActiveView,
    view_id: u64,
    search_query: String,
    search_tab: SearchTab,
    status_text: Option<String>,
    results: Vec<Track>,
    shelves: Vec<HomeShelf>,

    // Playback queue & modes
    queue: Vec<Track>,
    original_queue: Vec<Track>,
    queue_index: Option<usize>,
    repeat: RepeatMode,
    shuffle: bool,
    want_video: bool,
    pip_open: bool,
    queue_open: bool,
    volume: f32,
    seeking_val: Option<f64>,
    playback_id: u64,
    radio_fetching: bool,
    last_ended_session: u64,

    // Cached GPU textures
    covers: HashMap<String, TextureHandle>,
    pending_covers: HashSet<String>,
    video_tex: Option<TextureHandle>,
    video_blur_tex: Option<TextureHandle>,
    last_video_pts: f64,
    toast: Option<(String, Instant)>,
}

impl NadaApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        backend: Arc<Backend>,
        rt: tokio::runtime::Handle,
        tray_rx: mpsc::Receiver<TrayCmd>,
        tray_tooltip_cb: Option<Box<dyn Fn(&str) + Send>>,
        allow_quit: Arc<AtomicBool>,
        has_tray: bool,
    ) -> Self {
        configure_visuals(&cc.egui_ctx);

        let ctx_repaint = cc.egui_ctx.clone();
        let repaint_cb: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            ctx_repaint.request_repaint();
        });
        let media = MediaEngine::start(Some(repaint_cb));

        let (ev_tx, ev_rx) = mpsc::channel();

        let shelves = vec![
            HomeShelf {
                key: "indonesia",
                title: "Lagi Hits di Indonesia",
                subtitle: "Chart musik harian terpopuler",
                loading: true,
                error: None,
                items: Vec::new(),
            },
            HomeShelf {
                key: "international",
                title: "Top Global",
                subtitle: "Lagu populer mancanegara",
                loading: true,
                error: None,
                items: Vec::new(),
            },
            HomeShelf {
                key: "releases",
                title: "Rilisan Baru",
                subtitle: "Album terbaru pilihan",
                loading: true,
                error: None,
                items: Vec::new(),
            },
            HomeShelf {
                key: "community",
                title: "Playlist Komunitas",
                subtitle: "Kurasi populer pendengar",
                loading: true,
                error: None,
                items: Vec::new(),
            },
        ];

        let mut app = Self {
            backend,
            rt,
            media,
            ev_tx,
            ev_rx,
            tray_rx,
            tray_tooltip_cb,
            allow_quit,
            has_tray,
            view: ActiveView::Home,
            view_id: 1,
            search_query: String::new(),
            search_tab: SearchTab::Tracks,
            status_text: None,
            results: Vec::new(),
            shelves,
            queue: Vec::new(),
            original_queue: Vec::new(),
            queue_index: None,
            repeat: RepeatMode::Off,
            shuffle: false,
            want_video: false,
            pip_open: false,
            queue_open: false,
            volume: 0.8,
            seeking_val: None,
            playback_id: 0,
            radio_fetching: false,
            last_ended_session: 0,
            covers: HashMap::new(),
            pending_covers: HashSet::new(),
            video_tex: None,
            video_blur_tex: None,
            last_video_pts: -1.0,
            toast: None,
        };

        app.load_home(cc.egui_ctx.clone());
        app
    }

    fn show_toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now() + Duration::from_secs(3)));
    }

    fn load_home(&mut self, ctx: egui::Context) {
        self.view = ActiveView::Home;
        self.view_id = self.view_id.wrapping_add(1);
        self.status_text = None;
        let vid = self.view_id;

        for shelf in &mut self.shelves {
            shelf.loading = true;
            shelf.error = None;
            let key = shelf.key;
            let backend = self.backend.clone();
            let tx = self.ev_tx.clone();
            let ctx_cl = ctx.clone();
            self.rt.spawn(async move {
                let res = backend.home_section(key).await.map(|s| s.items);
                let _ = tx.send(AsyncEvent::HomeLoaded {
                    view_id: vid,
                    key,
                    res,
                });
                ctx_cl.request_repaint();
            });
        }
    }

    fn trigger_search(&mut self, ctx: egui::Context) {
        let q = self.search_query.trim().to_string();
        if q.is_empty() {
            return;
        }
        self.view = ActiveView::Search;
        self.view_id = self.view_id.wrapping_add(1);
        let vid = self.view_id;
        self.results.clear();
        self.status_text = Some("Mencari…".to_string());

        let kind = match self.search_tab {
            SearchTab::Tracks => "tracks",
            SearchTab::Videos => "videos",
        };
        let backend = self.backend.clone();
        let tx = self.ev_tx.clone();
        self.rt.spawn(async move {
            let res = backend.search(&q, kind).await;
            let _ = tx.send(AsyncEvent::SearchLoaded { view_id: vid, res });
            ctx.request_repaint();
        });
    }

    fn open_collection(&mut self, item: &HomeItem, ctx: egui::Context) {
        let title = item.name.clone();
        let id = item.id.clone();
        let kind = item.kind.clone();
        self.view = ActiveView::Collection {
            title: title.clone(),
        };
        self.view_id = self.view_id.wrapping_add(1);
        let vid = self.view_id;
        self.results.clear();
        self.status_text = Some("Memuat koleksi…".to_string());

        let backend = self.backend.clone();
        let tx = self.ev_tx.clone();
        self.rt.spawn(async move {
            let res = backend.collection_tracks(&id, &kind).await;
            let _ = tx.send(AsyncEvent::CollectionLoaded { view_id: vid, res });
            ctx.request_repaint();
        });
    }

    fn set_queue(&mut self, tracks: Vec<Track>, start_idx: usize) {
        self.original_queue = tracks.clone();
        if self.shuffle && tracks.len() > 1 {
            let mut next_q = Vec::with_capacity(tracks.len());
            if let Some(chosen) = tracks.get(start_idx).cloned() {
                next_q.push(chosen);
            }
            let mut rest: Vec<Track> = tracks
                .into_iter()
                .enumerate()
                .filter_map(|(i, t)| if i == start_idx { None } else { Some(t) })
                .collect();
            simple_shuffle(&mut rest);
            next_q.extend(rest);
            self.queue = next_q;
            self.queue_index = Some(0);
        } else {
            self.queue = tracks;
            self.queue_index = if self.queue.is_empty() {
                None
            } else {
                Some(start_idx.min(self.queue.len() - 1))
            };
        }
    }

    fn current_track(&self) -> Option<&Track> {
        self.queue_index.and_then(|i| self.queue.get(i))
    }

    fn start_current_track(&mut self, start_pos: f64, autoplay: bool, ctx: egui::Context) {
        let Some(track) = self.current_track().cloned() else {
            return;
        };

        self.playback_id = self.playback_id.wrapping_add(1);
        let pid = self.playback_id;
        self.last_video_pts = -1.0;
        if !self.want_video {
            self.pip_open = false;
            self.video_tex = None;
            self.video_blur_tex = None;
        }

        if let Some(cb) = &self.tray_tooltip_cb {
            let tip = if track.artist.is_empty() {
                track.name.clone()
            } else {
                format!("{} — {}", track.name, track.artist)
            };
            cb(&tip);
        }

        self.request_cover(&track.cover, &ctx);
        self.status_text = Some("Mengambil stream…".to_string());

        // Inform media engine immediately so loading state reflects right away
        self.media.stop();

        let backend = self.backend.clone();
        let tx = self.ev_tx.clone();
        let vid_id = track.id.clone();
        self.rt.spawn(async move {
            let res = backend.streams(&vid_id).await;
            let _ = tx.send(AsyncEvent::StreamsResolved {
                playback_id: pid,
                start_pos,
                autoplay,
                res,
            });
            ctx.request_repaint();
        });
    }

    fn toggle_play(&mut self, snap: &MediaSnapshot, ctx: &egui::Context) {
        if self.current_track().is_none() {
            return;
        }
        if snap.session_id == 0 && !snap.loading {
            self.start_current_track(0.0, true, ctx.clone());
            return;
        }
        self.media.set_paused(snap.playing);
    }

    fn next_track(&mut self, auto_ended: bool, ctx: &egui::Context) {
        let Some(idx) = self.queue_index else {
            return;
        };
        if self.queue.is_empty() {
            return;
        }

        if auto_ended && self.repeat == RepeatMode::One {
            self.start_current_track(0.0, true, ctx.clone());
            return;
        }

        if idx + 1 < self.queue.len() {
            self.queue_index = Some(idx + 1);
            self.start_current_track(0.0, true, ctx.clone());
            return;
        }

        if self.repeat == RepeatMode::All {
            self.queue_index = Some(0);
            self.start_current_track(0.0, true, ctx.clone());
            return;
        }

        // Queue exhausted -> fetch infinite YouTube Music radio continuation
        if !self.radio_fetching {
            if let Some(seed) = self.current_track().map(|t| t.id.clone()) {
                self.radio_fetching = true;
                self.status_text = Some("Memuat radio lanjutan…".to_string());
                let backend = self.backend.clone();
                let tx = self.ev_tx.clone();
                let ctx_cl = ctx.clone();
                self.rt.spawn(async move {
                    let res = backend.radio(&seed).await;
                    let _ = tx.send(AsyncEvent::RadioLoaded {
                        seed_id: seed,
                        res,
                    });
                    ctx_cl.request_repaint();
                });
            }
        }
    }

    fn prev_track(&mut self, snap: &MediaSnapshot, ctx: &egui::Context) {
        if snap.position_secs > 3.0 {
            self.media.seek(0.0);
            return;
        }
        if let Some(idx) = self.queue_index {
            if idx > 0 {
                self.queue_index = Some(idx - 1);
                self.start_current_track(0.0, true, ctx.clone());
            } else {
                self.media.seek(0.0);
            }
        }
    }

    fn toggle_repeat(&mut self) {
        self.repeat = match self.repeat {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        };
    }

    fn toggle_shuffle(&mut self) {
        self.shuffle = !self.shuffle;
        let Some(cur_id) = self.current_track().map(|t| t.id.clone()) else {
            return;
        };
        if self.shuffle {
            if self.original_queue.is_empty() {
                self.original_queue = self.queue.clone();
            }
            let cur_track = self.current_track().cloned();
            let mut rest: Vec<Track> = self
                .queue
                .iter()
                .filter(|t| t.id != cur_id)
                .cloned()
                .collect();
            simple_shuffle(&mut rest);
            let mut next_q = Vec::with_capacity(rest.len() + 1);
            if let Some(c) = cur_track {
                next_q.push(c);
            }
            next_q.extend(rest);
            self.queue = next_q;
            self.queue_index = Some(0);
        } else if !self.original_queue.is_empty() {
            self.queue = self.original_queue.clone();
            self.queue_index = self.queue.iter().position(|t| t.id == cur_id).or(Some(0));
        }
    }

    fn toggle_video_mode(&mut self, snap: &MediaSnapshot, ctx: &egui::Context) {
        self.want_video = !self.want_video;
        if !self.want_video {
            self.pip_open = false;
            self.video_tex = None;
            self.video_blur_tex = None;
        }
        if self.current_track().is_some() {
            let pos = snap.position_secs;
            let playing = snap.playing || snap.loading;
            self.start_current_track(pos, playing, ctx.clone());
        }
    }

    fn request_cover(&mut self, cover_opt: &Option<String>, ctx: &egui::Context) {
        let Some(url) = cover_opt.as_ref().filter(|s| !s.is_empty()) else {
            return;
        };
        if self.covers.contains_key(url) || self.pending_covers.contains(url) {
            return;
        }
        self.pending_covers.insert(url.clone());
        let url_cl = url.clone();
        let http = self.backend.bg.http_client().clone();
        let tx = self.ev_tx.clone();
        let ctx_cl = ctx.clone();
        self.rt.spawn(async move {
            let img_opt = match http.get(&url_cl).send().await {
                Ok(resp) if resp.status().is_success() => match resp.bytes().await {
                    Ok(bytes) => image::load_from_memory(&bytes).ok().map(|dyn_img| {
                        let thumb = dyn_img.thumbnail(256, 256).to_rgba8();
                        let (w, h) = thumb.dimensions();
                        ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &thumb)
                    }),
                    Err(_) => None,
                },
                _ => None,
            };
            let _ = tx.send(AsyncEvent::CoverLoaded {
                url: url_cl,
                image: img_opt,
            });
            ctx_cl.request_repaint();
        });
    }

    fn process_events(&mut self, ctx: &egui::Context, snap: &MediaSnapshot) {
        // Tray events
        while let Ok(tcmd) = self.tray_rx.try_recv() {
            match tcmd {
                TrayCmd::ShowWindow => {
                    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                }
                TrayCmd::TogglePlay => {
                    self.toggle_play(snap, ctx);
                }
                TrayCmd::Quit => {
                    self.allow_quit.store(true, Ordering::Release);
                    self.media.stop();
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }

        // Async backend events
        while let Ok(ev) = self.ev_rx.try_recv() {
            match ev {
                AsyncEvent::HomeLoaded { view_id, key, res } => {
                    if view_id != self.view_id {
                        continue;
                    }
                    if let Some(shelf) = self.shelves.iter_mut().find(|s| s.key == key) {
                        shelf.loading = false;
                        match res {
                            Ok(items) => {
                                for it in &items {
                                    // Preload visible covers lazily during render
                                    let _ = it;
                                }
                                shelf.items = items;
                                shelf.error = None;
                            }
                            Err(e) => {
                                shelf.error = Some(e);
                            }
                        }
                    }
                }
                AsyncEvent::SearchLoaded { view_id, res } => {
                    if view_id != self.view_id {
                        continue;
                    }
                    self.status_text = None;
                    match res {
                        Ok(tracks) => self.results = tracks,
                        Err(e) => self.show_toast(format!("Gagal mencari: {e}")),
                    }
                }
                AsyncEvent::CollectionLoaded { view_id, res } => {
                    if view_id != self.view_id {
                        continue;
                    }
                    self.status_text = None;
                    match res {
                        Ok(tracks) => self.results = tracks,
                        Err(e) => self.show_toast(format!("Gagal memuat koleksi: {e}")),
                    }
                }
                AsyncEvent::StreamsResolved {
                    playback_id,
                    start_pos,
                    autoplay,
                    res,
                } => {
                    if playback_id != self.playback_id {
                        continue;
                    }
                    self.status_text = None;
                    match res {
                        Ok(streams) => {
                            let audio_candidates = streams
                                .audio
                                .into_iter()
                                .map(|a| StreamInput {
                                    url: a.url,
                                    mime: a.mime,
                                })
                                .collect();
                            let muxed_candidates = streams
                                .muxed
                                .into_iter()
                                .filter(|v| v.height <= 720)
                                .map(|v| StreamInput {
                                    url: v.url,
                                    mime: v.mime,
                                })
                                .collect();
                            // Prefer 360p-480p H.264 first for fast software decode on the 430px stage,
                            // falling back to 720p when lower resolutions are unavailable.
                            let mut video_only_sorted: Vec<_> = streams
                                .video_only
                                .into_iter()
                                .filter(|v| v.height <= 720)
                                .collect();
                            video_only_sorted.sort_by_key(|v| {
                                let penalty = if v.height <= 480 { 0u32 } else { 1000u32 };
                                let pref_dist = (v.height as i32 - 360).unsigned_abs();
                                (penalty, pref_dist)
                            });
                            let video_candidates = video_only_sorted
                                .into_iter()
                                .map(|v| StreamInput {
                                    url: v.url,
                                    mime: v.mime,
                                })
                                .collect();

                            self.media.play(PlayRequest {
                                session_id: playback_id,
                                audio_candidates,
                                muxed_candidates,
                                video_candidates,
                                want_video: self.want_video,
                                start_pos_secs: start_pos,
                                autoplay,
                            });
                        }
                        Err(e) => {
                            self.show_toast(format!("Stream gagal dimuat: {e}"));
                        }
                    }
                }
                AsyncEvent::RadioLoaded { seed_id, res } => {
                    self.radio_fetching = false;
                    self.status_text = None;
                    if self.current_track().map(|t| t.id.as_str()) != Some(seed_id.as_str()) {
                        continue;
                    }
                    if let Ok(radio_tracks) = res {
                        let existing: HashSet<String> =
                            self.queue.iter().map(|t| t.id.clone()).collect();
                        let fresh: Vec<Track> = radio_tracks
                            .into_iter()
                            .filter(|t| !existing.contains(&t.id))
                            .collect();
                        if !fresh.is_empty() {
                            let next_idx = self.queue.len();
                            self.queue.extend(fresh.clone());
                            self.original_queue.extend(fresh);
                            self.queue_index = Some(next_idx);
                            self.start_current_track(0.0, true, ctx.clone());
                        }
                    }
                }
                AsyncEvent::CoverLoaded { url, image } => {
                    self.pending_covers.remove(&url);
                    if let Some(img) = image {
                        let tex =
                            ctx.load_texture(format!("cover:{url}"), img, TextureOptions::LINEAR);
                        self.covers.insert(url, tex);
                    }
                }
            }
        }

        // Upload latest video frame + tiny 20x12 GPU-blurred ambient backdrop texture
        if let Some(vf) = &snap.latest_frame {
            if (vf.pts_secs - self.last_video_pts).abs() > 0.001 {
                self.last_video_pts = vf.pts_secs;
                let color_img = ColorImage::from_rgba_unmultiplied(
                    [vf.width as usize, vf.height as usize],
                    &vf.rgba,
                );
                if let Some(tex) = &mut self.video_tex {
                    tex.set(color_img, TextureOptions::LINEAR);
                } else {
                    self.video_tex =
                        Some(ctx.load_texture("video_stage", color_img, TextureOptions::LINEAR));
                }

                let blur_img = make_ambient_blur_image(vf.width as usize, vf.height as usize, &vf.rgba);
                if let Some(btex) = &mut self.video_blur_tex {
                    btex.set(blur_img, TextureOptions::LINEAR);
                } else {
                    self.video_blur_tex =
                        Some(ctx.load_texture("video_blur", blur_img, TextureOptions::LINEAR));
                }
            }
        }

        // Auto-advance when current track finishes
        if snap.ended
            && snap.session_id == self.playback_id
            && self.playback_id != 0
            && self.last_ended_session != snap.session_id
        {
            self.last_ended_session = snap.session_id;
            self.next_track(true, ctx);
        }

        if let Some(err) = &snap.error {
            if snap.session_id == self.playback_id && self.toast.is_none() {
                self.show_toast(err.clone());
            }
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context, snap: &MediaSnapshot) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let mut do_toggle = false;
        let mut seek_delta: Option<f64> = None;
        let mut do_repeat = false;
        let mut do_shuffle = false;

        ctx.input(|i| {
            if i.key_pressed(Key::Space) {
                do_toggle = true;
            }
            if i.key_pressed(Key::ArrowRight) {
                seek_delta = Some(5.0);
            }
            if i.key_pressed(Key::ArrowLeft) {
                seek_delta = Some(-5.0);
            }
            if i.key_pressed(Key::R) {
                do_repeat = true;
            }
            if i.key_pressed(Key::S) {
                do_shuffle = true;
            }
        });

        if do_toggle {
            self.toggle_play(snap, ctx);
        }
        if let Some(d) = seek_delta {
            let max_dur = self.effective_duration(snap);
            let target = (snap.position_secs + d).clamp(0.0, max_dur.max(0.0));
            self.media.seek(target);
        }
        if do_repeat {
            self.toggle_repeat();
        }
        if do_shuffle {
            self.toggle_shuffle();
        }
    }

    fn effective_duration(&self, snap: &MediaSnapshot) -> f64 {
        if snap.duration_secs > 0.5 {
            snap.duration_secs
        } else {
            self.current_track()
                .and_then(|t| t.duration)
                .map(|d| d as f64)
                .unwrap_or(0.0)
        }
    }

    fn render_header(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        Frame::new()
            .fill(BG_STUDIO)
            .inner_margin(Margin::symmetric(14, 10))
            .show(ui, |ui| {
                // Brand row: "nada." + Glisse "Beranda" button (no STUDIO pill)
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label(
                        RichText::new("nada")
                            .size(21.0)
                            .strong()
                            .color(TEXT_MAIN),
                    );
                    ui.label(
                        RichText::new(".")
                            .size(21.0)
                            .strong()
                            .color(ACCENT_BRASS),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let home_v = if self.view == ActiveView::Home {
                            GlisseVariant::ActiveTint(ACCENT_GOLD)
                        } else {
                            GlisseVariant::Glass
                        };
                        if glisse_button(
                            ui,
                            "home_btn",
                            "Beranda",
                            Vec2::new(76.0, 28.0),
                            home_v,
                        )
                        .clicked()
                        {
                            self.load_home(ctx.clone());
                        }
                    });
                });

                ui.add_space(8.0);

                // Search bar container
                Frame::new()
                    .fill(SURFACE_PANEL)
                    .stroke(Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 22)))
                    .corner_radius(CornerRadius::same(14))
                    .inner_margin(Margin::symmetric(10, 4))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let avail_w = (ui.available_width()
                                - if self.search_query.is_empty() {
                                    32.0
                                } else {
                                    62.0
                                })
                            .max(120.0);
                            let edit = egui::TextEdit::singleline(&mut self.search_query)
                                .hint_text(
                                    RichText::new("Cari lagu atau video…").color(TEXT_DIMMED),
                                )
                                .frame(Frame::NONE)
                                .desired_width(avail_w)
                                .text_color(TEXT_MAIN);
                            let resp = ui.add(edit);
                            if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                                self.trigger_search(ctx.clone());
                            }

                            if !self.search_query.is_empty()
                                && glisse_icon_button(
                                    ui,
                                    "clear_search",
                                    IconKind::Clear,
                                    Vec2::new(26.0, 26.0),
                                    GlisseVariant::Ghost,
                                )
                                .on_hover_text("Hapus pencarian")
                                .clicked()
                            {
                                self.search_query.clear();
                            }

                            if glisse_icon_button(
                                ui,
                                "search_btn",
                                IconKind::Search,
                                Vec2::new(26.0, 26.0),
                                GlisseVariant::Ghost,
                            )
                            .on_hover_text("Cari")
                            .clicked()
                            {
                                self.trigger_search(ctx.clone());
                            }
                        });
                    });

                ui.add_space(8.0);

                // Unified Apple-style segmented Lagu / Video control capsule + sticky Refresh button
                ui.horizontal(|ui| {
                    Frame::new()
                        .fill(SURFACE_PANEL)
                        .stroke(Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 18)))
                        .corner_radius(CornerRadius::same(16))
                        .inner_margin(Margin::same(3))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 2.0;
                                let tracks_v = if self.search_tab == SearchTab::Tracks {
                                    GlisseVariant::Filled(GLASS_TERRACOTTA)
                                } else {
                                    GlisseVariant::Ghost
                                };
                                if glisse_button(
                                    ui,
                                    "tab_tracks",
                                    "Lagu",
                                    Vec2::new(64.0, 24.0),
                                    tracks_v,
                                )
                                .clicked()
                                    && self.search_tab != SearchTab::Tracks
                                {
                                    self.search_tab = SearchTab::Tracks;
                                    if !self.search_query.trim().is_empty() {
                                        self.trigger_search(ctx.clone());
                                    } else {
                                        self.load_home(ctx.clone());
                                    }
                                }

                                let videos_v = if self.search_tab == SearchTab::Videos {
                                    GlisseVariant::Filled(GLASS_TERRACOTTA)
                                } else {
                                    GlisseVariant::Ghost
                                };
                                if glisse_button(
                                    ui,
                                    "tab_videos",
                                    "Video",
                                    Vec2::new(66.0, 24.0),
                                    videos_v,
                                )
                                .clicked()
                                    && self.search_tab != SearchTab::Videos
                                {
                                    self.search_tab = SearchTab::Videos;
                                    if !self.search_query.trim().is_empty() {
                                        self.trigger_search(ctx.clone());
                                    } else {
                                        self.load_home(ctx.clone());
                                    }
                                }
                            });
                        });

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if glisse_button(
                            ui,
                            "refresh_header",
                            "Refresh",
                            Vec2::new(68.0, 28.0),
                            GlisseVariant::Glass,
                        )
                        .on_hover_text("Muat ulang")
                        .clicked()
                        {
                            if self.view == ActiveView::Search
                                && !self.search_query.trim().is_empty()
                            {
                                self.trigger_search(ctx.clone());
                            } else {
                                self.load_home(ctx.clone());
                            }
                        }
                    });
                });
            });
    }

    fn render_video_stage(&mut self, ui: &mut Ui) {
        if !self.want_video || self.pip_open {
            return;
        }
        let w = ui.available_width();
        let stage_h = (w * 9.0 / 16.0).min(242.0);
        let (stage_rect, stage_resp) =
            ui.allocate_exact_size(Vec2::new(w, stage_h), Sense::click());

        let painter = ui.painter_at(stage_rect);
        let stage_r = CornerRadius::same(12);
        painter.rect_filled(stage_rect, stage_r, Color32::from_rgba_unmultiplied(8, 8, 12, 235));

        if let Some(tex) = &self.video_tex {
            let tex_size = tex.size_vec2();
            if tex_size.x > 0.0 && tex_size.y > 0.0 {
                // 16:9 uncropped object-fit: contain with subtle rounded glass frame
                let scale = (stage_rect.width() / tex_size.x).min(stage_rect.height() / tex_size.y);
                let draw_size = tex_size * scale;
                let draw_rect = Rect::from_center_size(stage_rect.center(), draw_size);
                paint_rounded_image(&painter, draw_rect, tex.id(), 12);
            }
        } else {
            painter.text(
                stage_rect.center(),
                egui::Align2::CENTER_CENTER,
                "Memuat video…",
                FontId::proportional(12.0),
                TEXT_DIMMED,
            );
        }
        painter.rect_stroke(
            stage_rect,
            stage_r,
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 30)),
            StrokeKind::Inside,
        );

        // Right-click context menu on video stage for PiP
        stage_resp.context_menu(|ui| {
            if ui.button("Picture in Picture").clicked() {
                self.pip_open = true;
                ui.close();
            }
        });

        // Floating Glisse PiP button top-right
        let pip_rect = Rect::from_min_size(
            Pos2::new(stage_rect.right() - 42.0, stage_rect.top() + 8.0),
            Vec2::new(32.0, 32.0),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(pip_rect));
        let pip_variant = if self.pip_open {
            GlisseVariant::ActiveTint(ACCENT_BRASS)
        } else {
            GlisseVariant::Glass
        };
        if glisse_icon_button(
            &mut child,
            "stage_pip_btn",
            IconKind::Pip,
            Vec2::new(32.0, 32.0),
            pip_variant,
        )
        .on_hover_text("Picture in Picture")
        .clicked()
        {
            self.pip_open = !self.pip_open;
        }
    }

    fn render_pip_viewport(&mut self, ctx: &egui::Context, snap: &MediaSnapshot) {
        if !self.pip_open || !self.want_video {
            return;
        }

        let pip_id = ViewportId::from_hash_of("nada_pip_viewport");
        let title = self
            .current_track()
            .map(|t| format!("Nada PiP — {}", t.name))
            .unwrap_or_else(|| "Nada PiP".to_string());

        let builder = ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([360.0, 202.5])
            .with_min_inner_size([240.0, 135.0])
            .with_always_on_top();

        let tex_opt = self.video_tex.clone();
        let is_playing = snap.playing;
        let close_pip = Arc::new(AtomicBool::new(false));
        let toggle_play = Arc::new(AtomicBool::new(false));
        let close_cl = close_pip.clone();
        let toggle_cl = toggle_play.clone();

        ctx.show_viewport_immediate(pip_id, builder, |ui, _class| {
            let pip_ctx = ui.ctx().clone();
            if pip_ctx.input(|i| i.viewport().close_requested()) {
                close_cl.store(true, Ordering::Release);
            }

            egui::CentralPanel::default()
                .frame(Frame::new().fill(Color32::BLACK))
                .show(ui, |ui| {
                    let rect = ui.max_rect();
                    let _ = ui.allocate_rect(rect, Sense::hover());
                    let painter = ui.painter_at(rect);

                    if let Some(tex) = &tex_opt {
                        let ts = tex.size_vec2();
                        if ts.x > 0.0 && ts.y > 0.0 {
                            let scale = (rect.width() / ts.x).min(rect.height() / ts.y);
                            let dst = Rect::from_center_size(rect.center(), ts * scale);
                            painter.image(
                                tex.id(),
                                dst,
                                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                Color32::WHITE,
                            );
                        }
                    } else {
                        painter.text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "Picture in Picture",
                            FontId::proportional(13.0),
                            TEXT_DIMMED,
                        );
                    }

                    // Overlay PiP controls while pointer is anywhere inside the PiP rect
                    // (uses rect_contains_pointer so child buttons don't steal hover and cause flicker)
                    if ui.rect_contains_pointer(rect) {
                        let ctrl_rect = Rect::from_center_size(
                            Pos2::new(rect.center().x, rect.bottom() - 28.0),
                            Vec2::new(80.0, 36.0),
                        );
                        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(ctrl_rect));
                        child.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            let icon = if is_playing {
                                IconKind::Pause
                            } else {
                                IconKind::Play
                            };
                            if glisse_icon_button(
                                ui,
                                "pip_play_toggle",
                                icon,
                                Vec2::new(36.0, 36.0),
                                GlisseVariant::Filled(ACCENT_BRASS),
                            )
                            .clicked()
                            {
                                toggle_cl.store(true, Ordering::Release);
                            }
                            if glisse_icon_button(
                                ui,
                                "pip_exit_btn",
                                IconKind::Clear,
                                Vec2::new(36.0, 36.0),
                                GlisseVariant::Glass,
                            )
                            .on_hover_text("Tutup PiP")
                            .clicked()
                            {
                                close_cl.store(true, Ordering::Release);
                            }
                        });
                    }
                });
        });

        if toggle_play.load(Ordering::Acquire) {
            self.toggle_play(snap, ctx);
        }
        if close_pip.load(Ordering::Acquire) {
            self.pip_open = false;
        }
    }

    fn render_player_dock(&mut self, ui: &mut Ui, snap: &MediaSnapshot) {
        let ctx = ui.ctx().clone();
        let dur = self.effective_duration(snap);
        let cur = self.seeking_val.unwrap_or(snap.position_secs).clamp(0.0, dur.max(0.0));

        // Reserve background layer index so the Liquid Glass card paints cleanly behind
        // the controls with fully transparent outer margins showing scrolled content behind.
        let bg_shape_idx = ui.painter().add(Shape::Noop);
        let inner = Frame::new()
            .fill(Color32::TRANSPARENT)
            .corner_radius(CornerRadius::same(18))
            .inner_margin(Margin::symmetric(14, 11))
            .show(ui, |ui| {
                // Row 1: Progress bar
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format_time(cur))
                            .monospace()
                            .size(11.0)
                            .color(TEXT_DIMMED),
                    );
                    let slider_w = (ui.available_width() - 46.0).max(80.0);
                    let mut slider_pos = cur;
                    let max_r = if dur > 0.5 { dur } else { 1.0 };
                    let s_resp = studio_slider(
                        ui,
                        "progress_slider",
                        &mut slider_pos,
                        0.0..=max_r,
                        slider_w,
                        ACCENT_BRASS,
                    );
                    if s_resp.dragged() || s_resp.changed() {
                        self.seeking_val = Some(slider_pos);
                    }
                    if s_resp.drag_stopped()
                        || (s_resp.changed() && !ui.input(|i| i.pointer.any_down()))
                    {
                        if let Some(target) = self.seeking_val.take() {
                            self.media.seek(target);
                        }
                    }

                    ui.label(
                        RichText::new(format_time(dur))
                            .monospace()
                            .size(11.0)
                            .color(TEXT_DIMMED),
                    );
                });

                ui.add_space(7.0);

                // Row 2: Cover + Track Meta + Prev / Play / Next Glisse Buttons
                ui.horizontal(|ui| {
                    let cur_track = self.current_track().cloned();
                    if let Some(t) = &cur_track {
                        self.request_cover(&t.cover, &ctx);
                        let (cov_rect, _) =
                            ui.allocate_exact_size(Vec2::new(44.0, 44.0), Sense::hover());
                        if let Some(url) = &t.cover {
                            if let Some(tex) = self.covers.get(url) {
                                paint_rounded_image(ui.painter(), cov_rect, tex.id(), 8);
                            } else {
                                ui.painter().rect_filled(
                                    cov_rect,
                                    CornerRadius::same(8),
                                    SURFACE_CARD,
                                );
                            }
                        } else {
                            ui.painter()
                                .rect_filled(cov_rect, CornerRadius::same(8), SURFACE_CARD);
                        }
                        // Crisp glass catchlight rim around album art
                        ui.painter().rect_stroke(
                            cov_rect,
                            CornerRadius::same(8),
                            Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 42)),
                            StrokeKind::Inside,
                        );
                    }

                    let meta_w = (ui.available_width() - 128.0).max(80.0);
                    ui.allocate_ui_with_layout(
                        Vec2::new(meta_w, 44.0),
                        Layout::top_down(Align::Min),
                        |ui| {
                            if snap.loading {
                                ui.label(
                                    RichText::new("Memuat stream…")
                                        .strong()
                                        .size(13.0)
                                        .color(ACCENT_GOLD),
                                );
                                if let Some(t) = &cur_track {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&t.name).size(11.5).color(TEXT_MUTED),
                                        )
                                        .truncate(),
                                    );
                                }
                            } else if let Some(t) = &cur_track {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&t.name)
                                            .strong()
                                            .size(13.0)
                                            .color(TEXT_MAIN),
                                    )
                                    .truncate(),
                                );
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&t.artist).size(11.5).color(TEXT_MUTED),
                                    )
                                    .truncate(),
                                );
                            } else {
                                ui.label(
                                    RichText::new("Belum muter apa-apa")
                                        .strong()
                                        .size(13.0)
                                        .color(TEXT_MAIN),
                                );
                                ui.label(
                                    RichText::new("Pilih lagu untuk mulai")
                                        .size(11.5)
                                        .color(TEXT_MUTED),
                                );
                            }
                        },
                    );

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if glisse_icon_button(
                            ui,
                            "next_btn",
                            IconKind::Next,
                            Vec2::new(34.0, 34.0),
                            GlisseVariant::Ghost,
                        )
                        .on_hover_text("Berikutnya")
                        .clicked()
                        {
                            self.next_track(false, &ctx);
                        }

                        let play_icon = if snap.loading {
                            IconKind::Loading
                        } else if snap.playing {
                            IconKind::Pause
                        } else {
                            IconKind::Play
                        };
                        if glisse_icon_button(
                            ui,
                            "play_btn",
                            play_icon,
                            Vec2::new(40.0, 40.0),
                            GlisseVariant::Filled(ACCENT_BRASS),
                        )
                        .on_hover_text("Play / Pause (Space)")
                        .clicked()
                        {
                            self.toggle_play(snap, &ctx);
                        }

                        if glisse_icon_button(
                            ui,
                            "prev_btn",
                            IconKind::Prev,
                            Vec2::new(34.0, 34.0),
                            GlisseVariant::Ghost,
                        )
                        .on_hover_text("Sebelumnya")
                        .clicked()
                        {
                            self.prev_track(snap, &ctx);
                        }
                    });
                });

                ui.add_space(6.0);

                // Row 3: Repeat, Shuffle, Video Toggle, Volume Slider, Queue Button
                ui.horizontal(|ui| {
                    let rep_variant = if self.repeat != RepeatMode::Off {
                        GlisseVariant::ActiveTint(GLASS_TERRACOTTA)
                    } else {
                        GlisseVariant::Ghost
                    };
                    let rep_icon = match self.repeat {
                        RepeatMode::One => IconKind::RepeatOne,
                        _ => IconKind::Repeat,
                    };
                    if glisse_icon_button(
                        ui,
                        "repeat_btn",
                        rep_icon,
                        Vec2::new(30.0, 28.0),
                        rep_variant,
                    )
                    .on_hover_text("Ulangi (R)")
                    .clicked()
                    {
                        self.toggle_repeat();
                    }

                    let shuf_variant = if self.shuffle {
                        GlisseVariant::ActiveTint(GLASS_TERRACOTTA)
                    } else {
                        GlisseVariant::Ghost
                    };
                    if glisse_icon_button(
                        ui,
                        "shuffle_btn",
                        IconKind::Shuffle,
                        Vec2::new(30.0, 28.0),
                        shuf_variant,
                    )
                    .on_hover_text("Acak antrean (S)")
                    .clicked()
                    {
                        self.toggle_shuffle();
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let q_variant = if self.queue_open {
                            GlisseVariant::ActiveTint(ACCENT_BRASS)
                        } else {
                            GlisseVariant::Ghost
                        };
                        if glisse_icon_button(
                            ui,
                            "queue_btn",
                            IconKind::Queue,
                            Vec2::new(30.0, 28.0),
                            q_variant,
                        )
                        .on_hover_text("Antrean")
                        .clicked()
                        {
                            self.queue_open = !self.queue_open;
                        }

                        let mut vol_f64 = self.volume as f64;
                        if studio_slider(
                            ui,
                            "volume_slider",
                            &mut vol_f64,
                            0.0..=1.0,
                            72.0,
                            ACCENT_BRASS,
                        )
                        .changed()
                        {
                            self.volume = vol_f64 as f32;
                            self.media.set_volume(self.volume);
                        }

                        let vol_icon = if self.volume <= 0.001 {
                            IconKind::VolumeMuted
                        } else {
                            IconKind::Volume
                        };
                        if glisse_icon_button(
                            ui,
                            "mute_btn",
                            vol_icon,
                            Vec2::new(26.0, 26.0),
                            GlisseVariant::Ghost,
                        )
                        .on_hover_text(if self.volume <= 0.001 { "Unmute" } else { "Mute" })
                        .clicked()
                        {
                            if self.volume <= 0.001 {
                                self.volume = 0.8;
                            } else {
                                self.volume = 0.0;
                            }
                            self.media.set_volume(self.volume);
                        }

                        if self.current_track().is_some()
                            && (snap.has_video || self.want_video)
                        {
                            let vid_v = if self.want_video {
                                GlisseVariant::ActiveTint(STATUS_JADE)
                            } else {
                                GlisseVariant::Glass
                            };
                            if glisse_button(
                                ui,
                                "video_mode_btn",
                                "Video",
                                Vec2::new(52.0, 26.0),
                                vid_v,
                            )
                            .on_hover_text("Mode Video")
                            .clicked()
                            {
                                self.toggle_video_mode(snap, &ctx);
                            }
                        }
                    });
                });
                    });

                let dock_rect = inner.response.rect;
                let radius = CornerRadius::same(18);
                let mut glass_shapes: Vec<Shape> = Vec::with_capacity(6);

                // 1. Two-pass soft diffuse drop shadow grounding the floating glass dock
                glass_shapes.push(Shape::rect_filled(
                    dock_rect.expand(2.5).translate(Vec2::new(0.0, 4.0)),
                    CornerRadius::same(20),
                    Color32::from_rgba_unmultiplied(0, 0, 0, 64),
                ));
                glass_shapes.push(Shape::rect_filled(
                    dock_rect.expand(1.0).translate(Vec2::new(0.0, 2.0)),
                    CornerRadius::same(19),
                    Color32::from_rgba_unmultiplied(0, 0, 0, 96),
                ));

                // 2. Seamless frosted dark glass body: extra translucent during video mode
                //    so the live blurred ambient backdrop softly glows through and around the dock
                let dock_alpha = if self.want_video && !self.pip_open { 174 } else { 220 };
                glass_shapes.push(Shape::rect_filled(
                    dock_rect,
                    radius,
                    Color32::from_rgba_unmultiplied(20, 21, 30, dock_alpha),
                ));

                // 3. Crisp dual-layer specular glass rim + subtle inner bevel
                glass_shapes.push(Shape::rect_stroke(
                    dock_rect,
                    radius,
                    Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 38)),
                    StrokeKind::Inside,
                ));
                glass_shapes.push(Shape::rect_stroke(
                    dock_rect.shrink(1.0),
                    CornerRadius::same(17),
                    Stroke::new(0.75, Color32::from_rgba_unmultiplied(255, 255, 255, 14)),
                    StrokeKind::Inside,
                ));

                // 4. Razor-clean directional top catchlight along the upper glass edge
                glass_shapes.push(Shape::line_segment(
                    [
                        Pos2::new(dock_rect.left() + 20.0, dock_rect.top() + 1.0),
                        Pos2::new(dock_rect.right() - 20.0, dock_rect.top() + 1.0),
                    ],
                    Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 80)),
                ));

                ui.painter().set(bg_shape_idx, Shape::Vec(glass_shapes));
    }

    fn render_main_body(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();

        if let Some(st) = &self.status_text {
            Frame::new()
                .inner_margin(Margin::symmetric(14, 6))
                .show(ui, |ui| {
                    ui.label(RichText::new(st).size(12.0).color(ACCENT_GOLD));
                });
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                Frame::new()
                    .inner_margin(Margin::symmetric(14, 10))
                    .show(ui, |ui| match self.view.clone() {
                        ActiveView::Home => self.render_home_view(ui, &ctx),
                        ActiveView::Search => self.render_tracks_list(ui, &ctx, None),
                        ActiveView::Collection { title } => {
                            self.render_tracks_list(ui, &ctx, Some(&title))
                        }
                    });
                // Bottom scroll clearance so the last item can scroll clear of the floating dock
                ui.add_space(146.0);
            });
    }

    fn render_home_view(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        ui.vertical(|ui| {
            ui.label(
                RichText::new("DISCOVER")
                    .monospace()
                    .size(10.0)
                    .color(ACCENT_GOLD),
            );
            ui.label(
                RichText::new("Mau dengerin apa?")
                    .strong()
                    .size(19.0)
                    .color(TEXT_MAIN),
            );
        });

        ui.add_space(12.0);

        let mut clicked_track_action: Option<(Vec<Track>, usize, bool)> = None;
        let mut clicked_collection: Option<HomeItem> = None;
        let mut covers_to_load: Vec<Option<String>> = Vec::new();

        for shelf in &self.shelves {
            ui.label(
                RichText::new(shelf.title)
                    .strong()
                    .size(14.5)
                    .color(TEXT_MAIN),
            );
            ui.label(RichText::new(shelf.subtitle).size(11.5).color(TEXT_MUTED));
            ui.add_space(6.0);

            if shelf.loading {
                let t = ui.input(|i| i.time) as f32;
                ui.ctx().request_repaint();
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    for i in 0..3 {
                        let phase = (t * 3.2 - i as f32 * 0.65).sin() * 0.5 + 0.5;
                        let card_bg = lerp_color(
                            Color32::from_rgb(24, 25, 33),
                            Color32::from_rgb(33, 35, 46),
                            phase,
                        );
                        let bone = lerp_color(
                            Color32::from_rgb(34, 36, 48),
                            Color32::from_rgb(54, 57, 74),
                            phase,
                        );
                        let (r, _) =
                            ui.allocate_exact_size(Vec2::new(122.0, 162.0), Sense::hover());
                        let p = ui.painter();
                        p.rect_filled(r, CornerRadius::same(12), card_bg);
                        p.rect_stroke(
                            r,
                            CornerRadius::same(12),
                            Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 14)),
                            StrokeKind::Inside,
                        );
                        let img_r = Rect::from_min_size(r.min + Vec2::splat(8.0), Vec2::splat(106.0));
                        p.rect_filled(img_r, CornerRadius::same(8), bone);
                        let title_r = Rect::from_min_size(
                            Pos2::new(r.left() + 8.0, img_r.bottom() + 9.0),
                            Vec2::new(86.0, 10.0),
                        );
                        p.rect_filled(title_r, CornerRadius::same(4), bone);
                        let sub_r = Rect::from_min_size(
                            Pos2::new(r.left() + 8.0, title_r.bottom() + 6.0),
                            Vec2::new(58.0, 8.0),
                        );
                        p.rect_filled(sub_r, CornerRadius::same(4), bone);
                    }
                });
            } else if shelf.error.is_some() {
                ui.label(
                    RichText::new("Belum bisa dimuat. Coba Refresh.")
                        .size(12.0)
                        .color(TEXT_DIMMED),
                );
            } else if shelf.items.is_empty() {
                ui.label(
                    RichText::new("Belum ada rekomendasi tersedia.")
                        .size(12.0)
                        .color(TEXT_DIMMED),
                );
            } else {
                egui::ScrollArea::horizontal()
                    .id_salt(format!("shelf_{}", shelf.key))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            for (idx, item) in shelf.items.iter().enumerate() {
                                covers_to_load.push(item.cover.clone());
                                let (card_rect, resp) = ui.allocate_exact_size(
                                    Vec2::new(122.0, 162.0),
                                    Sense::click(),
                                );
                                if resp.hovered() {
                                    ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                                }
                                let bg = if resp.hovered() {
                                    Color32::from_rgb(38, 39, 50)
                                } else {
                                    Color32::from_rgb(24, 25, 33)
                                };
                                let border = if resp.hovered() {
                                    Color32::from_rgba_unmultiplied(245, 158, 11, 95)
                                } else {
                                    Color32::from_rgba_unmultiplied(255, 255, 255, 14)
                                };
                                let painter = ui.painter();
                                painter.rect_filled(card_rect, CornerRadius::same(12), bg);
                                painter.rect_stroke(
                                    card_rect,
                                    CornerRadius::same(12),
                                    Stroke::new(1.0, border),
                                    StrokeKind::Inside,
                                );

                                let img_rect = Rect::from_min_size(
                                    card_rect.min + Vec2::splat(8.0),
                                    Vec2::splat(106.0),
                                );
                                if let Some(url) = &item.cover {
                                    if let Some(tex) = self.covers.get(url) {
                                        paint_rounded_image(painter, img_rect, tex.id(), 8);
                                    } else {
                                        painter.rect_filled(
                                            img_rect,
                                            CornerRadius::same(8),
                                            SURFACE_PANEL,
                                        );
                                    }
                                } else {
                                    painter.rect_filled(
                                        img_rect,
                                        CornerRadius::same(8),
                                        SURFACE_PANEL,
                                    );
                                }

                                // Hover play affordance on card cover
                                if resp.hovered() {
                                    painter.rect_filled(
                                        img_rect,
                                        CornerRadius::same(8),
                                        Color32::from_rgba_unmultiplied(0, 0, 0, 72),
                                    );
                                    let badge_c = Pos2::new(
                                        img_rect.right() - 18.0,
                                        img_rect.bottom() - 18.0,
                                    );
                                    painter.circle_filled(badge_c, 13.0, ACCENT_BRASS);
                                    paint_icon(ui, painter, badge_c, 12.0, IconKind::Play, BG_STUDIO);
                                }

                                let text_rect = Rect::from_min_max(
                                    Pos2::new(card_rect.left() + 8.0, img_rect.bottom() + 6.0),
                                    Pos2::new(card_rect.right() - 8.0, card_rect.bottom() - 6.0),
                                );
                                let mut text_ui = ui.new_child(
                                    egui::UiBuilder::new()
                                        .max_rect(text_rect)
                                        .layout(Layout::top_down(Align::Min)),
                                );
                                text_ui.set_clip_rect(text_rect.intersect(ui.clip_rect()));
                                text_ui.spacing_mut().item_spacing.y = 1.0;
                                text_ui.add(
                                    egui::Label::new(
                                        RichText::new(&item.name)
                                            .strong()
                                            .size(12.0)
                                            .color(TEXT_MAIN),
                                    )
                                    .truncate(),
                                );
                                let sub = if item.artist.is_empty() {
                                    item.kind.as_str()
                                } else {
                                    item.artist.as_str()
                                };
                                text_ui.add(
                                    egui::Label::new(
                                        RichText::new(sub).size(10.5).color(TEXT_MUTED),
                                    )
                                    .truncate(),
                                );

                                if resp.clicked() {
                                    if item.kind == "track" {
                                        let tracks: Vec<Track> = shelf
                                            .items
                                            .iter()
                                            .filter(|i| i.kind == "track")
                                            .map(|i| Track {
                                                id: i.id.clone(),
                                                name: i.name.clone(),
                                                artist: i.artist.clone(),
                                                duration: i.duration,
                                                cover: i.cover.clone(),
                                                video: i.video,
                                            })
                                            .collect();
                                        let pos = tracks
                                            .iter()
                                            .position(|t| t.id == item.id)
                                            .unwrap_or(idx);
                                        clicked_track_action = Some((tracks, pos, item.video));
                                    } else {
                                        clicked_collection = Some(item.clone());
                                    }
                                }
                            }
                        });
                    });
            }

            ui.add_space(16.0);
        }

        for cov in covers_to_load {
            self.request_cover(&cov, ctx);
        }
        if let Some((tracks, idx, is_video)) = clicked_track_action {
            self.want_video = is_video || self.search_tab == SearchTab::Videos;
            self.set_queue(tracks, idx);
            self.start_current_track(0.0, true, ctx.clone());
        } else if let Some(col) = clicked_collection {
            self.open_collection(&col, ctx.clone());
        }
    }

    fn render_tracks_list(
        &mut self,
        ui: &mut Ui,
        ctx: &egui::Context,
        collection_title: Option<&str>,
    ) {
        if let Some(title) = collection_title {
            if glisse_button(
                ui,
                "back_home_btn",
                "← Beranda",
                Vec2::new(88.0, 28.0),
                GlisseVariant::Glass,
            )
            .clicked()
            {
                self.load_home(ctx.clone());
                return;
            }
            ui.add_space(6.0);
            ui.label(RichText::new(title).strong().size(18.0).color(TEXT_MAIN));
            ui.add_space(10.0);
        }

        if self.results.is_empty() && self.status_text.is_none() {
            ui.add_space(28.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("Belum ada lagu di sini.")
                        .size(13.0)
                        .color(TEXT_DIMMED),
                );
            });
            return;
        }

        let cur_id = self.current_track().map(|t| t.id.clone());
        let tracks_snapshot = self.results.clone();
        let mut clicked_idx: Option<usize> = None;

        for (idx, track) in tracks_snapshot.iter().enumerate() {
            self.request_cover(&track.cover, ctx);
            let is_playing = cur_id.as_deref() == Some(track.id.as_str());

            let row_w = ui.available_width();
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(row_w, 56.0), Sense::click());
            if resp.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }

            let fill = if is_playing {
                Color32::from_rgba_unmultiplied(217, 119, 6, 22)
            } else if resp.hovered() {
                Color32::from_rgba_unmultiplied(255, 255, 255, 12)
            } else {
                Color32::TRANSPARENT
            };
            let painter = ui.painter();
            painter.rect_filled(rect, CornerRadius::same(10), fill);
            if is_playing {
                let bar = Rect::from_min_max(
                    Pos2::new(rect.left() + 2.0, rect.top() + 12.0),
                    Pos2::new(rect.left() + 5.0, rect.bottom() - 12.0),
                );
                painter.rect_filled(bar, CornerRadius::same(2), ACCENT_GOLD);
            }

            let cov_rect =
                Rect::from_min_size(Pos2::new(rect.left() + 10.0, rect.top() + 7.0), Vec2::splat(42.0));
            if let Some(url) = &track.cover {
                if let Some(tex) = self.covers.get(url) {
                    paint_rounded_image(painter, cov_rect, tex.id(), 7);
                } else {
                    painter.rect_filled(cov_rect, CornerRadius::same(7), SURFACE_CARD);
                }
            } else {
                painter.rect_filled(cov_rect, CornerRadius::same(7), SURFACE_CARD);
            }
            if resp.hovered() {
                painter.rect_filled(
                    cov_rect,
                    CornerRadius::same(7),
                    Color32::from_rgba_unmultiplied(0, 0, 0, 110),
                );
                paint_icon(ui, painter, cov_rect.center(), 14.0, IconKind::Play, TEXT_MAIN);
            }

            let dur_str = track.duration.map(|d| format_time(d as f64)).unwrap_or_default();
            painter.text(
                Pos2::new(rect.right() - 10.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                dur_str,
                FontId::monospace(11.5),
                TEXT_DIMMED,
            );

            let text_rect = Rect::from_min_max(
                Pos2::new(cov_rect.right() + 10.0, rect.top() + 9.0),
                Pos2::new(rect.right() - 52.0, rect.bottom() - 7.0),
            );
            let mut text_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(text_rect)
                    .layout(Layout::top_down(Align::Min)),
            );
            text_ui.set_clip_rect(text_rect.intersect(ui.clip_rect()));
            let title_col = if is_playing { ACCENT_GOLD } else { TEXT_MAIN };
            text_ui.add(
                egui::Label::new(RichText::new(&track.name).strong().size(13.0).color(title_col))
                    .truncate(),
            );
            text_ui.add(
                egui::Label::new(RichText::new(&track.artist).size(11.5).color(TEXT_MUTED))
                    .truncate(),
            );

            if resp.clicked() {
                clicked_idx = Some(idx);
            }
            ui.add_space(2.0);
        }

        if let Some(idx) = clicked_idx {
            let is_vid = tracks_snapshot.get(idx).map(|t| t.video).unwrap_or(false);
            self.want_video = is_vid;
            self.set_queue(tracks_snapshot, idx);
            self.start_current_track(0.0, true, ctx.clone());
        }
    }

    fn render_queue_drawer(&mut self, ctx: &egui::Context) {
        if !self.queue_open {
            return;
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.queue_open = false;
            return;
        }

        let screen = ctx.content_rect();
        let drawer_w = 300.0f32.min(screen.width() - 24.0).max(220.0);
        let drawer_rect = Rect::from_min_max(
            Pos2::new(screen.right() - drawer_w - 8.0, screen.top() + 8.0),
            Pos2::new(screen.right() - 8.0, screen.bottom() - 8.0),
        );

        // Translucent dark scrim behind the sheet: click outside to dismiss
        egui::Area::new(Id::unique("queue_scrim"))
            .order(Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let (r, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
                ui.painter()
                    .rect_filled(r, CornerRadius::ZERO, Color32::from_rgba_unmultiplied(8, 8, 12, 168));
                if resp.clicked() {
                    self.queue_open = false;
                }
            });

        let queue_snapshot = self.queue.clone();
        let mut covers_needed: Vec<Option<String>> = Vec::new();
        let mut jump_to: Option<usize> = None;

        egui::Area::new(Id::unique("queue_drawer"))
            .order(Order::Foreground)
            .fixed_pos(drawer_rect.min)
            .show(ctx, |ui| {
                ui.set_min_size(drawer_rect.size());
                ui.set_max_size(drawer_rect.size());
                Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(24, 25, 34, 250))
                    .stroke(Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 32)))
                    .corner_radius(CornerRadius::same(16))
                    .inner_margin(Margin::same(14))
                    .show(ui, |ui| {
                        ui.set_min_size(drawer_rect.size() - Vec2::splat(28.0));
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 1.0;
                                ui.label(RichText::new("Antrean").strong().size(15.0).color(TEXT_MAIN));
                                ui.label(
                                    RichText::new(format!("{} lagu", queue_snapshot.len()))
                                        .size(11.0)
                                        .color(TEXT_DIMMED),
                                );
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if glisse_icon_button(
                                    ui,
                                    "queue_close_btn",
                                    IconKind::Clear,
                                    Vec2::new(28.0, 28.0),
                                    GlisseVariant::Ghost,
                                )
                                .on_hover_text("Tutup antrean (Esc)")
                                .clicked()
                                {
                                    self.queue_open = false;
                                }
                            });
                        });
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(6.0);

                        let cur_idx = self.queue_index;
                        egui::ScrollArea::vertical()
                            .id_salt("queue_scroll")
                            .max_height((drawer_rect.height() - 88.0).max(120.0))
                            .show(ui, |ui| {
                                for (i, t) in queue_snapshot.iter().enumerate() {
                                    covers_needed.push(t.cover.clone());
                                    let active = cur_idx == Some(i);
                                    let (r, resp) = ui.allocate_exact_size(
                                        Vec2::new(ui.available_width(), 48.0),
                                        Sense::click(),
                                    );
                                    if resp.hovered() {
                                        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                                    }
                                    if active || resp.hovered() {
                                        let c = if active {
                                            Color32::from_rgba_unmultiplied(217, 119, 6, 28)
                                        } else {
                                            Color32::from_rgba_unmultiplied(255, 255, 255, 14)
                                        };
                                        ui.painter().rect_filled(r, CornerRadius::same(8), c);
                                    }
                                    if active {
                                        let bar = Rect::from_min_max(
                                            Pos2::new(r.left() + 2.0, r.top() + 10.0),
                                            Pos2::new(r.left() + 5.0, r.bottom() - 10.0),
                                        );
                                        ui.painter().rect_filled(bar, CornerRadius::same(2), ACCENT_GOLD);
                                    }

                                    let cov_rect = Rect::from_min_size(
                                        Pos2::new(r.left() + 9.0, r.top() + 8.0),
                                        Vec2::splat(32.0),
                                    );
                                    if let Some(url) = &t.cover {
                                        if let Some(tex) = self.covers.get(url) {
                                            paint_rounded_image(ui.painter(), cov_rect, tex.id(), 6);
                                        } else {
                                            ui.painter().rect_filled(
                                                cov_rect,
                                                CornerRadius::same(6),
                                                SURFACE_CARD,
                                            );
                                        }
                                    } else {
                                        ui.painter().rect_filled(
                                            cov_rect,
                                            CornerRadius::same(6),
                                            SURFACE_CARD,
                                        );
                                    }

                                    let row_rect = Rect::from_min_max(
                                        Pos2::new(cov_rect.right() + 9.0, r.top() + 6.0),
                                        Pos2::new(r.right() - 8.0, r.bottom() - 6.0),
                                    );
                                    let mut row_ui = ui.new_child(
                                        egui::UiBuilder::new()
                                            .max_rect(row_rect)
                                            .layout(Layout::top_down(Align::Min)),
                                    );
                                    row_ui.set_clip_rect(row_rect.intersect(ui.clip_rect()));
                                    row_ui.add(
                                        egui::Label::new(
                                            RichText::new(&t.name)
                                                .strong()
                                                .size(12.5)
                                                .color(if active { ACCENT_GOLD } else { TEXT_MAIN }),
                                        )
                                        .truncate(),
                                    );
                                    row_ui.add(
                                        egui::Label::new(
                                            RichText::new(&t.artist).size(11.0).color(TEXT_MUTED),
                                        )
                                        .truncate(),
                                    );
                                    if resp.clicked() {
                                        jump_to = Some(i);
                                    }
                                    ui.add_space(2.0);
                                }
                            });
                    });
            });

        for c in covers_needed {
            self.request_cover(&c, ctx);
        }
        if let Some(idx) = jump_to {
            self.queue_index = Some(idx);
            self.start_current_track(0.0, true, ctx.clone());
        }
    }

    fn render_toast(&mut self, ctx: &egui::Context) {
        let Some((msg, until)) = &self.toast else {
            return;
        };
        if Instant::now() >= *until {
            self.toast = None;
            return;
        }
        let screen = ctx.content_rect();
        egui::Area::new(Id::unique("toast_overlay"))
            .order(Order::Tooltip)
            .anchor(egui::Align2::CENTER_BOTTOM, Vec2::new(0.0, -154.0))
            .constrain_to(screen)
            .show(ctx, |ui| {
                Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(34, 34, 43, 240))
                    .stroke(Stroke::new(1.0, ACCENT_BRASS))
                    .corner_radius(CornerRadius::same(99))
                    .inner_margin(Margin::symmetric(14, 7))
                    .show(ui, |ui| {
                        ui.label(RichText::new(msg).size(12.0).color(TEXT_MAIN));
                    });
            });
    }
}

impl eframe::App for NadaApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        BG_STUDIO.to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // Intercept close button -> hide to tray while keeping audio playback alive
        if ctx.input(|i| i.viewport().close_requested())
            && self.has_tray
            && !self.allow_quit.load(Ordering::Acquire)
        {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        }

        let snap = self.media.snapshot();
        self.process_events(&ctx, &snap);
        self.handle_shortcuts(&ctx, &snap);

        egui::Panel::top("nada_header")
            .frame(Frame::NONE)
            .show_separator_line(true)
            .show(ui, |ui| {
                self.render_header(ui);
            });

        let video_inline = self.want_video && !self.pip_open;

        egui::CentralPanel::default()
            .frame(Frame::new().fill(BG_STUDIO))
            .show(ui, |ui| {
                self.render_main_body(ui);

                // When video is playing inline, softly blur & frost the background area outside
                // the video stage and floating player dock using the live blurred video frame.
                if video_inline {
                    let body_rect = ui.max_rect();
                    let painter = ui.painter_at(body_rect);
                    if let Some(blur_tex) = &self.video_blur_tex {
                        painter.image(
                            blur_tex.id(),
                            body_rect,
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                            Color32::from_rgba_unmultiplied(255, 255, 255, 195),
                        );
                        painter.rect_filled(
                            body_rect,
                            CornerRadius::ZERO,
                            Color32::from_rgba_unmultiplied(10, 11, 16, 138),
                        );
                    } else {
                        painter.rect_filled(
                            body_rect,
                            CornerRadius::ZERO,
                            Color32::from_rgba_unmultiplied(10, 11, 16, 170),
                        );
                    }
                }
            });

        // Floating Video Stage + Liquid Glass Player Dock: transparent margins around both
        // let the live blurred video ambient backdrop shine through cleanly.
        let screen = ctx.content_rect();
        let dock_w = (screen.width() - 20.0).max(260.0);
        egui::Area::new(Id::unique("nada_floating_dock"))
            .order(Order::Middle)
            .anchor(egui::Align2::CENTER_BOTTOM, Vec2::new(0.0, -10.0))
            .constrain_to(screen)
            .show(&ctx, |ui| {
                ui.set_width(dock_w);
                if video_inline {
                    self.render_video_stage(ui);
                    ui.add_space(10.0);
                }
                self.render_player_dock(ui, &snap);
            });

        self.render_queue_drawer(&ctx);
        self.render_toast(&ctx);
        self.render_pip_viewport(&ctx, &snap);

        if snap.playing || snap.loading {
            ctx.request_repaint_after(Duration::from_millis(33));
        } else {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }
}

fn configure_visuals(ctx: &egui::Context) {
    // Load crisp native Windows Segoe UI font when available (falls back to egui default)
    #[cfg(windows)]
    {
        if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf") {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(
                "segoe_ui".to_owned(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "segoe_ui".to_owned());
            ctx.set_fonts(fonts);
        }
    }

    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG_STUDIO;
    visuals.window_fill = SURFACE_PANEL;
    visuals.override_text_color = Some(TEXT_MAIN);
    visuals.selection.bg_fill = ACCENT_BRASS;
    visuals.selection.stroke = Stroke::new(1.0, ACCENT_GOLD);
    visuals.widgets.inactive.bg_fill = SURFACE_CARD;
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(52, 52, 64);
    visuals.widgets.active.bg_fill = ACCENT_BRASS;
    ctx.set_visuals(visuals);
    // Prevent accidental text drag/selection on track rows, cards, and queue labels
    ctx.all_styles_mut(|s| s.interaction.selectable_labels = false);
}

fn make_ambient_blur_image(src_w: usize, src_h: usize, rgba: &[u8]) -> ColorImage {
    const BW: usize = 20;
    const BH: usize = 12;
    if src_w == 0 || src_h == 0 || rgba.len() < src_w * src_h * 4 {
        return ColorImage::new([BW, BH], vec![Color32::from_rgb(18, 19, 26); BW * BH]);
    }
    let mut grid = vec![[0f32; 3]; BW * BH];
    for by in 0..BH {
        let y0 = by * src_h / BH;
        let y1 = ((by + 1) * src_h / BH).max(y0 + 1).min(src_h);
        for bx in 0..BW {
            let x0 = bx * src_w / BW;
            let x1 = ((bx + 1) * src_w / BW).max(x0 + 1).min(src_w);
            let mut r = 0u64;
            let mut g = 0u64;
            let mut b = 0u64;
            let mut n = 0u64;
            // Sample every 4th pixel for <0.05ms CPU overhead
            let step_y = ((y1 - y0) / 4).max(1);
            let step_x = ((x1 - x0) / 4).max(1);
            let mut y = y0;
            while y < y1 {
                let mut x = x0;
                while x < x1 {
                    let idx = (y * src_w + x) * 4;
                    r += rgba[idx] as u64;
                    g += rgba[idx + 1] as u64;
                    b += rgba[idx + 2] as u64;
                    n += 1;
                    x += step_x;
                }
                y += step_y;
            }
            let denom = n.max(1) as f32;
            grid[by * BW + bx] = [r as f32 / denom, g as f32 / denom, b as f32 / denom];
        }
    }
    // Single 3x3 box blur pass over the 20x12 grid for ultra-smooth GPU bilinear ambient glow
    let mut pixels = Vec::with_capacity(BW * BH);
    for y in 0..BH {
        for x in 0..BW {
            let mut acc = [0f32; 3];
            let mut cnt = 0f32;
            for dy in -1i32..=1 {
                let ny = (y as i32 + dy).clamp(0, (BH - 1) as i32) as usize;
                for dx in -1i32..=1 {
                    let nx = (x as i32 + dx).clamp(0, (BW - 1) as i32) as usize;
                    let c = grid[ny * BW + nx];
                    acc[0] += c[0];
                    acc[1] += c[1];
                    acc[2] += c[2];
                    cnt += 1.0;
                }
            }
            pixels.push(Color32::from_rgb(
                (acc[0] / cnt).clamp(0.0, 255.0) as u8,
                (acc[1] / cnt).clamp(0.0, 255.0) as u8,
                (acc[2] / cnt).clamp(0.0, 255.0) as u8,
            ));
        }
    }
    ColorImage::new([BW, BH], pixels)
}

pub fn format_time(secs: f64) -> String {
    if !secs.is_finite() || secs <= 0.0 {
        return "0:00".to_string();
    }
    let total = secs.floor() as u64;
    let m = total / 60;
    let s = total % 60;
    format!("{m}:{s:02}")
}

fn simple_shuffle<T>(slice: &mut [T]) {
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9e3779b97f4a7c15);
    for i in (1..slice.len()).rev() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let j = (seed as usize) % (i + 1);
        slice.swap(i, j);
    }
}

// Keep unused shape/button imports warning-free
#[allow(dead_code)]
fn _touch_types(_: Shape, _: PointerButton) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_time_renders_mm_ss() {
        assert_eq!(format_time(0.0), "0:00");
        assert_eq!(format_time(64.9), "1:04");
        assert_eq!(format_time(233.0), "3:53");
    }
}
