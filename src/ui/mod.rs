//! The Wayland side: a fullscreen layer-shell overlay per output to select
//! what to capture, and a small pill with a timer and stop button while
//! recording. Rendering is software (tiny-skia) into wl_shm buffers at each
//! output's physical size, presented at its logical size through wp_viewporter,
//! so fractional scales stay sharp and exact.

pub mod draw;
pub mod text;

use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_pointer,
    delegate_registry, delegate_seat, delegate_shm, delegate_simple,
    output::{OutputHandler, OutputState},
    reexports::protocols::wp::{
        cursor_shape::v1::client::wp_cursor_shape_device_v1::{Shape, WpCursorShapeDeviceV1},
        viewporter::client::{
            wp_viewport::{self, WpViewport},
            wp_viewporter::WpViewporter,
        },
    },
    registry::{ProvidesRegistryState, RegistryState, SimpleGlobal},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers},
        pointer::{
            PointerEvent, PointerEventKind, PointerHandler, cursor_shape::CursorShapeManager,
        },
    },
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
    shm::{
        Shm, ShmHandler,
        slot::{Buffer, SlotPool},
    },
};
use tiny_skia::Pixmap;
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle,
    globals::{GlobalList, registry_queue_init},
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
};

use self::draw::{Backdrop, OverlayView, PILL, Toolbar, ToolbarItem};
use self::text::Text;
use crate::capture::Frame;
use crate::geom::{OutputGeometry, Rect};
use crate::ipc::Instance;
use crate::selection::{Key, Mode, Outcome, Selector};

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

/// An output and its frozen frame.
pub struct Shot {
    pub geometry: OutputGeometry,
    pub frame: Frame,
}

pub enum Selection {
    Cancelled,
    Confirmed {
        rect: Rect,
        output: usize,
        mode: Mode,
        delay: bool,
    },
}

/// Freezes every output, shows the overlay, and returns what was chosen. The
/// overlay is gone from the screen by the time this returns.
pub fn select(instance: &Instance, mode: Mode, delay_secs: u64) -> Result<(Vec<Shot>, Selection)> {
    let (_conn, mut queue, globals) = connect()?;
    let qh = queue.handle();
    let mut app = App::new(&globals, &qh)?;
    queue.roundtrip(&mut app)?;
    queue.roundtrip(&mut app)?;

    let mut shots = Vec::new();
    let mut wl_outputs = Vec::new();
    for output in app.output_state.outputs() {
        let Some(info) = app.output_state.info(&output) else {
            continue;
        };
        let (Some(name), Some(pos), Some(size)) =
            (info.name.clone(), info.logical_position, info.logical_size)
        else {
            continue;
        };
        let frame = Frame::grab_output(&name)?;
        let geometry = OutputGeometry {
            name,
            logical: Rect::new(pos.0 as f64, pos.1 as f64, size.0 as f64, size.1 as f64),
            physical: (frame.width, frame.height),
        };
        shots.push(Shot { geometry, frame });
        wl_outputs.push(output);
    }
    if shots.is_empty() {
        bail!("the compositor reported no outputs");
    }

    let windows = crate::hypr::visible_windows();
    let outputs: Vec<Rect> = shots.iter().map(|s| s.geometry.logical).collect();
    let text = Text::new();
    let toolbars = shots
        .iter()
        .map(|s| Toolbar::layout(&text, s.geometry.logical.w, delay_secs))
        .collect();
    app.overlay = Some(Overlay {
        selector: Selector::new(outputs, windows, mode),
        geometries: shots.iter().map(|s| s.geometry.clone()).collect(),
        toolbars,
        text,
        on_toolbar: false,
        result: None,
    });

    for (i, (shot, output)) in shots.iter().zip(&wl_outputs).enumerate() {
        let surface = app.compositor.create_surface(&qh);
        let layer = app.layer_shell.create_layer_surface(
            &qh,
            surface,
            Layer::Overlay,
            Some("snapframe"),
            Some(output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        layer.set_size(0, 0);
        layer.commit();
        let backdrop = Backdrop::new(shot.frame.width, shot.frame.height, &shot.frame.rgb);
        app.add_surface(
            &qh,
            layer,
            i,
            (shot.frame.width, shot.frame.height),
            Some(backdrop),
        )?;
    }

    run(&mut queue, &mut app, instance, |app| {
        app.overlay.as_ref().is_some_and(|o| o.result.is_some())
    })?;
    let result = app
        .overlay
        .take()
        .and_then(|o| o.result)
        .unwrap_or(Selection::Cancelled);
    app.close(&mut queue)?;
    Ok((shots, result))
}

/// Shows the recording pill on the output named `output`, away from `region`,
/// until it's clicked or another snapframe asks to stop.
pub fn record_indicator(
    instance: &Instance,
    output: &str,
    region: &Rect,
    label: &str,
) -> Result<()> {
    let (_conn, mut queue, globals) = connect()?;
    let qh = queue.handle();
    let mut app = App::new(&globals, &qh)?;
    queue.roundtrip(&mut app)?;
    queue.roundtrip(&mut app)?;

    let (wl_output, info) = app
        .output_state
        .outputs()
        .find_map(|o| {
            let info = app.output_state.info(&o)?;
            (info.name.as_deref() == Some(output)).then_some((o, info))
        })
        .ok_or_else(|| anyhow!("output {output} disappeared"))?;
    let scale = info.scale_factor.max(1) as u32;
    let bounds = match (info.logical_position, info.logical_size) {
        (Some(p), Some(s)) => Rect::new(p.0 as f64, p.1 as f64, s.0 as f64, s.1 as f64),
        _ => region.to_owned(),
    };

    let surface = app.compositor.create_surface(&qh);
    let layer = app.layer_shell.create_layer_surface(
        &qh,
        surface,
        Layer::Overlay,
        Some("snapframe-recording"),
        Some(&wl_output),
    );
    let (anchor, margin) = pill_placement(&bounds, region);
    layer.set_anchor(anchor);
    layer.set_margin(margin.0, margin.1, margin.2, margin.3);
    layer.set_exclusive_zone(-1);
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    layer.set_size(PILL.0, PILL.1);
    layer.commit();
    app.pill = Some(Pill {
        text: Text::new(),
        started: Instant::now(),
        shown_secs: u64::MAX,
        label: label.to_owned(),
        stop: false,
    });
    app.add_surface(&qh, layer, 0, (PILL.0 * scale, PILL.1 * scale), None)?;

    run(&mut queue, &mut app, instance, |app| {
        app.pill.as_ref().is_some_and(|p| p.stop)
    })?;
    app.close(&mut queue)?;
    Ok(())
}

/// Top centre, unless the recording covers it; then bottom centre; then the
/// top-right corner. Returns the anchor and margins (top, right, bottom, left).
fn pill_placement(output: &Rect, region: &Rect) -> (Anchor, (i32, i32, i32, i32)) {
    let (w, h) = (PILL.0 as f64, PILL.1 as f64);
    let cx = output.x + (output.w - w) / 2.0;
    let top = Rect::new(cx, output.y + 12.0, w, h);
    let bottom = Rect::new(cx, output.bottom() - 12.0 - h, w, h);
    if region.intersect(&top).is_none() {
        (Anchor::TOP, (12, 0, 0, 0))
    } else if region.intersect(&bottom).is_none() {
        (Anchor::BOTTOM, (0, 0, 12, 0))
    } else {
        (Anchor::TOP | Anchor::RIGHT, (12, 12, 0, 0))
    }
}

fn connect() -> Result<(Connection, EventQueue<App>, GlobalList)> {
    let conn = Connection::connect_to_env().context("connecting to the Wayland compositor")?;
    let (globals, queue) = registry_queue_init(&conn).context("listing Wayland globals")?;
    Ok((conn, queue, globals))
}

/// Dispatches until `done`, polling the IPC socket and ticking the pill.
fn run(
    queue: &mut EventQueue<App>,
    app: &mut App,
    instance: &Instance,
    done: impl Fn(&App) -> bool,
) -> Result<()> {
    let qh = queue.handle();
    while !done(app) {
        queue.dispatch_pending(app)?;
        queue.flush()?;
        if let Some(guard) = queue.prepare_read() {
            let mut fd = libc::pollfd {
                fd: guard.connection_fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one valid pollfd for the duration of the call.
            let ready = unsafe { libc::poll(&mut fd, 1, 100) };
            if ready > 0 {
                guard.read().or_else(|e| match e {
                    wayland_client::backend::WaylandError::Io(io)
                        if io.kind() == std::io::ErrorKind::WouldBlock =>
                    {
                        Ok(0)
                    }
                    e => Err(e),
                })?;
            }
        }
        queue.dispatch_pending(app)?;
        if app.closed {
            bail!("the compositor closed the overlay");
        }
        for message in instance.drain() {
            if message == crate::ipc::TOGGLE {
                app.toggle();
            }
        }
        app.tick(&qh);
    }
    Ok(())
}

struct Overlay {
    selector: Selector,
    geometries: Vec<OutputGeometry>,
    toolbars: Vec<Toolbar>,
    text: Text,
    /// The left button went down on the toolbar, so its release isn't a selection.
    on_toolbar: bool,
    result: Option<Selection>,
}

struct Pill {
    text: Text,
    started: Instant,
    shown_secs: u64,
    label: String,
    stop: bool,
}

struct Surf {
    layer: LayerSurface,
    viewport: WpViewport,
    index: usize,
    physical: (u32, u32),
    configured: bool,
    dirty: bool,
    frame_pending: bool,
    backdrop: Option<Backdrop>,
    scratch: Pixmap,
    buffer: Option<Buffer>,
}

struct App {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    viewporter: SimpleGlobal<WpViewporter, 1>,
    cursor_shape: Option<CursorShapeManager>,
    pool: SlotPool,
    surfaces: Vec<Surf>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    shape_device: Option<WpCursorShapeDeviceV1>,
    overlay: Option<Overlay>,
    pill: Option<Pill>,
    closed: bool,
}

impl App {
    fn new(globals: &GlobalList, qh: &QueueHandle<App>) -> Result<App> {
        let shm = Shm::bind(globals, qh).context("wl_shm is unavailable")?;
        Ok(App {
            registry_state: RegistryState::new(globals),
            seat_state: SeatState::new(globals, qh),
            output_state: OutputState::new(globals, qh),
            compositor: CompositorState::bind(globals, qh)
                .context("wl_compositor is unavailable")?,
            layer_shell: LayerShell::bind(globals, qh)
                .context("the compositor lacks wlr-layer-shell")?,
            viewporter: SimpleGlobal::bind(globals, qh)
                .context("the compositor lacks wp_viewporter")?,
            cursor_shape: CursorShapeManager::bind(globals, qh).ok(),
            pool: SlotPool::new(4096, &shm).context("creating the shm pool")?,
            shm,
            surfaces: Vec::new(),
            keyboard: None,
            pointer: None,
            shape_device: None,
            overlay: None,
            pill: None,
            closed: false,
        })
    }

    fn add_surface(
        &mut self,
        qh: &QueueHandle<App>,
        layer: LayerSurface,
        index: usize,
        physical: (u32, u32),
        backdrop: Option<Backdrop>,
    ) -> Result<()> {
        let viewport = self
            .viewporter
            .get()?
            .get_viewport(layer.wl_surface(), qh, ());
        let scratch = Pixmap::new(physical.0, physical.1).context("surface size")?;
        self.surfaces.push(Surf {
            layer,
            viewport,
            index,
            physical,
            configured: false,
            dirty: true,
            frame_pending: false,
            backdrop,
            scratch,
            buffer: None,
        });
        Ok(())
    }

    /// Unmaps everything and waits for the compositor to have processed it.
    fn close(&mut self, queue: &mut EventQueue<App>) -> Result<()> {
        for surf in self.surfaces.drain(..) {
            surf.viewport.destroy();
            drop(surf.layer);
        }
        if let Some(device) = self.shape_device.take() {
            device.destroy();
        }
        queue.roundtrip(self)?;
        // Give the compositor a couple of frames to repaint without us.
        std::thread::sleep(Duration::from_millis(60));
        Ok(())
    }

    fn toggle(&mut self) {
        if let Some(overlay) = &mut self.overlay {
            overlay.result.get_or_insert(Selection::Cancelled);
        }
        if let Some(pill) = &mut self.pill {
            pill.stop = true;
        }
    }

    fn tick(&mut self, qh: &QueueHandle<App>) {
        if let Some(pill) = &mut self.pill {
            let secs = pill.started.elapsed().as_secs();
            if secs != pill.shown_secs {
                pill.shown_secs = secs;
                self.invalidate(qh);
            }
        }
    }

    fn invalidate(&mut self, qh: &QueueHandle<App>) {
        for i in 0..self.surfaces.len() {
            self.surfaces[i].dirty = true;
            if !self.surfaces[i].frame_pending {
                self.draw(qh, i);
            }
        }
    }

    fn apply(&mut self, qh: &QueueHandle<App>, outcome: Outcome) {
        let Some(overlay) = &mut self.overlay else {
            return;
        };
        match outcome {
            Outcome::Unchanged => {}
            Outcome::Redraw => self.invalidate(qh),
            Outcome::Cancel => overlay.result = Some(Selection::Cancelled),
            Outcome::Confirm {
                rect,
                output,
                mode,
                delay,
            } => {
                overlay.result = Some(Selection::Confirmed {
                    rect,
                    output,
                    mode,
                    delay,
                })
            }
        }
    }

    fn draw(&mut self, qh: &QueueHandle<App>, i: usize) {
        let surf = &mut self.surfaces[i];
        if !surf.configured {
            return;
        }
        let (w, h) = surf.physical;
        if let (Some(overlay), Some(backdrop)) = (&self.overlay, &surf.backdrop) {
            let geometry = &overlay.geometries[surf.index];
            let view = OverlayView {
                origin: (geometry.logical.x, geometry.logical.y),
                scale: geometry.scale().0,
                selection: overlay
                    .selector
                    .selection()
                    .filter(|r| r.intersect(&geometry.logical).is_some()),
                adjustable: overlay.selector.adjustable(),
                hovered_window: overlay.selector.hovered_window(),
                toolbar: &overlay.toolbars[surf.index],
                mode: overlay.selector.mode,
                delay: overlay.selector.delay,
            };
            draw::overlay(&mut surf.scratch, backdrop, &overlay.text, &view);
        } else if let Some(pill) = &self.pill {
            let scale = w as f64 / PILL.0 as f64;
            draw::pill(
                &mut surf.scratch,
                &pill.text,
                scale,
                pill.started.elapsed().as_secs(),
                &pill.label,
            );
        }
        let Ok((buffer, canvas)) =
            self.pool
                .create_buffer(w as i32, h as i32, w as i32 * 4, wl_shm::Format::Argb8888)
        else {
            return;
        };
        draw::to_argb(&surf.scratch, canvas);
        let surface = surf.layer.wl_surface();
        surface.damage_buffer(0, 0, w as i32, h as i32);
        surface.frame(qh, surface.clone());
        if buffer.attach_to(surface).is_err() {
            return;
        }
        surf.layer.commit();
        surf.buffer = Some(buffer);
        surf.dirty = false;
        surf.frame_pending = true;
    }

    fn surface_index(&self, surface: &wl_surface::WlSurface) -> Option<usize> {
        self.surfaces
            .iter()
            .position(|s| s.layer.wl_surface() == surface)
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        if let Some(i) = self.surface_index(surface) {
            self.surfaces[i].frame_pending = false;
            if self.surfaces[i].dirty {
                self.draw(qh, i);
            }
        }
    }
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.closed = true;
    }

    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        let Some(i) = self.surfaces.iter().position(|s| &s.layer == layer) else {
            return;
        };
        let (w, h) = configure.new_size;
        let surf = &mut self.surfaces[i];
        if w > 0 && h > 0 {
            surf.viewport.set_destination(w as i32, h as i32);
        }
        let first = !surf.configured;
        surf.configured = true;
        if first || surf.dirty {
            surf.frame_pending = false;
            self.draw(qh, i);
        }
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            self.keyboard = self.seat_state.get_keyboard(qh, &seat, None).ok();
        }
        if capability == Capability::Pointer
            && self.pointer.is_none()
            && let Ok(pointer) = self.seat_state.get_pointer(qh, &seat)
        {
            self.shape_device = self
                .cursor_shape
                .as_ref()
                .map(|m| m.get_shape_device(&pointer, qh));
            self.pointer = Some(pointer);
        }
    }
    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard
            && let Some(k) = self.keyboard.take()
        {
            k.release();
        }
        if capability == Capability::Pointer
            && let Some(p) = self.pointer.take()
        {
            p.release();
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
    }
    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
    }
    fn press_key(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        let key = match event.keysym {
            Keysym::Escape => Some(Key::Cancel),
            Keysym::Return | Keysym::KP_Enter => Some(Key::Confirm),
            _ => match event
                .utf8
                .as_deref()
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("s") | Some("i") => Some(Key::Image),
                Some("g") => Some(Key::Gif),
                Some("v") | Some("r") => Some(Key::Video),
                Some("d") => Some(Key::Delay),
                Some("f") => Some(Key::FullOutput),
                Some("q") => Some(Key::Cancel),
                _ => None,
            },
        };
        if let (Some(key), Some(overlay)) = (key, &mut self.overlay) {
            let outcome = overlay.selector.key(key);
            self.apply(qh, outcome);
        }
    }
    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }
    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: Modifiers,
        _: u32,
    ) {
    }
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            let Some(i) = self.surface_index(&event.surface) else {
                continue;
            };
            let index = self.surfaces[i].index;
            let local = event.position;
            if let PointerEventKind::Enter { serial } = event.kind
                && let Some(device) = &self.shape_device
            {
                let shape = if self.pill.is_some() {
                    Shape::Pointer
                } else {
                    Shape::Crosshair
                };
                device.set_shape(serial, shape);
            }
            if let Some(pill) = &mut self.pill {
                // The whole pill is the stop button.
                if let PointerEventKind::Release {
                    button: BTN_LEFT, ..
                } = event.kind
                {
                    pill.stop = true;
                }
                continue;
            }
            let Some(overlay) = &mut self.overlay else {
                continue;
            };
            let global = overlay.geometries[index].to_global(local);
            let outcome = match event.kind {
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                    overlay.selector.motion(index, global)
                }
                PointerEventKind::Press {
                    button: BTN_LEFT, ..
                } => {
                    let toolbar = &overlay.toolbars[index];
                    if toolbar.contains(local) {
                        overlay.on_toolbar = true;
                        match toolbar.hit(local) {
                            Some(ToolbarItem::Mode(mode)) => overlay.selector.set_mode(mode),
                            Some(ToolbarItem::Delay) => overlay.selector.key(Key::Delay),
                            None => Outcome::Unchanged,
                        }
                    } else {
                        overlay.on_toolbar = false;
                        overlay.selector.press(index, global)
                    }
                }
                PointerEventKind::Release {
                    button: BTN_LEFT,
                    time,
                    ..
                } => {
                    if std::mem::take(&mut overlay.on_toolbar) {
                        Outcome::Unchanged
                    } else {
                        overlay.selector.release(global, time)
                    }
                }
                PointerEventKind::Press {
                    button: BTN_RIGHT, ..
                } => Outcome::Cancel,
                _ => Outcome::Unchanged,
            };
            self.apply(qh, outcome);
        }
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

impl Dispatch<WpViewport, ()> for App {
    fn event(
        _: &mut Self,
        _: &WpViewport,
        _: wp_viewport::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl AsMut<SimpleGlobal<WpViewporter, 1>> for App {
    fn as_mut(&mut self) -> &mut SimpleGlobal<WpViewporter, 1> {
        &mut self.viewporter
    }
}

delegate_compositor!(App);
delegate_output!(App);
delegate_shm!(App);
delegate_seat!(App);
delegate_keyboard!(App);
delegate_pointer!(App);
delegate_layer!(App);
delegate_registry!(App);
delegate_simple!(App, WpViewporter, 1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pill_avoids_the_recorded_region() {
        let output = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let (anchor, _) = pill_placement(&output, &Rect::new(0.0, 500.0, 300.0, 300.0));
        assert_eq!(anchor, Anchor::TOP);
        let (anchor, _) = pill_placement(&output, &Rect::new(800.0, 0.0, 300.0, 300.0));
        assert_eq!(anchor, Anchor::BOTTOM);
        let (anchor, _) = pill_placement(&output, &output);
        assert_eq!(anchor, Anchor::TOP | Anchor::RIGHT);
    }
}
