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
#[derive(Debug, Default)]
pub struct LoopControl {
    exit: bool,
    redraw: bool,
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
}

/// A live window, borrowed by [`WindowApp::ready`].
pub struct WindowRef<'a> {
    window: &'a std::sync::Arc<winit::window::Window>,
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
                self.app.ready(&WindowRef { window: &window });
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
        if let Some(translated) = translate_event(event) {
            self.app.event(translated);
        }
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

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.tick() {
            event_loop.exit();
        }
    }
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
    use super::*;
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
        }
    }

    /// An adapter over a fresh app, in the state the loop starts in.
    fn new_adapter<'a>(config: &'a WindowConfig, app: &'a mut Recorder) -> Adapter<'a> {
        Adapter {
            config,
            app,
            window: None,
            failure: None,
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
