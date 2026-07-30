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

/// The engine's event vocabulary, translated from the OS.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum WindowEvent {
    /// The user asked to close the window; the app decides what happens
    /// (typically request exit through [`LoopControl`]).
    CloseRequested,
    Resized {
        width: u32,
        height: u32,
    },
    ScaleFactorChanged {
        scale: f64,
    },
    /// The OS wants the window drawn now. Render here, never elsewhere.
    RedrawRequested,
    Key {
        code: KeyCode,
        pressed: bool,
        repeat: bool,
    },
    PointerMoved {
        x: f64,
        y: f64,
    },
    PointerButton {
        button: PointerButton,
        pressed: bool,
    },
    Wheel {
        dx: f32,
        dy: f32,
    },
    Focused(bool),
}

/// Physical keys, the subset current consumers need — grows additively.
/// Unmapped keys arrive as [`KeyCode::Unidentified`]; nothing is lost
/// silently, nothing panics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum KeyCode {
    Escape,
    Space,
    Enter,
    Tab,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    KeyW,
    KeyA,
    KeyS,
    KeyD,
    Unidentified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PointerButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    /// A native button by its OS index — distinct from the named
    /// variants above; nothing is aliased.
    Other(u16),
}

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

/// A live window, borrowed by [`WindowApp::ready`]. Accessors only;
/// surface-handle exposure for the renderer arrives with the renderer.
pub struct WindowRef<'a> {
    window: &'a winit::window::Window,
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
    event_loop
        .run_app(&mut adapter)
        .map_err(|error| WindowError::Loop {
            message: error.to_string(),
        })?;
    match adapter.failure.take() {
        Some(message) => Err(WindowError::Loop { message }),
        None => Ok(()),
    }
}

/// The bridge between the OS loop and the engine app.
struct Adapter<'a> {
    config: &'a WindowConfig,
    app: &'a mut dyn WindowApp,
    window: Option<winit::window::Window>,
    /// Failure inside a callback (window creation): carried out of the
    /// loop so it surfaces through `run_window_app`'s Result instead of
    /// a log line.
    failure: Option<String>,
}

impl ApplicationHandler for Adapter<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            // Desktop platforms emit exactly one Resumed; a second one
            // must not recreate the window.
            return;
        }
        let attributes = winit::window::Window::default_attributes()
            .with_title(&self.config.title)
            .with_resizable(self.config.resizable)
            .with_inner_size(winit::dpi::LogicalSize::new(
                self.config.logical_width,
                self.config.logical_height,
            ));
        match event_loop.create_window(attributes) {
            Ok(window) => {
                self.app.ready(&WindowRef { window: &window });
                self.window = Some(window);
            }
            Err(error) => {
                self.failure = Some(format!("window creation failed: {error}"));
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        if let Some(translated) = translate_event(&event) {
            self.app.event(translated);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.failure.is_some() {
            // The window never came up: the loop is exiting and the app
            // never saw `ready` — do not feed it `update` either.
            return;
        }
        let mut control = LoopControl::default();
        self.app.update(&mut control);
        if control.redraw
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
        if control.exit {
            event_loop.exit();
        }
    }
}

/// Translate one OS window event into the engine vocabulary. Events
/// with no engine meaning yet return `None` — dropped deliberately,
/// not accidentally.
fn translate_event(event: &winit::event::WindowEvent) -> Option<WindowEvent> {
    use winit::event::WindowEvent as We;
    match event {
        We::CloseRequested => Some(WindowEvent::CloseRequested),
        We::Resized(size) => Some(WindowEvent::Resized {
            width: size.width,
            height: size.height,
        }),
        We::ScaleFactorChanged { scale_factor, .. } => Some(WindowEvent::ScaleFactorChanged {
            scale: *scale_factor,
        }),
        We::RedrawRequested => Some(WindowEvent::RedrawRequested),
        We::KeyboardInput { event, .. } => Some(WindowEvent::Key {
            code: translate_key(event.physical_key),
            pressed: event.state.is_pressed(),
            repeat: event.repeat,
        }),
        We::CursorMoved { position, .. } => Some(WindowEvent::PointerMoved {
            x: position.x,
            y: position.y,
        }),
        We::MouseInput { state, button, .. } => Some(WindowEvent::PointerButton {
            button: translate_button(*button),
            pressed: state.is_pressed(),
        }),
        We::MouseWheel { delta, .. } => {
            let (dx, dy) = translate_wheel(*delta);
            Some(WindowEvent::Wheel { dx, dy })
        }
        We::Focused(focused) => Some(WindowEvent::Focused(*focused)),
        _ => None,
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
        assert!(matches!(wheel, Some(WindowEvent::Wheel { .. })));
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
}
