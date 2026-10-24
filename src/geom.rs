//! Rectangles in the compositor's global logical coordinate space, and the
//! mapping between that space and an output's physical pixels.

/// An axis-aligned rectangle. `w` and `h` are never negative once built with
/// [`Rect::from_points`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    /// The rectangle spanned by two corners, in either order.
    pub fn from_points(a: (f64, f64), b: (f64, f64)) -> Self {
        Self::new(
            a.0.min(b.0),
            a.1.min(b.1),
            (a.0 - b.0).abs(),
            (a.1 - b.1).abs(),
        )
    }

    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }

    pub fn contains(&self, p: (f64, f64)) -> bool {
        p.0 >= self.x && p.0 < self.right() && p.1 >= self.y && p.1 < self.bottom()
    }

    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let r = self.right().min(other.right());
        let b = self.bottom().min(other.bottom());
        (r > x && b > y).then(|| Rect::new(x, y, r - x, b - y))
    }

    /// Moves the rectangle by `d`, keeping it inside `bounds` when it fits.
    pub fn translated_within(&self, d: (f64, f64), bounds: &Rect) -> Rect {
        let x = (self.x + d.0).clamp(bounds.x, (bounds.right() - self.w).max(bounds.x));
        let y = (self.y + d.1).clamp(bounds.y, (bounds.bottom() - self.h).max(bounds.y));
        Rect::new(x, y, self.w, self.h)
    }

    /// Snaps outward to whole logical pixels, the unit recorders and `grim -g` take.
    pub fn to_pixels(self) -> PixelRect {
        let x = self.x.floor();
        let y = self.y.floor();
        PixelRect {
            x: x as i32,
            y: y as i32,
            w: (self.right().ceil() - x).max(1.0) as u32,
            h: (self.bottom().ceil() - y).max(1.0) as u32,
        }
    }
}

/// A rectangle on the integer grid, used on the command line and for recorders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl PixelRect {
    /// The `x,y wxh` geometry slurp prints and grim and wf-recorder read.
    pub fn geometry(&self) -> String {
        format!("{},{} {}x{}", self.x, self.y, self.w, self.h)
    }

    pub fn rect(&self) -> Rect {
        Rect::new(self.x as f64, self.y as f64, self.w as f64, self.h as f64)
    }

    /// Parses `x,y,w,h` or slurp's `x,y wxh`.
    pub fn parse(s: &str) -> Option<PixelRect> {
        let parts: Vec<&str> = s.split([',', ' ', 'x']).filter(|p| !p.is_empty()).collect();
        let [x, y, w, h] = parts.as_slice() else {
            return None;
        };
        let rect = PixelRect {
            x: x.trim().parse().ok()?,
            y: y.trim().parse().ok()?,
            w: w.trim().parse().ok()?,
            h: h.trim().parse().ok()?,
        };
        (rect.w > 0 && rect.h > 0).then_some(rect)
    }
}

/// Where an output sits: its logical rectangle and the physical size of its frames.
#[derive(Clone, Debug, PartialEq)]
pub struct OutputGeometry {
    pub name: String,
    pub logical: Rect,
    pub physical: (u32, u32),
}

impl OutputGeometry {
    /// Physical pixels per logical pixel, from the frame itself, so fractional
    /// scales map exactly whatever the compositor rounded.
    pub fn scale(&self) -> (f64, f64) {
        (
            self.physical.0 as f64 / self.logical.w,
            self.physical.1 as f64 / self.logical.h,
        )
    }

    /// A global logical rectangle as physical pixels of this output's frame,
    /// clipped to the frame.
    pub fn to_physical(&self, r: &Rect) -> Option<(u32, u32, u32, u32)> {
        let r = r.intersect(&self.logical)?;
        let (sx, sy) = self.scale();
        let x0 = ((r.x - self.logical.x) * sx).round().max(0.0) as u32;
        let y0 = ((r.y - self.logical.y) * sy).round().max(0.0) as u32;
        let x1 = (((r.right() - self.logical.x) * sx).round() as u32).min(self.physical.0);
        let y1 = (((r.bottom() - self.logical.y) * sy).round() as u32).min(self.physical.1);
        (x1 > x0 && y1 > y0).then_some((x0, y0, x1 - x0, y1 - y0))
    }

    /// A point local to this output's surface, as a global logical point.
    pub fn to_global(&self, local: (f64, f64)) -> (f64, f64) {
        (self.logical.x + local.0, self.logical.y + local.1)
    }
}

/// A grab point on a selection: corners, edges, or the body (to move it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
    Body,
}

impl Handle {
    pub const CORNERS_AND_EDGES: [Handle; 8] = [
        Handle::TopLeft,
        Handle::Top,
        Handle::TopRight,
        Handle::Right,
        Handle::BottomRight,
        Handle::Bottom,
        Handle::BottomLeft,
        Handle::Left,
    ];

    /// The handle's centre on `r`.
    pub fn anchor(self, r: &Rect) -> (f64, f64) {
        let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        match self {
            Handle::TopLeft => (r.x, r.y),
            Handle::Top => (cx, r.y),
            Handle::TopRight => (r.right(), r.y),
            Handle::Right => (r.right(), cy),
            Handle::BottomRight => (r.right(), r.bottom()),
            Handle::Bottom => (cx, r.bottom()),
            Handle::BottomLeft => (r.x, r.bottom()),
            Handle::Left => (r.x, cy),
            Handle::Body => (cx, cy),
        }
    }

    /// The handle under `p`, within `radius` logical pixels, else the body.
    pub fn hit(r: &Rect, p: (f64, f64), radius: f64) -> Option<Handle> {
        let near = |h: Handle| {
            let a = h.anchor(r);
            (a.0 - p.0).abs() <= radius && (a.1 - p.1).abs() <= radius
        };
        Handle::CORNERS_AND_EDGES
            .into_iter()
            .find(|h| near(*h))
            .or_else(|| r.contains(p).then_some(Handle::Body))
    }

    /// `start` with this handle dragged by `d`, kept inside `bounds`.
    pub fn drag(self, start: &Rect, d: (f64, f64), bounds: &Rect) -> Rect {
        if self == Handle::Body {
            return start.translated_within(d, bounds);
        }
        let (mut l, mut t, mut r, mut b) = (start.x, start.y, start.right(), start.bottom());
        match self {
            Handle::TopLeft | Handle::Left | Handle::BottomLeft => l += d.0,
            Handle::TopRight | Handle::Right | Handle::BottomRight => r += d.0,
            _ => {}
        }
        match self {
            Handle::TopLeft | Handle::Top | Handle::TopRight => t += d.1,
            Handle::BottomLeft | Handle::Bottom | Handle::BottomRight => b += d.1,
            _ => {}
        }
        let clamp_x = |v: f64| v.clamp(bounds.x, bounds.right());
        let clamp_y = |v: f64| v.clamp(bounds.y, bounds.bottom());
        Rect::from_points((clamp_x(l), clamp_y(t)), (clamp_x(r), clamp_y(b)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(scale: f64) -> OutputGeometry {
        OutputGeometry {
            name: "DP-1".into(),
            logical: Rect::new(1000.0, 500.0, 1280.0, 800.0),
            physical: (
                (1280.0 * scale).round() as u32,
                (800.0 * scale).round() as u32,
            ),
        }
    }

    #[test]
    fn from_points_is_order_independent() {
        let a = Rect::from_points((10.0, 20.0), (4.0, 2.0));
        assert_eq!(a, Rect::new(4.0, 2.0, 6.0, 18.0));
        assert_eq!(a, Rect::from_points((4.0, 2.0), (10.0, 20.0)));
    }

    #[test]
    fn integer_scale_maps_to_physical_pixels() {
        let o = output(2.0);
        let r = Rect::new(1010.0, 520.0, 100.0, 50.0);
        assert_eq!(o.to_physical(&r), Some((20, 40, 200, 100)));
    }

    #[test]
    fn fractional_scale_maps_exactly_and_clips() {
        let o = output(1.25);
        assert_eq!(o.physical, (1600, 1000));
        let r = Rect::new(1000.0, 500.0, 80.0, 40.0);
        assert_eq!(o.to_physical(&r), Some((0, 0, 100, 50)));
        // Spills past the right edge: clipped to the frame.
        let spill = Rect::new(2200.0, 500.0, 200.0, 10.0);
        assert_eq!(o.to_physical(&spill), Some((1500, 0, 100, 13)));
        // Entirely on another output.
        assert_eq!(o.to_physical(&Rect::new(0.0, 0.0, 10.0, 10.0)), None);
    }

    #[test]
    fn local_points_become_global() {
        assert_eq!(output(1.0).to_global((5.0, 7.0)), (1005.0, 507.0));
    }

    #[test]
    fn to_pixels_snaps_outward() {
        let p = Rect::new(10.4, 20.6, 5.2, 3.1).to_pixels();
        assert_eq!(
            p,
            PixelRect {
                x: 10,
                y: 20,
                w: 6,
                h: 4
            }
        );
        assert_eq!(p.geometry(), "10,20 6x4");
    }

    #[test]
    fn parses_both_geometry_forms() {
        let want = Some(PixelRect {
            x: 8934,
            y: 6682,
            w: 300,
            h: 200,
        });
        assert_eq!(PixelRect::parse("8934,6682,300,200"), want);
        assert_eq!(PixelRect::parse("8934,6682 300x200"), want);
        assert_eq!(PixelRect::parse("1,2,0,4"), None);
        assert_eq!(PixelRect::parse("1,2,3"), None);
    }

    #[test]
    fn handles_hit_corners_before_body() {
        let r = Rect::new(100.0, 100.0, 200.0, 100.0);
        assert_eq!(Handle::hit(&r, (102.0, 98.0), 6.0), Some(Handle::TopLeft));
        assert_eq!(Handle::hit(&r, (200.0, 199.0), 6.0), Some(Handle::Bottom));
        assert_eq!(Handle::hit(&r, (150.0, 150.0), 6.0), Some(Handle::Body));
        assert_eq!(Handle::hit(&r, (10.0, 10.0), 6.0), None);
    }

    #[test]
    fn dragging_an_edge_past_its_opposite_flips_cleanly() {
        let bounds = Rect::new(0.0, 0.0, 1000.0, 1000.0);
        let r = Rect::new(100.0, 100.0, 50.0, 50.0);
        let grown = Handle::BottomRight.drag(&r, (10.0, 20.0), &bounds);
        assert_eq!(grown, Rect::new(100.0, 100.0, 60.0, 70.0));
        let flipped = Handle::Right.drag(&r, (-80.0, 0.0), &bounds);
        assert_eq!(flipped, Rect::new(70.0, 100.0, 30.0, 50.0));
    }

    #[test]
    fn dragging_stays_inside_the_output() {
        let bounds = Rect::new(0.0, 0.0, 300.0, 300.0);
        let r = Rect::new(250.0, 250.0, 40.0, 40.0);
        assert_eq!(
            Handle::Body.drag(&r, (100.0, 100.0), &bounds),
            Rect::new(260.0, 260.0, 40.0, 40.0)
        );
        assert_eq!(
            Handle::BottomRight.drag(&r, (500.0, 500.0), &bounds),
            Rect::new(250.0, 250.0, 50.0, 50.0)
        );
    }
}
