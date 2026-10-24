//! Painting the overlay and the recording pill into pixmaps. Layout is in
//! logical pixels local to a surface; painting multiplies by the surface scale.

use tiny_skia::{
    Color, FillRule, Paint, PathBuilder, Pixmap, PixmapPaint, Rect as SkRect, Stroke, Transform,
};

use super::text::Text;
use crate::geom::{Handle, Rect};
use crate::selection::Mode;

fn accent() -> Color {
    rgb(0x82, 0xAA, 0xFF)
}
fn panel() -> Color {
    Color::from_rgba8(0x16, 0x18, 0x1d, 0xEE)
}
fn panel_edge() -> Color {
    Color::from_rgba8(0xFF, 0xFF, 0xFF, 0x22)
}
fn text_color() -> Color {
    rgb(0xEE, 0xF0, 0xF5)
}
fn dim_text() -> Color {
    Color::from_rgba8(0xEE, 0xF0, 0xF5, 0x99)
}
fn record() -> Color {
    rgb(0xF0, 0x55, 0x5A)
}
/// How dark the area outside the selection gets.
const DIM: u8 = 0x70;

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgba8(r, g, b, 0xFF)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolbarItem {
    Mode(Mode),
    Delay,
}

/// Where the toolbar's parts sit, in logical pixels local to the surface.
pub struct Toolbar {
    pub panel: Rect,
    pub items: Vec<(ToolbarItem, Rect, String, &'static str)>,
    pub hint: (f64, f64),
}

const BUTTON_H: f64 = 32.0;
const LABEL_PX: f32 = 14.0;
const KEY_PX: f32 = 11.0;
const HINT_PX: f32 = 12.0;
pub const HINT: &str =
    "Drag a region · click a window · F whole screen · Enter capture · Esc cancel";

impl Toolbar {
    pub fn layout(text: &Text, surface_width: f64, delay_secs: u64) -> Toolbar {
        let entries = [
            (ToolbarItem::Mode(Mode::Image), "Image".to_owned(), "S"),
            (ToolbarItem::Mode(Mode::Gif), "GIF".to_owned(), "G"),
            (ToolbarItem::Mode(Mode::Video), "Video".to_owned(), "V"),
            (ToolbarItem::Delay, format!("Delay {delay_secs}s"), "D"),
        ];
        let widths: Vec<f64> = entries
            .iter()
            .map(|(_, label, key)| {
                (text.width(label, LABEL_PX) + text.width(key, KEY_PX)) as f64 + 34.0
            })
            .collect();
        let gap = 6.0;
        let pad = 6.0;
        let inner = widths.iter().sum::<f64>() + gap * (widths.len() - 1) as f64;
        let hint_w = text.width(HINT, HINT_PX) as f64 + 24.0;
        let panel_w = (inner + pad * 2.0).max(hint_w);
        let panel = Rect::new(
            ((surface_width - panel_w) / 2.0).round(),
            20.0,
            panel_w,
            BUTTON_H + pad * 2.0 + 22.0,
        );
        let mut x = panel.x + (panel_w - inner) / 2.0;
        let items = entries
            .into_iter()
            .zip(widths)
            .map(|((item, label, key), w)| {
                let r = Rect::new(x, panel.y + pad, w, BUTTON_H);
                x += w + gap;
                (item, r, label, key)
            })
            .collect();
        let hint = (
            panel.x + (panel_w - hint_w) / 2.0 + 12.0,
            panel.bottom() - 9.0,
        );
        Toolbar { panel, items, hint }
    }

    pub fn hit(&self, p: (f64, f64)) -> Option<ToolbarItem> {
        self.items
            .iter()
            .find(|(_, r, ..)| r.contains(p))
            .map(|(item, ..)| *item)
    }

    pub fn contains(&self, p: (f64, f64)) -> bool {
        self.panel.contains(p)
    }
}

/// The frozen frame and its dimmed copy, prepared once per surface.
pub struct Backdrop {
    pub bright: Pixmap,
    pub dim: Pixmap,
}

impl Backdrop {
    pub fn new(width: u32, height: u32, rgb: &[u8]) -> Backdrop {
        let mut bright = Pixmap::new(width, height).expect("non-empty frame");
        let pixels = bright.data_mut().as_chunks_mut::<4>().0;
        for (dst, src) in pixels.iter_mut().zip(rgb.as_chunks::<3>().0) {
            *dst = [src[0], src[1], src[2], 0xFF];
        }
        let mut dim = bright.clone();
        let keep = 255 - DIM as u16;
        for px in dim.data_mut().as_chunks_mut::<4>().0 {
            for c in &mut px[..3] {
                *c = (*c as u16 * keep / 255) as u8;
            }
        }
        Backdrop { bright, dim }
    }
}

pub struct OverlayView<'a> {
    /// The surface's logical origin in global coordinates.
    pub origin: (f64, f64),
    pub scale: f64,
    pub selection: Option<Rect>,
    pub adjustable: bool,
    pub hovered_window: Option<Rect>,
    pub toolbar: &'a Toolbar,
    pub mode: Mode,
    pub delay: bool,
}

fn sk_rect(r: &Rect, origin: (f64, f64), s: f64) -> Option<SkRect> {
    SkRect::from_xywh(
        ((r.x - origin.0) * s) as f32,
        ((r.y - origin.1) * s) as f32,
        (r.w * s) as f32,
        (r.h * s) as f32,
    )
}

fn paint(color: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(color);
    p.anti_alias = true;
    p
}

fn rounded(r: SkRect, radius: f32) -> Option<tiny_skia::Path> {
    let (x, y, w, h) = (r.x(), r.y(), r.width(), r.height());
    let k = radius.min(w / 2.0).min(h / 2.0);
    let c = k * 0.4477;
    let mut pb = PathBuilder::new();
    pb.move_to(x + k, y);
    pb.line_to(x + w - k, y);
    pb.cubic_to(x + w - c, y, x + w, y + c, x + w, y + k);
    pb.line_to(x + w, y + h - k);
    pb.cubic_to(x + w, y + h - c, x + w - c, y + h, x + w - k, y + h);
    pb.line_to(x + k, y + h);
    pb.cubic_to(x + c, y + h, x, y + h - c, x, y + h - k);
    pb.line_to(x, y + k);
    pb.cubic_to(x, y + c, x + c, y, x + k, y);
    pb.close();
    pb.finish()
}

pub fn overlay(target: &mut Pixmap, backdrop: &Backdrop, text: &Text, view: &OverlayView) {
    let s = view.scale;
    target.draw_pixmap(
        0,
        0,
        backdrop.dim.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );

    let outline = |target: &mut Pixmap, r: &Rect, color: Color, width: f32| {
        if let Some(rect) = sk_rect(r, view.origin, s) {
            let path = PathBuilder::from_rect(rect);
            let stroke = Stroke {
                width: width * s as f32,
                ..Stroke::default()
            };
            target.stroke_path(&path, &paint(color), &stroke, Transform::identity(), None);
        }
    };

    if let Some(win) = view.hovered_window.filter(|_| view.selection.is_none()) {
        reveal(target, backdrop, &win, view.origin, s);
        outline(target, &win, accent(), 2.0);
    }

    if let Some(sel) = view.selection {
        reveal(target, backdrop, &sel, view.origin, s);
        outline(target, &sel, accent(), 2.0);
        if view.adjustable {
            for h in Handle::CORNERS_AND_EDGES {
                let (hx, hy) = h.anchor(&sel);
                let half = 5.0;
                let r = Rect::new(hx - half, hy - half, half * 2.0, half * 2.0);
                if let Some(rect) = sk_rect(&r, view.origin, s) {
                    target.fill_rect(rect, &paint(Color::WHITE), Transform::identity(), None);
                    outline(target, &r, accent(), 1.5);
                }
            }
        }
        size_label(target, text, &sel, view);
    }

    toolbar(target, text, view);
}

/// Copies the undimmed frame back inside `r`.
fn reveal(target: &mut Pixmap, backdrop: &Backdrop, r: &Rect, origin: (f64, f64), s: f64) {
    let Some(rect) = sk_rect(r, origin, s) else {
        return;
    };
    let x0 = rect.left().max(0.0).round() as usize;
    let y0 = rect.top().max(0.0).round() as usize;
    let x1 = (rect.right().round() as usize).min(target.width() as usize);
    let y1 = (rect.bottom().round() as usize).min(target.height() as usize);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let w = target.width() as usize;
    let src = backdrop.bright.data();
    let dst = target.data_mut();
    for y in y0..y1 {
        let a = (y * w + x0) * 4;
        let b = (y * w + x1) * 4;
        dst[a..b].copy_from_slice(&src[a..b]);
    }
}

fn size_label(target: &mut Pixmap, text: &Text, sel: &Rect, view: &OverlayView) {
    let s = view.scale;
    let px = sel.to_pixels();
    let label = format!("{} × {}", px.w, px.h);
    let size = 12.0 * s as f32;
    let w = text.width(&label, size) as f64 / s + 14.0;
    let h = 22.0;
    let mut r = Rect::new(sel.x, sel.bottom() + 6.0, w, h);
    // Inside the selection's bottom edge when there's no room below it.
    let bottom = view.origin.1 + target.height() as f64 / s;
    if r.bottom() > bottom {
        r.y = sel.bottom() - h - 6.0;
        r.x += 6.0;
    }
    if let Some(rect) = sk_rect(&r, view.origin, s).and_then(|r| rounded(r, 6.0 * s as f32)) {
        target.fill_path(
            &rect,
            &paint(panel()),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    let baseline = ((r.y - view.origin.1) + h / 2.0) * s + text.ascent(size) as f64 / 2.6;
    text.draw(
        target,
        &label,
        ((r.x - view.origin.0 + 7.0) * s) as f32,
        baseline as f32,
        size,
        text_color(),
    );
}

fn toolbar(target: &mut Pixmap, text: &Text, view: &OverlayView) {
    let s = view.scale;
    let local = (0.0, 0.0);
    let bar = view.toolbar;
    if let Some(shape) = sk_rect(&bar.panel, local, s).and_then(|r| rounded(r, 12.0 * s as f32)) {
        target.fill_path(
            &shape,
            &paint(panel()),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        let stroke = Stroke {
            width: s as f32,
            ..Stroke::default()
        };
        target.stroke_path(
            &shape,
            &paint(panel_edge()),
            &stroke,
            Transform::identity(),
            None,
        );
    }
    for (item, r, label, key) in &bar.items {
        let on = match item {
            ToolbarItem::Mode(m) => *m == view.mode,
            ToolbarItem::Delay => view.delay,
        };
        if on && let Some(btn) = sk_rect(r, local, s).and_then(|r| rounded(r, 8.0 * s as f32)) {
            let fill = if *item == ToolbarItem::Mode(Mode::Image) || *item == ToolbarItem::Delay {
                accent()
            } else {
                record()
            };
            target.fill_path(
                &btn,
                &paint(fill),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        let fg = if on {
            rgb(0x10, 0x12, 0x16)
        } else {
            text_color()
        };
        let key_fg = if on {
            Color::from_rgba8(0x10, 0x12, 0x16, 0xAA)
        } else {
            dim_text()
        };
        let ls = LABEL_PX * s as f32;
        let ks = KEY_PX * s as f32;
        let baseline = ((r.y + r.h / 2.0) * s) as f32 + text.ascent(ls) / 2.6;
        let x = ((r.x + 13.0) * s) as f32;
        text.draw(target, label, x, baseline, ls, fg);
        let kx = x + text.width(label, ls) + 8.0 * s as f32;
        text.draw(target, key, kx, baseline, ks, key_fg);
    }
    let hs = HINT_PX * s as f32;
    text.draw(
        target,
        HINT,
        (bar.hint.0 * s) as f32,
        (bar.hint.1 * s) as f32,
        hs,
        dim_text(),
    );
}

/// The recording pill's logical size.
pub const PILL: (u32, u32) = (168, 40);

/// Where on the pill the stop button is, in logical pixels.
pub fn pill_stop_button() -> Rect {
    Rect::new(PILL.0 as f64 - 36.0, 6.0, 28.0, 28.0)
}

pub fn pill(target: &mut Pixmap, text: &Text, scale: f64, elapsed_secs: u64, label: &str) {
    target.fill(Color::TRANSPARENT);
    let s = scale;
    let body = Rect::new(0.0, 0.0, PILL.0 as f64, PILL.1 as f64);
    if let Some(p) = sk_rect(&body, (0.0, 0.0), s).and_then(|r| rounded(r, 20.0 * s as f32)) {
        target.fill_path(
            &p,
            &paint(panel()),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    // Blinking dot.
    if elapsed_secs.is_multiple_of(2) {
        let mut pb = PathBuilder::new();
        pb.push_circle((18.0 * s) as f32, (20.0 * s) as f32, (6.0 * s) as f32);
        if let Some(dot) = pb.finish() {
            target.fill_path(
                &dot,
                &paint(record()),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
    let size = 14.0 * s as f32;
    let clock = format!("{label} {:02}:{:02}", elapsed_secs / 60, elapsed_secs % 60);
    let baseline = (20.0 * s) as f32 + text.ascent(size) / 2.6;
    text.draw(
        target,
        &clock,
        (32.0 * s) as f32,
        baseline,
        size,
        text_color(),
    );
    let stop = pill_stop_button();
    if let Some(p) = sk_rect(&stop, (0.0, 0.0), s).and_then(|r| rounded(r, 8.0 * s as f32)) {
        target.fill_path(
            &p,
            &paint(Color::from_rgba8(0xFF, 0xFF, 0xFF, 0x1E)),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    let square = Rect::new(stop.x + 9.0, stop.y + 9.0, 10.0, 10.0);
    if let Some(r) = sk_rect(&square, (0.0, 0.0), s) {
        target.fill_rect(r, &paint(record()), Transform::identity(), None);
    }
}

/// Copies a premultiplied RGBA pixmap into a little-endian ARGB8888 shm buffer.
pub fn to_argb(pixmap: &Pixmap, canvas: &mut [u8]) {
    let pixels = pixmap.data().as_chunks::<4>().0;
    for (dst, src) in canvas.as_chunks_mut::<4>().0.iter_mut().zip(pixels) {
        *dst = [src[2], src[1], src[0], src[3]];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolbar_is_centred_and_hit_tested() {
        let text = Text::new();
        let bar = Toolbar::layout(&text, 1920.0, 3);
        let centre = bar.panel.x + bar.panel.w / 2.0;
        assert!((centre - 960.0).abs() <= 1.0);
        let (item, r, ..) = &bar.items[1];
        assert_eq!(*item, ToolbarItem::Mode(Mode::Gif));
        assert_eq!(
            bar.hit((r.x + 2.0, r.y + 2.0)),
            Some(ToolbarItem::Mode(Mode::Gif))
        );
        assert_eq!(bar.hit((5.0, 500.0)), None);
        assert!(bar.items.windows(2).all(|w| w[0].1.right() < w[1].1.x));
    }

    #[test]
    fn dimming_darkens_but_reveal_restores() {
        let rgb = vec![200u8; 4 * 4 * 3];
        let backdrop = Backdrop::new(4, 4, &rgb);
        assert!(backdrop.dim.data()[0] < 200);
        let mut target = backdrop.dim.clone();
        reveal(
            &mut target,
            &backdrop,
            &Rect::new(1.0, 1.0, 2.0, 2.0),
            (0.0, 0.0),
            1.0,
        );
        let px = |x: usize, y: usize| target.data()[(y * 4 + x) * 4];
        assert_eq!(px(1, 1), 200);
        assert!(px(0, 0) < 200);
    }

    #[test]
    fn argb_swaps_red_and_blue() {
        let mut pixmap = Pixmap::new(1, 1).unwrap();
        pixmap.data_mut().copy_from_slice(&[1, 2, 3, 255]);
        let mut canvas = [0u8; 4];
        to_argb(&pixmap, &mut canvas);
        assert_eq!(canvas, [3, 2, 1, 255]);
    }
}
