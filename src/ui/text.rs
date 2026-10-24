//! Text for the toolbar and labels: ab_glyph coverage blended into a pixmap.

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use tiny_skia::{Color, Pixmap};

static FONT_BYTES: &[u8] = include_bytes!("../../assets/DejaVuSans.ttf");

pub struct Text {
    font: FontRef<'static>,
}

impl Text {
    pub fn new() -> Text {
        Text {
            font: FontRef::try_from_slice(FONT_BYTES).expect("bundled font parses"),
        }
    }

    /// Advance width of `text` at `size` pixels.
    pub fn width(&self, text: &str, size: f32) -> f32 {
        let font = self.font.as_scaled(PxScale::from(size));
        let mut width = 0.0;
        let mut prev = None;
        for c in text.chars() {
            let id = font.glyph_id(c);
            if let Some(p) = prev {
                width += font.kern(p, id);
            }
            width += font.h_advance(id);
            prev = Some(id);
        }
        width
    }

    /// Ascent above the baseline at `size`, for vertical centring.
    pub fn ascent(&self, size: f32) -> f32 {
        self.font.as_scaled(PxScale::from(size)).ascent()
    }

    /// Draws `text` with its baseline at `(x, y)` in pixmap pixels.
    pub fn draw(&self, pixmap: &mut Pixmap, text: &str, x: f32, y: f32, size: f32, color: Color) {
        let font = self.font.as_scaled(PxScale::from(size));
        let (w, h) = (pixmap.width() as i32, pixmap.height() as i32);
        let color = color.premultiply();
        let (cr, cg, cb, ca) = (color.red(), color.green(), color.blue(), color.alpha());
        let data = pixmap.data_mut();
        let mut caret = x;
        let mut prev = None;
        for c in text.chars() {
            let id = font.glyph_id(c);
            if let Some(p) = prev {
                caret += font.kern(p, id);
            }
            let glyph = id.with_scale_and_position(PxScale::from(size), ab_glyph::point(caret, y));
            caret += font.h_advance(id);
            prev = Some(id);
            let Some(outline) = self.font.outline_glyph(glyph) else {
                continue;
            };
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, coverage| {
                let px = bounds.min.x as i32 + gx as i32;
                let py = bounds.min.y as i32 + gy as i32;
                if px < 0 || py < 0 || px >= w || py >= h {
                    return;
                }
                let a = coverage.clamp(0.0, 1.0);
                let i = (py as usize * w as usize + px as usize) * 4;
                let src = [cr * a, cg * a, cb * a, ca * a];
                for (k, s) in src.iter().enumerate() {
                    let d = data[i + k] as f32 / 255.0;
                    data[i + k] =
                        ((s + d * (1.0 - src[3])) * 255.0).round().clamp(0.0, 255.0) as u8;
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_and_draws_inside_bounds() {
        let text = Text::new();
        assert!(text.width("Image", 14.0) > text.width("GIF", 14.0));
        let mut pixmap = Pixmap::new(80, 30).unwrap();
        text.draw(&mut pixmap, "Hello", 2.0, 20.0, 16.0, Color::WHITE);
        // Drawing past the edge must not panic.
        text.draw(&mut pixmap, "Clipped text", 60.0, 40.0, 16.0, Color::WHITE);
        assert!(pixmap.data().chunks(4).any(|p| p[3] > 0));
    }
}
