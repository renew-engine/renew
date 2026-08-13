//! Windowing and input — the OS window seam, behind the `window`
//! feature. The operating system owns the event loop (that is how every
//! desktop platform wants it); the engine's frame logic stays a plain
//! library the [`WindowApp`] callbacks drive. No windowing-library type
//! crosses this boundary: consumers see only the vocabulary below.
//!
//! Threading: [`run_window_app`] must be called on the main thread —
//! every desktop platform imposes it and macOS has no escape hatch.
//! Headless environments (no display server) fail recoverably with
//! [`WindowError::LoopUnavailable`], which is what lets tests skip
//! gracefully instead of crashing.

use core::fmt;

use winit::application::ApplicationHandler;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};

/// Plain-data window configuration.
#[derive(Debug, Clone)]
pub struct WindowConfig {
    pub title: String,
    pub logical_width: f64,
    pub logical_height: f64,
    pub resizable: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "renew".to_string(),
            logical_width: 1280.0,
            logical_height: 720.0,
            resizable: true,
        }
    }
}

/// The event vocabulary lives in its own dependency-free crate, which
/// this crate re-exports as [`crate::event`]: naming a key must not
/// require compiling a windowing library, and a crate boundary is the
/// only form of that promise the dependency graph can check.
/// Re-exported here too, so existing paths keep working.
///
/// **Note for anyone extending the translation below.** This crate is
/// now *downstream* of enums it used to define, and they are
/// `#[non_exhaustive]` — an attribute that binds downstream crates and
/// never the defining one. Constructing these values is still fine, and
/// is all the translation does. **Matching on one exhaustively is not**,
/// and will fail to compile with a message that looks baffling until you
/// remember the types moved.
pub use crate::event::{EVERY_EVENT_SHAPE, KeyCode, PointerButton, WindowEvent, shape_index};

/// What the app tells the loop each iteration.
///
/// **Readable as well as writable, and the read side is not for the
/// loop.** The loop owns this type and could reach the fields directly.
/// The accessors exist for a driver standing where the OS loop normally
/// stands — a test, a replay, a headless run — which otherwise cannot
/// see what the application asked for. Requests that nothing can observe
/// are requests a caller can delete without any test noticing, because
/// what a test can reach instead is the application's own copy of the
/// state, which it set itself.
#[derive(Debug, Default)]
pub struct LoopControl {
    exit: bool,
    redraw: bool,
    /// A change of mind about the cursor, if the app had one this
    /// iteration. `None` leaves the grab as it is.
    cursor: Option<bool>,
}

impl LoopControl {
    /// Leave the event loop after this iteration.
    pub fn exit(&mut self) {
        self.exit = true;
    }

    /// Ask the OS to schedule a redraw (delivered as
    /// [`WindowEvent::RedrawRequested`]).
    pub fn request_redraw(&mut self) {
        self.redraw = true;
    }

    /// Hold the cursor for mouse look, or release it.
    ///
    /// **Between events, not only at bring-up.** An application with a
    /// menu has to release the cursor to be clicked and take it back to
    /// be played, and it learns which it wants long after the window
    /// opened. Asking once at bring-up leaves it holding an invisible
    /// pointer over its own buttons.
    ///
    /// Idempotent, and the loop still owns the lifecycle: focus loss
    /// releases the grab whatever was asked here, and focus return
    /// reapplies the last request. Refusal is ordinary — cursor
    /// confinement is one of the places the desktops differ.
    pub fn hold_cursor(&mut self, held: bool) {
        self.cursor = Some(held);
    }

    /// Whether [`exit`](Self::exit) was called this iteration.
    #[must_use]
    pub fn exiting(&self) -> bool {
        self.exit
    }

    /// Whether [`request_redraw`](Self::request_redraw) was called this
    /// iteration.
    #[must_use]
    pub fn redraw_requested(&self) -> bool {
        self.redraw
    }

    /// What the app asked of the cursor this iteration, if anything.
    #[must_use]
    pub fn cursor_request(&self) -> Option<bool> {
        self.cursor
    }
}

/// A live window, borrowed by [`WindowApp::ready`].
pub struct WindowRef<'a> {
    window: &'a std::sync::Arc<winit::window::Window>,
    /// Where a cursor request is remembered, so the loop can reapply it
    /// when focus returns.
    ///
    /// A `Cell` because the application is handed `&WindowRef` and this
    /// is the one thing it may change — and because the event loop is a
    /// single thread, so there is nothing here for a lock to protect.
    cursor_wanted: &'a core::cell::Cell<bool>,
}

impl WindowRef<'_> {
    /// Current inner size in physical pixels.
    #[must_use]
    pub fn physical_size(&self) -> (u32, u32) {
        let size = self.window.inner_size();
        (size.width, size.height)
    }

    /// Current scale factor.
    #[must_use]
    pub fn scale_factor(&self) -> f64 {
        self.window.scale_factor()
    }

    /// An owned, opaque handle to this window for the renderer's surface
    /// creation. The returned value KEEPS THE WINDOW ALIVE for as long
    /// as it (or anything owning it) exists — surface validity is
    /// Hold the cursor inside the window and hide it, or let it go.
    ///
    /// **Answers rather than refuses.** Cursor confinement is one of the
    /// places the three desktops genuinely differ, and a caller can do
    /// nothing useful with the distinction between one compositor's
    /// refusal and another's — what it *can* do is fall back to keys. A
    /// `Result` would push a platform-specific error into every caller to
    /// be discarded on the spot.
    ///
    /// `true` means the cursor is held and hidden; `false` means this
    /// platform would not, and the caller should carry on without it. A
    /// first-person sample must stay playable either way.
    ///
    /// **Asked for once.** The loop remembers the request and reapplies
    /// it when focus returns, so a caller does not have to — and cannot,
    /// since only this seam is handed a window.
    #[must_use]
    pub fn grab_cursor(&self, held: bool) -> bool {
        self.cursor_wanted.set(held);
        grab_on(self.window, held)
    }

    /// ownership, not convention.
    #[must_use]
    pub fn native(&self) -> NativeWindow {
        NativeWindow {
            window: std::sync::Arc::clone(self.window),
        }
    }
}

/// An owned window handle: the value the renderer's window target takes
/// and holds. Opaque — it exposes nothing but the two standard
/// window-handle traits the graphics stack consumes. Cloning shares the
/// same OS window; the window stays alive while any clone exists.
#[derive(Clone)]
pub struct NativeWindow {
    window: std::sync::Arc<winit::window::Window>,
}

impl NativeWindow {
    /// Relabel the window's title bar.
    ///
    /// The engine renders no text yet, so the title bar is the only
    /// surface an application has for a running measurement — a real and
    /// conventional one, visible in the task switcher as well as on the
    /// window.
    ///
    /// MAIN THREAD ONLY, like everything else that touches a window.
    ///
    /// Ownership: `title` is borrowed for the duration of the call and
    /// copied into whatever representation the OS keeps; nothing here
    /// retains the string, and no allocation of the engine's survives
    /// the call.
    ///
    /// Cost: one OS call, which on some platforms round-trips to a
    /// window manager. Callers relabel on an interval, never once a
    /// frame — a title that changes faster than it can be read is
    /// unreadable anyway.
    pub fn set_title(&self, title: &str) {
        self.window.set_title(title);
    }
}

impl raw_window_handle::HasDisplayHandle for NativeWindow {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        raw_window_handle::HasDisplayHandle::display_handle(&*self.window)
    }
}

impl raw_window_handle::HasWindowHandle for NativeWindow {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        raw_window_handle::HasWindowHandle::window_handle(&*self.window)
    }
}

/// The application the window loop drives — the module's designed
/// extension point.
pub trait WindowApp {
    /// The window exists. On the success path this fires exactly once,
    /// before any other callback; if window creation fails, no callback
    /// fires and the failure surfaces through [`run_window_app`].
    fn ready(&mut self, window: &WindowRef<'_>);
    /// A translated OS event.
    fn event(&mut self, event: WindowEvent);
    /// Once per loop iteration, after events: tick simulation here,
    /// request redraws and exit through `control`.
    fn update(&mut self, control: &mut LoopControl);
}

/// Why the window loop could not run.
#[derive(Debug)]
#[non_exhaustive]
pub enum WindowError {
    /// No display server is reachable (headless environment) or the
    /// loop cannot be created here. Recoverable by design: headless
    /// callers detect this and proceed windowless.
    LoopUnavailable { message: String },
    /// The loop ran and failed.
    Loop { message: String },
}

impl fmt::Display for WindowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LoopUnavailable { message } => {
                write!(f, "window loop unavailable: {message}")
            }
            Self::Loop { message } => write!(f, "window loop failed: {message}"),
        }
    }
}

impl std::error::Error for WindowError {}

/// Run the OS event loop on the calling thread until the app exits.
///
/// MAIN THREAD ONLY: every desktop platform requires the loop on the
/// main thread (attempting otherwise panics inside the OS layer, by the
/// windowing stack's own contract). Call this from `main`.
///
/// # Errors
///
/// [`WindowError::LoopUnavailable`] when the loop cannot be created —
/// on Linux this is the recoverable no-display-server case;
/// [`WindowError::Loop`] when the running loop reports failure.
pub fn run_window_app(config: &WindowConfig, app: &mut dyn WindowApp) -> Result<(), WindowError> {
    let event_loop = EventLoop::new().map_err(|error| WindowError::LoopUnavailable {
        message: error.to_string(),
    })?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut adapter = Adapter {
        config,
        app,
        window: None,
        failure: None,
        commanding: false,
        cursor_wanted: core::cell::Cell::new(false),
    };
    let run = event_loop.run_app(&mut adapter);
    adapter.outcome(run)
}

/// The bridge between the OS loop and the engine app.
struct Adapter<'a> {
    config: &'a WindowConfig,
    app: &'a mut dyn WindowApp,
    window: Option<std::sync::Arc<winit::window::Window>>,
    /// Failure inside a callback (window creation): carried out of the
    /// loop so it surfaces through `run_window_app`'s Result instead of
    /// a log line.
    failure: Option<String>,
    /// Whether a command modifier is currently down.
    ///
    /// **Because a shortcut is not typing.** The windowing library
    /// reports the *unmodified* text for a modified key — Ctrl+S arrives
    /// with `text = Some("s")`, not with a control character — so
    /// filtering control characters cannot tell a shortcut from a
    /// keystroke. Without this, every shortcut in the application would
    /// insert a letter into whatever field held focus.
    ///
    /// Shift is deliberately not a command modifier: it is how capitals
    /// and most punctuation are typed.
    commanding: bool,
    /// Whether the application asked for the cursor to be held.
    ///
    /// **The layer owns the grab's lifecycle, not the application.** A
    /// cursor held across a tab away traps a player in a window they are
    /// trying to leave, so focus loss must release it — and an
    /// application that had to re-grab afterwards would need to change
    /// window state from an event, which this seam does not allow. So the
    /// request is remembered here and reapplied when focus returns, and
    /// mouse look survives alt-tab without the application knowing it
    /// happened.
    cursor_wanted: core::cell::Cell<bool>,
}

/// Where a window comes from: the running loop's creation call, reduced
/// to what [`Adapter::open`] needs of it — attributes in, a live window
/// or the OS's reason out.
///
/// Passing it in rather than calling the loop directly is what makes the
/// bring-up rules — at most one window, no retry after a refusal, a
/// refusal reported through the returned `Result` rather than logged —
/// testable against a refusing source, on machines and CI cells with no
/// display at all. A trait object rather than a type parameter, so the
/// real path and the tested path are literally the same instructions.
type WindowSource<'a> =
    dyn Fn(winit::window::WindowAttributes) -> Result<winit::window::Window, String> + 'a;

impl Adapter<'_> {
    /// Bring the window up and hand it to the app.
    ///
    /// Does nothing once a window exists — desktop platforms emit
    /// exactly one resume, but a second must never recreate it — and
    /// nothing after a refusal, which is final: the loop is on its way
    /// out and the reason already recorded is the one to report.
    fn open(&mut self, create: &WindowSource<'_>) {
        if self.window.is_some() || self.failure.is_some() {
            return;
        }
        let attributes = winit::window::Window::default_attributes()
            .with_title(&self.config.title)
            .with_resizable(self.config.resizable)
            .with_inner_size(winit::dpi::LogicalSize::new(
                self.config.logical_width,
                self.config.logical_height,
            ));
        match create(attributes) {
            Ok(window) => {
                let window = std::sync::Arc::new(window);
                self.app.ready(&WindowRef {
                    window: &window,
                    cursor_wanted: &self.cursor_wanted,
                });
                self.window = Some(window);
            }
            Err(message) => {
                self.failure = Some(format!("window creation failed: {message}"));
            }
        }
    }

    /// Deliver one OS event to the app. Events with no engine meaning
    /// are dropped here — deliberately, not accidentally.
    fn dispatch(&mut self, event: &winit::event::WindowEvent) {
        // Before the app sees it: a held cursor is the layer's to manage,
        // and the app's own handling of focus must not race it.
        if let winit::event::WindowEvent::Focused(focused) = event
            && self.cursor_wanted.get()
        {
            self.apply_cursor_grab(*focused);
        }
        if let winit::event::WindowEvent::ModifiersChanged(state) = event {
            self.commanding = commanding(state.state());
        }
        if let Some(translated) = translate_event(event) {
            self.app.event(translated);
        }
        // Text rides beside the key rather than replacing it: a press
        // that commits a character is both, and a consumer may want
        // either. Delivered after, so a driver that acts on the key
        // sees it first.
        if !self.commanding {
            for ch in typed_characters(event) {
                self.app.event(WindowEvent::TextEntered { ch });
            }
        }
    }

    /// Hold or release the cursor on the live window, if there is one.
    ///
    /// The request itself is remembered by the caller; this is only the
    /// asking. A window that has gone away has nothing to hold.
    fn apply_cursor_grab(&self, held: bool) -> bool {
        self.window
            .as_ref()
            .is_some_and(|window| grab_on(window, held))
    }

    /// One loop iteration's app work, after the events. Returns whether
    /// the loop must now leave: because the app asked, or because the
    /// window never came up — the app never saw `ready`, so it must not
    /// see `update` either, and there is nothing left to wait for.
    fn tick(&mut self) -> bool {
        if self.failure.is_some() {
            return true;
        }
        let mut control = LoopControl::default();
        self.app.update(&mut control);
        if let Some(held) = control.cursor {
            self.cursor_wanted.set(held);
            // The answer is the same one bring-up gets and means the
            // same thing: a desktop that refuses confinement plays on
            // without mouse look, which is not a failure to report.
            let _refused = self.apply_cursor_grab(held);
        }
        if control.redraw
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
        control.exit
    }

    /// The run's outcome. Two failures can reach here — the loop itself
    /// reported one, or a callback recorded one — and both surface
    /// through `run_window_app`'s `Result` instead of a log line.
    fn outcome(
        &mut self,
        run: Result<(), winit::error::EventLoopError>,
    ) -> Result<(), WindowError> {
        if let Err(error) = run {
            return Err(WindowError::Loop {
                message: error.to_string(),
            });
        }
        match self.failure.take() {
            Some(message) => Err(WindowError::Loop { message }),
            None => Ok(()),
        }
    }
}

impl ApplicationHandler for Adapter<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.open(&|attributes| {
            event_loop
                .create_window(attributes)
                .map_err(|error| error.to_string())
        });
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        self.dispatch(&event);
    }

    /// Raw device motion, which does not arrive with the window events.
    ///
    /// **The only device event forwarded, and deliberately so.** A window
    /// event says what happened to the window; this says what a device
    /// did, regardless of which window had focus or whether a cursor
    /// exists. A first-person view needs exactly this and nothing else on
    /// this seam, so everything else is dropped rather than translated
    /// into a vocabulary no caller has asked for.
    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: winit::event::DeviceEvent,
    ) {
        if let Some(translated) = translate_device_event(&event) {
            self.app.event(translated);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.tick() {
            event_loop.exit();
        }
    }
}

/// Translate one OS device event into the engine vocabulary.
///
/// Only motion has a meaning here; everything else returns `None`,
/// dropped deliberately rather than accidentally — the same rule the
/// window translation follows.
fn translate_device_event(event: &winit::event::DeviceEvent) -> Option<WindowEvent> {
    match event {
        winit::event::DeviceEvent::MouseMotion { delta } => Some(WindowEvent::PointerMotion {
            dx: delta.0,
            dy: delta.1,
        }),
        _ => None,
    }
}

/// Hold or release the cursor on `window`, and hide it while held.
///
/// Confined first and locked second, which is winit's own documented
/// order: Windows supports the first, macOS the second, and the
/// X11/Wayland pair varies by compositor. Answers whether it took.
fn grab_on(window: &winit::window::Window, held: bool) -> bool {
    use winit::window::CursorGrabMode;

    let granted = if held {
        window
            .set_cursor_grab(CursorGrabMode::Confined)
            .or_else(|_| window.set_cursor_grab(CursorGrabMode::Locked))
            .is_ok()
    } else {
        window.set_cursor_grab(CursorGrabMode::None).is_ok()
    };
    // Hidden only when the grab took: a hidden cursor that can still
    // wander out of the window is worse than a visible one, because the
    // player cannot see where it went.
    window.set_cursor_visible(!(held && granted));
    granted
}

/// Translate one OS window event into the engine vocabulary. Events
/// with no engine meaning yet return `None` — dropped deliberately,
/// not accidentally.
fn translate_event(event: &winit::event::WindowEvent) -> Option<WindowEvent> {
    use winit::event::WindowEvent as We;
    let translated = match event {
        We::CloseRequested => WindowEvent::CloseRequested,
        We::Resized(size) => WindowEvent::Resized {
            width: size.width,
            height: size.height,
        },
        // The next two arms are the only ones no test can reach: the
        // windowing library's scale-factor and key-event payloads have
        // no public constructor, and neither event can be provoked from
        // inside the process. Each is therefore a single delegation to
        // a function that IS driven directly.
        We::ScaleFactorChanged { scale_factor, .. } => scale_change(*scale_factor),
        We::KeyboardInput { event, .. } => keyboard(event.physical_key, event.state, event.repeat),
        We::RedrawRequested => WindowEvent::RedrawRequested,
        We::CursorMoved { position, .. } => WindowEvent::PointerMoved {
            x: position.x,
            y: position.y,
        },
        We::MouseInput { state, button, .. } => WindowEvent::PointerButton {
            button: translate_button(*button),
            pressed: state.is_pressed(),
        },
        We::MouseWheel { delta, .. } => {
            let (dx, dy) = translate_wheel(*delta);
            WindowEvent::Wheel { dx, dy }
        }
        We::Focused(focused) => WindowEvent::Focused(*focused),
        _ => return None,
    };
    Some(translated)
}

/// The characters one OS event committed, if any.
///
/// Only a key event carries text, and only while going down. Two of the
/// three platforms report nothing on release; the third re-derives it
/// from the key, so acting on a release would type some characters
/// twice and others wrongly.
fn typed_characters(event: &winit::event::WindowEvent) -> impl Iterator<Item = u32> + '_ {
    let typed = match event {
        // **Synthetic events are not typing.** On focus gain the
        // windowing library replays a press for every key physically
        // held, text and all, so alt-tabbing back with a letter down
        // would insert a character nobody struck.
        winit::event::WindowEvent::KeyboardInput {
            event,
            is_synthetic: false,
            ..
        } => committed(event.text.as_deref(), event.state.is_pressed()),
        _ => None,
    };
    typed.into_iter().flat_map(str::chars).filter_map(printable)
}

/// Is a command modifier down?
///
/// Control, alt and the platform key mean the keystroke is addressed to
/// the application rather than to a field. Shift is not one of them —
/// it is how capitals are typed.
fn commanding(state: winit::keyboard::ModifiersState) -> bool {
    state.control_key() || state.alt_key() || state.super_key()
}

/// The text a key event committed, if it committed any.
///
/// Split out from the event so it is driven by tests directly: the
/// windowing library's key event has no public constructor, which is the
/// same reason [`keyboard`] takes its fields loose.
fn committed(text: Option<&str>, pressed: bool) -> Option<&str> {
    if pressed { text } else { None }
}

/// A scalar worth calling text, as its code point.
///
/// **Control characters are dropped here rather than downstream.** The
/// window system reports `\r` for Enter and `\u{8}` for Backspace, and a
/// field that inserted either would hold bytes no reader can see. Those
/// keys arrive as [`WindowEvent::Key`], which is where editing intent
/// belongs. Everything outside the C0 range, `DEL` and the C1 range is
/// text and is delivered.
fn printable(ch: char) -> Option<u32> {
    (!ch.is_control()).then(|| u32::from(ch))
}

/// A display's scale factor changed.
fn scale_change(scale: f64) -> WindowEvent {
    WindowEvent::ScaleFactorChanged { scale }
}

/// A key changed state. Takes the fields loose rather than the library's
/// key event, because that type cannot be constructed outside the
/// library — this way the mapping is driven by tests without one.
fn keyboard(
    key: winit::keyboard::PhysicalKey,
    state: winit::event::ElementState,
    repeat: bool,
) -> WindowEvent {
    WindowEvent::Key {
        code: translate_key(key),
        pressed: state.is_pressed(),
        repeat,
    }
}

fn translate_key(key: winit::keyboard::PhysicalKey) -> KeyCode {
    use winit::keyboard::KeyCode as Wk;
    use winit::keyboard::PhysicalKey;
    match key {
        PhysicalKey::Code(code) => match code {
            Wk::Escape => KeyCode::Escape,
            Wk::Space => KeyCode::Space,
            Wk::Enter => KeyCode::Enter,
            Wk::Tab => KeyCode::Tab,
            Wk::Backspace => KeyCode::Backspace,
            Wk::Delete => KeyCode::Delete,
            Wk::Home => KeyCode::Home,
            Wk::End => KeyCode::End,
            Wk::ArrowUp => KeyCode::ArrowUp,
            Wk::ArrowDown => KeyCode::ArrowDown,
            Wk::ArrowLeft => KeyCode::ArrowLeft,
            Wk::ArrowRight => KeyCode::ArrowRight,
            Wk::KeyW => KeyCode::KeyW,
            Wk::KeyA => KeyCode::KeyA,
            Wk::KeyS => KeyCode::KeyS,
            Wk::KeyD => KeyCode::KeyD,
            _ => KeyCode::Unidentified,
        },
        PhysicalKey::Unidentified(_) => KeyCode::Unidentified,
    }
}

fn translate_button(button: winit::event::MouseButton) -> PointerButton {
    use winit::event::MouseButton as Wb;
    match button {
        Wb::Left => PointerButton::Left,
        Wb::Right => PointerButton::Right,
        Wb::Middle => PointerButton::Middle,
        Wb::Back => PointerButton::Back,
        Wb::Forward => PointerButton::Forward,
        Wb::Other(id) => PointerButton::Other(id),
    }
}

/// Line-based scroll steps and pixel deltas both arrive; lines are
/// scaled to a nominal pixel step so consumers see one unit.
fn translate_wheel(delta: winit::event::MouseScrollDelta) -> (f32, f32) {
    const LINE_STEP: f32 = 16.0;
    match delta {
        winit::event::MouseScrollDelta::LineDelta(x, y) => (x * LINE_STEP, y * LINE_STEP),
        winit::event::MouseScrollDelta::PixelDelta(pos) => {
            // Truncation to f32 is fine for wheel deltas.
            #[expect(
                clippy::cast_possible_truncation,
                reason = "wheel deltas are small; f32 is the consumer unit"
            )]
            let pair = (pos.x as f32, pos.y as f32);
            pair
        }
    }
}

#[cfg(test)]
mod tests {

    /// Text is delivered; the keys that also report text are not.
    ///
    /// The window system reports `\r` for Enter and `\u{8}` for
    /// Backspace. A field that inserted either would hold bytes no
    /// reader can see, and those keys already arrive as key events.
    #[test]
    fn control_characters_are_not_text() {
        // The edges of the exempted ranges, not only their middles: a
        // filter tested on letters and control codes alone can move its
        // boundaries freely. U+00A0 is one past the C1 range and is
        // typable, AltGr+Space on several layouts.
        for ch in ['a', 'Z', '9', ' ', '.', ':', 'é', '中', '🙂', '\u{a0}'] {
            assert_eq!(
                printable(ch),
                Some(u32::from(ch)),
                "{ch:?} is text and must be delivered"
            );
        }
        for ch in [
            '\r', '\n', '\t', '\u{8}', '\u{1b}', '\u{3}', '\u{7f}', '\u{9f}',
        ] {
            assert_eq!(
                printable(ch),
                None,
                "{ch:?} is a control character and must not be text"
            );
        }
    }

    /// A release repeats the press's text, and must not type it twice.
    #[test]
    fn only_a_press_commits_text() {
        assert_eq!(committed(Some("a"), true), Some("a"));
        assert_eq!(
            committed(Some("a"), false),
            None,
            "a release reports the text the press already delivered"
        );
        assert_eq!(committed(None, true), None, "a key with no text is a key");
    }

    /// **Shift is not a command modifier, and that asymmetry is the
    /// whole point.** The windowing library reports the unmodified text
    /// for a modified key, so Ctrl+S arrives carrying `"s"`; without
    /// this distinction every shortcut in an application would type a
    /// letter into whatever field held focus. Shift has to fall the
    /// other way, because it is how capitals are typed.
    #[test]
    fn the_command_modifiers_are_named_and_shift_is_not_one() {
        use winit::keyboard::ModifiersState as Ms;

        for state in [Ms::CONTROL, Ms::ALT, Ms::SUPER] {
            assert!(commanding(state), "{state:?} addresses the application");
        }
        assert!(!commanding(Ms::empty()), "no modifier is not a shortcut");
        assert!(!commanding(Ms::SHIFT), "shift is how capitals are typed");
        // Shift alongside one is still a shortcut: the test would pass on
        // an implementation that checked shift last and lost the others.
        assert!(
            commanding(Ms::SHIFT | Ms::CONTROL),
            "a shortcut does not stop being one because shift is held"
        );
    }

    use super::*;

    /// **Raw motion crosses as a delta, not a position.** The two are
    /// different events for a reason: a view driven by the cursor's
    /// position stops turning when the cursor stops moving at the edge of
    /// the window, and the whole point of this one is that it does not.
    #[test]
    fn device_motion_crosses_as_a_delta() {
        assert_eq!(
            translate_device_event(&winit::event::DeviceEvent::MouseMotion {
                delta: (-3.5, 1.25)
            }),
            Some(WindowEvent::PointerMotion { dx: -3.5, dy: 1.25 })
        );
    }

    /// Every other device event is dropped deliberately rather than
    /// translated into a vocabulary no caller has asked for.
    #[test]
    fn other_device_events_are_dropped_on_purpose() {
        for event in [
            winit::event::DeviceEvent::MouseWheel {
                delta: winit::event::MouseScrollDelta::LineDelta(0.0, 1.0),
            },
            winit::event::DeviceEvent::Motion {
                axis: 0,
                value: 1.0,
            },
            winit::event::DeviceEvent::Button {
                button: 0,
                state: winit::event::ElementState::Pressed,
            },
        ] {
            assert_eq!(translate_device_event(&event), None, "{event:?}");
        }
    }
    use winit::event::{MouseButton, MouseScrollDelta};
    use winit::keyboard::{KeyCode as Wk, PhysicalKey};

    #[test]
    fn config_defaults_are_sane() {
        let config = WindowConfig::default();
        assert_eq!(config.title, "renew");
        assert!(config.logical_width > 0.0 && config.logical_height > 0.0);
        assert!(config.resizable);
    }

    #[test]
    fn every_mapped_key_translates_and_unmapped_keys_are_identified_as_such() {
        let mapped = [
            (Wk::Escape, KeyCode::Escape),
            (Wk::Space, KeyCode::Space),
            (Wk::Enter, KeyCode::Enter),
            (Wk::Tab, KeyCode::Tab),
            (Wk::Backspace, KeyCode::Backspace),
            (Wk::Delete, KeyCode::Delete),
            (Wk::Home, KeyCode::Home),
            (Wk::End, KeyCode::End),
            (Wk::ArrowUp, KeyCode::ArrowUp),
            (Wk::ArrowDown, KeyCode::ArrowDown),
            (Wk::ArrowLeft, KeyCode::ArrowLeft),
            (Wk::ArrowRight, KeyCode::ArrowRight),
            (Wk::KeyW, KeyCode::KeyW),
            (Wk::KeyA, KeyCode::KeyA),
            (Wk::KeyS, KeyCode::KeyS),
            (Wk::KeyD, KeyCode::KeyD),
        ];
        for (winit_key, engine_key) in mapped {
            assert_eq!(
                translate_key(PhysicalKey::Code(winit_key)),
                engine_key,
                "{winit_key:?}"
            );
        }
        assert_eq!(
            translate_key(PhysicalKey::Code(Wk::KeyZ)),
            KeyCode::Unidentified
        );
    }

    #[test]
    fn buttons_translate_including_the_extended_ones() {
        assert_eq!(translate_button(MouseButton::Left), PointerButton::Left);
        assert_eq!(translate_button(MouseButton::Right), PointerButton::Right);
        assert_eq!(translate_button(MouseButton::Middle), PointerButton::Middle);
        assert_eq!(translate_button(MouseButton::Back), PointerButton::Back);
        assert_eq!(
            translate_button(MouseButton::Forward),
            PointerButton::Forward
        );
        assert_eq!(
            translate_button(MouseButton::Other(7)),
            PointerButton::Other(7)
        );
        // Named variants never alias native indices.
        assert_ne!(translate_button(MouseButton::Back), PointerButton::Other(0));
    }

    #[test]
    fn pointer_events_translate_through_the_full_event_path() {
        use winit::event::{DeviceId, ElementState, WindowEvent as We};
        let device = DeviceId::dummy();
        assert_eq!(
            translate_event(&We::CursorMoved {
                device_id: device,
                position: winit::dpi::PhysicalPosition::new(10.5, 20.5),
            }),
            Some(WindowEvent::PointerMoved { x: 10.5, y: 20.5 })
        );
        assert_eq!(
            translate_event(&We::MouseInput {
                device_id: device,
                state: ElementState::Pressed,
                button: MouseButton::Left,
            }),
            Some(WindowEvent::PointerButton {
                button: PointerButton::Left,
                pressed: true
            })
        );
        let wheel = translate_event(&We::MouseWheel {
            device_id: device,
            delta: MouseScrollDelta::LineDelta(0.0, 1.0),
            phase: winit::event::TouchPhase::Moved,
        });
        assert_eq!(wheel, Some(WindowEvent::Wheel { dx: 0.0, dy: 16.0 }));
    }

    #[test]
    fn unidentified_physical_keys_map_to_unidentified() {
        use winit::keyboard::NativeKeyCode;
        assert_eq!(
            translate_key(PhysicalKey::Unidentified(NativeKeyCode::Unidentified)),
            KeyCode::Unidentified
        );
    }

    #[test]
    fn window_errors_display_their_context() {
        let unavailable = WindowError::LoopUnavailable {
            message: "no display".to_string(),
        };
        assert!(unavailable.to_string().contains("no display"));
        let looped = WindowError::Loop {
            message: "backend fell over".to_string(),
        };
        assert!(looped.to_string().contains("backend fell over"));
    }

    #[test]
    fn wheel_lines_scale_to_pixels_and_pixels_pass_through() {
        let (_, dy) = translate_wheel(MouseScrollDelta::LineDelta(0.0, 2.0));
        assert!((dy - 32.0).abs() < f32::EPSILON);
        let (dx, _) = translate_wheel(MouseScrollDelta::PixelDelta(
            winit::dpi::PhysicalPosition::new(12.5, 0.0),
        ));
        assert!((dx - 12.5).abs() < f32::EPSILON);
    }

    #[test]
    fn constructible_events_translate_to_their_engine_forms() {
        use winit::event::WindowEvent as We;
        assert_eq!(
            translate_event(&We::CloseRequested),
            Some(WindowEvent::CloseRequested)
        );
        assert_eq!(
            translate_event(&We::Resized(winit::dpi::PhysicalSize::new(800, 600))),
            Some(WindowEvent::Resized {
                width: 800,
                height: 600
            })
        );
        assert_eq!(
            translate_event(&We::RedrawRequested),
            Some(WindowEvent::RedrawRequested)
        );
        assert_eq!(
            translate_event(&We::Focused(true)),
            Some(WindowEvent::Focused(true))
        );
        // An event with no engine meaning is dropped deliberately.
        assert_eq!(translate_event(&We::Destroyed), None);
    }

    #[test]
    fn loop_control_accumulates_requests() {
        let mut control = LoopControl::default();
        assert!(!control.exit && !control.redraw);
        control.exit();
        control.request_redraw();
        assert!(control.exit && control.redraw);
    }

    /// **A request between events is remembered, and silence changes
    /// nothing.** Remembering is the whole mechanism: the loop reapplies
    /// the last request when focus returns, so an app that asked once
    /// keeps mouse look across an alt-tab. An iteration that asks nothing
    /// must therefore leave the memory alone rather than write a default
    /// into it — otherwise every quiet frame would revoke the grab.
    ///
    /// The grab itself is not asserted here and cannot be: there is no
    /// window under a unit test, and confinement is a request the desktop
    /// may refuse anyway.
    #[test]
    fn a_cursor_request_between_events_is_remembered() {
        let config = WindowConfig::default();

        let mut asking = Recorder {
            ask_cursor: Some(true),
            ..Recorder::default()
        };
        let mut adapter = new_adapter(&config, &mut asking);
        assert!(!adapter.cursor_wanted.get(), "the loop starts holding none");
        assert!(!adapter.tick());
        assert!(
            adapter.cursor_wanted.get(),
            "the request must outlive the iteration that made it"
        );

        // A release is a request too, and reaches the same memory.
        let mut releasing = Recorder {
            ask_cursor: Some(false),
            ..Recorder::default()
        };
        let mut adapter = new_adapter(&config, &mut releasing);
        adapter.cursor_wanted.set(true);
        assert!(!adapter.tick());
        assert!(!adapter.cursor_wanted.get(), "a release must be applied");

        // And silence is not a release.
        let mut quiet = Recorder::default();
        let mut adapter = new_adapter(&config, &mut quiet);
        adapter.cursor_wanted.set(true);
        assert!(!adapter.tick());
        assert!(
            adapter.cursor_wanted.get(),
            "an iteration that asked nothing must not revoke the grab"
        );
    }

    /// The accessors report the fields the loop reads, and a silent
    /// iteration is distinguishable from one that asked for something.
    ///
    /// **The cursor is three-valued and the other two are not.** Saying
    /// nothing about the cursor leaves the grab alone, which is a
    /// different instruction from asking for it to be released — so a
    /// reader that collapsed `None` into `false` would turn every quiet
    /// iteration into a release request.
    #[test]
    fn loop_control_reports_what_was_asked() {
        let quiet = LoopControl::default();
        assert!(!quiet.exiting());
        assert!(!quiet.redraw_requested());
        assert_eq!(
            quiet.cursor_request(),
            None,
            "saying nothing about the cursor is not asking for it to be released"
        );

        let mut asked = LoopControl::default();
        asked.exit();
        asked.request_redraw();
        asked.hold_cursor(true);
        assert!(asked.exiting());
        assert!(asked.redraw_requested());
        assert_eq!(asked.cursor_request(), Some(true));

        // A release is a request, and the last one in the iteration wins.
        let mut released = LoopControl::default();
        released.hold_cursor(true);
        released.hold_cursor(false);
        assert_eq!(released.cursor_request(), Some(false));
        assert!(
            !released.exiting(),
            "a cursor request must not be read as an exit"
        );
    }

    #[test]
    fn scale_and_key_payloads_survive_translation() {
        use winit::event::ElementState;
        assert_eq!(
            scale_change(2.5),
            WindowEvent::ScaleFactorChanged { scale: 2.5 }
        );
        assert_eq!(
            keyboard(PhysicalKey::Code(Wk::Space), ElementState::Pressed, true),
            WindowEvent::Key {
                code: KeyCode::Space,
                pressed: true,
                repeat: true
            }
        );
        // The two flags are independent and neither is the other.
        assert_eq!(
            keyboard(PhysicalKey::Code(Wk::KeyZ), ElementState::Released, false),
            WindowEvent::Key {
                code: KeyCode::Unidentified,
                pressed: false,
                repeat: false
            }
        );
    }

    /// A [`WindowApp`] that records what the adapter told it, and asks
    /// for whatever the test configured.
    #[derive(Default)]
    struct Recorder {
        events: Vec<WindowEvent>,
        updates: u32,
        ask_redraw: bool,
        ask_exit: bool,
        /// What to ask of the cursor, if anything. `None` asks nothing,
        /// which is the case that must leave the grab alone.
        ask_cursor: Option<bool>,
    }

    impl WindowApp for Recorder {
        // Unreachable from here, and nothing can change that: a
        // `WindowRef` borrows a live OS window, which needs a running
        // event loop no unit test can host. The callback's own contract
        // — fires exactly once, before the rest — is proven by the
        // windowed smoke test instead.
        fn ready(&mut self, _window: &WindowRef<'_>) {}

        fn event(&mut self, event: WindowEvent) {
            self.events.push(event);
        }

        fn update(&mut self, control: &mut LoopControl) {
            self.updates += 1;
            if self.ask_redraw {
                control.request_redraw();
            }
            if self.ask_exit {
                control.exit();
            }
            if let Some(held) = self.ask_cursor {
                control.hold_cursor(held);
            }
        }
    }

    /// An adapter over a fresh app, in the state the loop starts in.
    fn new_adapter<'a>(config: &'a WindowConfig, app: &'a mut Recorder) -> Adapter<'a> {
        Adapter {
            config,
            app,
            window: None,
            failure: None,
            commanding: false,
            cursor_wanted: core::cell::Cell::new(false),
        }
    }

    #[test]
    fn a_refused_window_is_reported_once_and_never_retried() {
        let config = WindowConfig {
            title: "renew-refused".to_string(),
            logical_width: 640.0,
            logical_height: 480.0,
            resizable: false,
        };
        let attempts = std::cell::Cell::new(0_u32);
        let refuse: &WindowSource<'_> = &|attributes| {
            attempts.set(attempts.get() + 1);
            // The config reaches the OS call unaltered.
            assert_eq!(attributes.title, "renew-refused");
            assert!(!attributes.resizable);
            assert_eq!(
                attributes.inner_size,
                Some(winit::dpi::LogicalSize::new(640.0, 480.0).into())
            );
            Err("no display".to_string())
        };
        let mut app = Recorder::default();
        let mut adapter = new_adapter(&config, &mut app);
        adapter.open(refuse);
        // A refusal is final: a second resume must not reach the OS again.
        adapter.open(refuse);
        assert_eq!(attempts.get(), 1, "a refusal must not be retried");
        assert!(adapter.tick(), "a failed bring-up must end the loop");
        let error = adapter
            .outcome(Ok(()))
            .expect_err("the refusal must surface through the Result");
        assert_eq!(
            error.to_string(),
            "window loop failed: window creation failed: no display"
        );
        assert_eq!(app.updates, 0, "a failed bring-up must not drive the app");
    }

    #[test]
    fn translated_events_reach_the_app_and_meaningless_ones_do_not() {
        use winit::event::WindowEvent as We;
        let config = WindowConfig::default();
        let mut app = Recorder::default();
        let mut adapter = new_adapter(&config, &mut app);
        adapter.dispatch(&We::CloseRequested);
        adapter.dispatch(&We::Destroyed);
        adapter.dispatch(&We::Focused(false));
        assert_eq!(
            app.events,
            [WindowEvent::CloseRequested, WindowEvent::Focused(false)],
            "the untranslatable event must be dropped, and only it"
        );
    }

    /// **A modifier is a state, not an event, so the seam has to hold
    /// it.** The library reports a modifier change once and then reports
    /// keys, so whether a later keystroke is typing or a shortcut is
    /// only answerable from what was remembered in between. Dispatching
    /// the change is what proves it lands somewhere durable — the arm
    /// could compute the right answer and drop it.
    ///
    /// That a held modifier then suppresses text is not asserted here
    /// and cannot be: it takes a key event, and the library's key event
    /// has no public constructor.
    #[test]
    fn a_modifier_change_is_remembered_until_the_next_one() {
        use winit::event::WindowEvent as We;
        use winit::keyboard::ModifiersState as Ms;

        let config = WindowConfig::default();
        let mut app = Recorder::default();
        let mut adapter = new_adapter(&config, &mut app);
        assert!(!adapter.commanding, "the loop starts with nothing held");

        adapter.dispatch(&We::ModifiersChanged(Ms::CONTROL.into()));
        assert!(adapter.commanding, "a control press must be remembered");

        adapter.dispatch(&We::ModifiersChanged(Ms::SHIFT.into()));
        assert!(
            !adapter.commanding,
            "releasing control while holding shift returns to typing"
        );

        adapter.dispatch(&We::ModifiersChanged(Ms::empty().into()));
        assert!(!adapter.commanding, "releasing everything holds nothing");

        assert!(
            app.events.is_empty(),
            "a modifier change is state, and reaches the application as no event of its own"
        );
    }

    #[test]
    fn an_iteration_drives_the_app_and_carries_its_exit_request() {
        let config = WindowConfig::default();
        let mut quiet = Recorder::default();
        let mut adapter = new_adapter(&config, &mut quiet);
        assert!(!adapter.tick(), "an app that asks nothing keeps the loop");
        assert_eq!(quiet.updates, 1, "every iteration drives the app once");

        // Asking for a redraw before the window exists is not a reason
        // to skip the app's exit request — or to reach for a window
        // that is not there.
        let mut demanding = Recorder {
            ask_redraw: true,
            ask_exit: true,
            ..Recorder::default()
        };
        let mut adapter = new_adapter(&config, &mut demanding);
        assert!(adapter.tick(), "the app asked to exit");
        assert_eq!(demanding.updates, 1);
    }

    #[test]
    fn a_loop_failure_surfaces_even_though_no_callback_recorded_one() {
        let config = WindowConfig::default();
        let mut app = Recorder::default();
        let mut adapter = new_adapter(&config, &mut app);
        let reported = winit::error::EventLoopError::ExitFailure(3);
        let expected = format!("window loop failed: {reported}");
        let error = adapter
            .outcome(Err(reported))
            .expect_err("a loop failure must surface");
        assert_eq!(error.to_string(), expected);
        // Nothing failed, nothing recorded: the run succeeded.
        assert!(adapter.outcome(Ok(())).is_ok());
    }
}
