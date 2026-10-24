//! What the overlay is selecting, independent of Wayland: drag a region, click
//! a window, take a whole output, adjust with handles, confirm or cancel.

use crate::geom::{Handle, Rect};

/// Movement under this many logical pixels between press and release is a click.
const CLICK_SLOP: f64 = 4.0;
/// How close (logical pixels) the pointer must be to grab a handle.
pub const HANDLE_RADIUS: f64 = 8.0;
/// Two clicks within this many milliseconds are a double-click.
const DOUBLE_CLICK_MS: u32 = 400;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Image,
    Gif,
    Video,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Image => "Image",
            Mode::Gif => "GIF",
            Mode::Video => "Video",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Idle,
    /// Button held; `anchor` is where it went down.
    Pressing {
        output: usize,
        anchor: (f64, f64),
        current: (f64, f64),
    },
    Selected {
        output: usize,
        rect: Rect,
    },
    Adjusting {
        output: usize,
        handle: Handle,
        start: Rect,
        from: (f64, f64),
        current: Rect,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    /// Nothing visible changed.
    Unchanged,
    Redraw,
    Confirm {
        rect: Rect,
        output: usize,
        mode: Mode,
        delay: bool,
    },
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Image,
    Gif,
    Video,
    Delay,
    FullOutput,
    Confirm,
    Cancel,
}

#[derive(Debug)]
pub struct Selector {
    outputs: Vec<Rect>,
    /// Visible windows, topmost first, in global logical coordinates.
    windows: Vec<Rect>,
    pub mode: Mode,
    pub delay: bool,
    phase: Phase,
    pointer: Option<(usize, (f64, f64))>,
    last_click: Option<(u32, (f64, f64))>,
}

impl Selector {
    pub fn new(outputs: Vec<Rect>, windows: Vec<Rect>, mode: Mode) -> Self {
        Self {
            outputs,
            windows,
            mode,
            delay: false,
            phase: Phase::Idle,
            pointer: None,
            last_click: None,
        }
    }

    /// The rectangle to draw as the selection, if any.
    pub fn selection(&self) -> Option<Rect> {
        match self.phase {
            Phase::Idle => None,
            Phase::Pressing {
                anchor, current, ..
            } => {
                let r = Rect::from_points(anchor, current);
                (r.w >= CLICK_SLOP || r.h >= CLICK_SLOP).then_some(r)
            }
            Phase::Selected { rect, .. } | Phase::Adjusting { current: rect, .. } => Some(rect),
        }
    }

    /// Whether the selection has handles to adjust (it is settled, not being drawn).
    pub fn adjustable(&self) -> bool {
        matches!(self.phase, Phase::Selected { .. } | Phase::Adjusting { .. })
    }

    /// The window that a click would take, shown while nothing is selected.
    pub fn hovered_window(&self) -> Option<Rect> {
        match self.phase {
            Phase::Idle => {
                let (output, p) = self.pointer?;
                self.window_at(output, p)
            }
            _ => None,
        }
    }

    fn window_at(&self, output: usize, p: (f64, f64)) -> Option<Rect> {
        let bounds = self.outputs.get(output)?;
        self.windows
            .iter()
            .find(|w| w.contains(p))
            .and_then(|w| w.intersect(bounds))
    }

    pub fn motion(&mut self, output: usize, p: (f64, f64)) -> Outcome {
        let before = (self.phase, self.hovered_window());
        self.pointer = Some((output, p));
        match &mut self.phase {
            Phase::Pressing {
                output: o, current, ..
            } => {
                let bounds = self.outputs[*o];
                *current = (
                    p.0.clamp(bounds.x, bounds.right()),
                    p.1.clamp(bounds.y, bounds.bottom()),
                );
            }
            Phase::Adjusting {
                output: o,
                handle,
                start,
                from,
                current,
            } => {
                let d = (p.0 - from.0, p.1 - from.1);
                *current = handle.drag(start, d, &self.outputs[*o]);
            }
            Phase::Idle | Phase::Selected { .. } => {}
        }
        if (self.phase, self.hovered_window()) == before {
            Outcome::Unchanged
        } else {
            Outcome::Redraw
        }
    }

    pub fn press(&mut self, output: usize, p: (f64, f64)) -> Outcome {
        self.pointer = Some((output, p));
        if let Phase::Selected { output: o, rect } = self.phase
            && let Some(handle) = Handle::hit(&rect, p, HANDLE_RADIUS).filter(|_| o == output)
        {
            self.phase = Phase::Adjusting {
                output: o,
                handle,
                start: rect,
                from: p,
                current: rect,
            };
            return Outcome::Redraw;
        }
        self.phase = Phase::Pressing {
            output,
            anchor: p,
            current: p,
        };
        Outcome::Redraw
    }

    /// `time` is the pointer event's millisecond timestamp, for double-clicks.
    pub fn release(&mut self, p: (f64, f64), time: u32) -> Outcome {
        match self.phase {
            Phase::Pressing {
                output,
                anchor,
                current,
            } => {
                let r = Rect::from_points(anchor, current);
                if r.w >= CLICK_SLOP || r.h >= CLICK_SLOP {
                    self.last_click = None;
                    self.phase = Phase::Selected { output, rect: r };
                    return Outcome::Redraw;
                }
                let double = self.is_double_click(time, p);
                self.last_click = Some((time, p));
                let rect = if double {
                    self.outputs[output]
                } else {
                    self.window_at(output, anchor)
                        .unwrap_or(self.outputs[output])
                };
                self.phase = Phase::Selected { output, rect };
                Outcome::Redraw
            }
            Phase::Adjusting {
                output,
                current,
                from,
                ..
            } => {
                let still = (p.0 - from.0).abs() < CLICK_SLOP && (p.1 - from.1).abs() < CLICK_SLOP;
                // A click on the selection (not a drag) can complete a double-click.
                let double = still && self.is_double_click(time, p);
                self.last_click = still.then_some((time, p));
                let rect = if double {
                    self.outputs[output]
                } else {
                    current
                };
                self.phase = Phase::Selected { output, rect };
                Outcome::Redraw
            }
            Phase::Idle | Phase::Selected { .. } => Outcome::Unchanged,
        }
    }

    pub fn key(&mut self, key: Key) -> Outcome {
        match key {
            Key::Image => self.set_mode(Mode::Image),
            Key::Gif => self.set_mode(Mode::Gif),
            Key::Video => self.set_mode(Mode::Video),
            Key::Delay => {
                self.delay = !self.delay;
                Outcome::Redraw
            }
            Key::FullOutput => match self.pointer_output() {
                Some(output) => {
                    self.phase = Phase::Selected {
                        output,
                        rect: self.outputs[output],
                    };
                    Outcome::Redraw
                }
                None => Outcome::Unchanged,
            },
            Key::Confirm => self.confirm(),
            Key::Cancel => Outcome::Cancel,
        }
    }

    pub fn set_mode(&mut self, mode: Mode) -> Outcome {
        if self.mode == mode {
            return Outcome::Unchanged;
        }
        self.mode = mode;
        Outcome::Redraw
    }

    /// Confirms the selection; with none, the output under the pointer.
    pub fn confirm(&mut self) -> Outcome {
        let (output, rect) = match self.phase {
            Phase::Selected { output, rect }
            | Phase::Adjusting {
                output,
                current: rect,
                ..
            } => (output, rect),
            Phase::Idle | Phase::Pressing { .. } => match self.pointer_output() {
                Some(output) => (output, self.outputs[output]),
                None => return Outcome::Unchanged,
            },
        };
        Outcome::Confirm {
            rect,
            output,
            mode: self.mode,
            delay: self.delay,
        }
    }

    fn is_double_click(&self, time: u32, p: (f64, f64)) -> bool {
        self.last_click.is_some_and(|(t, q)| {
            time.wrapping_sub(t) <= DOUBLE_CLICK_MS
                && (q.0 - p.0).abs() < CLICK_SLOP
                && (q.1 - p.1).abs() < CLICK_SLOP
        })
    }

    fn pointer_output(&self) -> Option<usize> {
        self.pointer
            .map(|(o, _)| o)
            .or((!self.outputs.is_empty()).then_some(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selector() -> Selector {
        let outputs = vec![
            Rect::new(0.0, 0.0, 1000.0, 800.0),
            Rect::new(1000.0, 0.0, 500.0, 800.0),
        ];
        let windows = vec![
            Rect::new(100.0, 100.0, 300.0, 200.0),
            Rect::new(50.0, 50.0, 900.0, 700.0),
        ];
        Selector::new(outputs, windows, Mode::Image)
    }

    #[test]
    fn dragging_selects_a_region_clamped_to_its_output() {
        let mut s = selector();
        s.press(0, (900.0, 700.0));
        s.motion(0, (1200.0, 750.0));
        assert_eq!(s.release((1200.0, 750.0), 10), Outcome::Redraw);
        assert_eq!(s.selection(), Some(Rect::new(900.0, 700.0, 100.0, 50.0)));
        assert!(s.adjustable());
    }

    #[test]
    fn a_click_takes_the_topmost_window_under_it() {
        let mut s = selector();
        s.motion(0, (150.0, 150.0));
        assert_eq!(
            s.hovered_window(),
            Some(Rect::new(100.0, 100.0, 300.0, 200.0))
        );
        s.press(0, (150.0, 150.0));
        s.release((151.0, 150.0), 10);
        assert_eq!(s.selection(), Some(Rect::new(100.0, 100.0, 300.0, 200.0)));
    }

    #[test]
    fn a_click_on_the_desktop_takes_the_output() {
        let mut s = selector();
        s.press(1, (1200.0, 400.0));
        s.release((1200.0, 400.0), 10);
        assert_eq!(s.selection(), Some(Rect::new(1000.0, 0.0, 500.0, 800.0)));
    }

    #[test]
    fn a_double_click_takes_the_whole_output() {
        let mut s = selector();
        s.press(0, (150.0, 150.0));
        s.release((150.0, 150.0), 100);
        s.press(0, (150.0, 150.0));
        s.release((150.0, 150.0), 300);
        assert_eq!(s.selection(), Some(Rect::new(0.0, 0.0, 1000.0, 800.0)));
    }

    #[test]
    fn handles_adjust_and_the_body_moves() {
        let mut s = selector();
        s.press(0, (100.0, 100.0));
        s.motion(0, (200.0, 200.0));
        s.release((200.0, 200.0), 10);
        // Grab the bottom-right corner and pull it out.
        s.press(0, (201.0, 199.0));
        s.motion(0, (251.0, 249.0));
        s.release((251.0, 249.0), 20);
        assert_eq!(s.selection(), Some(Rect::new(100.0, 100.0, 150.0, 150.0)));
        // Drag the body.
        s.press(0, (150.0, 150.0));
        s.motion(0, (160.0, 170.0));
        s.release((160.0, 170.0), 30);
        assert_eq!(s.selection(), Some(Rect::new(110.0, 120.0, 150.0, 150.0)));
    }

    #[test]
    fn pressing_outside_a_selection_starts_a_new_one() {
        let mut s = selector();
        s.press(0, (100.0, 100.0));
        s.motion(0, (200.0, 200.0));
        s.release((200.0, 200.0), 10);
        s.press(0, (500.0, 500.0));
        assert_eq!(s.selection(), None);
        s.motion(0, (560.0, 540.0));
        s.release((560.0, 540.0), 20);
        assert_eq!(s.selection(), Some(Rect::new(500.0, 500.0, 60.0, 40.0)));
    }

    #[test]
    fn keys_switch_mode_toggle_delay_and_confirm() {
        let mut s = selector();
        assert_eq!(s.key(Key::Gif), Outcome::Redraw);
        assert_eq!(s.key(Key::Gif), Outcome::Unchanged);
        s.key(Key::Delay);
        s.motion(1, (1100.0, 10.0));
        assert_eq!(
            s.key(Key::Confirm),
            Outcome::Confirm {
                rect: Rect::new(1000.0, 0.0, 500.0, 800.0),
                output: 1,
                mode: Mode::Gif,
                delay: true
            }
        );
        assert_eq!(s.key(Key::Cancel), Outcome::Cancel);
    }

    #[test]
    fn motion_without_a_change_does_not_redraw() {
        let mut s = selector();
        s.motion(0, (20.0, 20.0));
        assert_eq!(s.motion(0, (21.0, 20.0)), Outcome::Unchanged);
        assert_eq!(s.motion(0, (150.0, 150.0)), Outcome::Redraw);
    }
}
