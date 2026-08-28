//! Windowing and input — the OS window seam, behind the `window`
//! feature. The operating system owns the event loop (that is how every
//! desktop platform wants it); the engine's frame logic stays a plain
//! library the [`WindowApp`] callbacks drive. No windowing-library type
//! crosses this boundary — consumers see only the vocabulary below —
//! with one documented exception at the Android entry seam, where the
//! OS hands the process a handle before any engine code runs.
//!
//! Threading: [`run_window_app`] must be called on the main thread —
//! every desktop platform imposes it and macOS has no escape hatch.
//! Headless environments (no display server) fail recoverably with
//! [`WindowError::LoopUnavailable`], which is what lets tests skip
//! gracefully instead of crashing.

use core::fmt;

use winit::application::ApplicationHandler;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};

/// A window's icon, as straight RGBA rows.
///
/// **Plain data, like everything else that crosses this seam.** No
/// windowing-library type appears here for the reason the module doc
/// gives: a consumer naming an icon must not be compiling a windowing
/// library to do it. The bytes are eight-bit red, green, blue and alpha
/// in that order, row-major from the top-left, which is what every image
/// decoder on the way in already produces.
///
/// **Validated at construction rather than at the window.** A window is
/// created once, deep inside a platform callback, at the one moment
/// there is nowhere useful to report a mistake to; the size of a byte
/// slice against two dimensions is arithmetic that can be done anywhere,
/// so it is done where the caller is still holding the error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowIcon {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl WindowIcon {
    /// Take `rgba` as a `width` by `height` image.
    ///
    /// # Errors
    ///
    /// [`IconError::Empty`] for an image with no pixels in it, and
    /// [`IconError::WrongLength`] when the bytes and the dimensions
    /// disagree — which is the mistake this type exists to catch, since
    /// four bytes a pixel is the assumption every caller makes silently.
    pub fn from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self, IconError> {
        if width == 0 || height == 0 {
            return Err(IconError::Empty);
        }
        let wanted = usize::try_from(width)
            .ok()
            .and_then(|width| usize::try_from(height).ok().map(|height| (width, height)))
            .and_then(|(width, height)| width.checked_mul(height))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or(IconError::Empty)?;
        if rgba.len() != wanted {
            return Err(IconError::WrongLength {
                expected: wanted,
                found: rgba.len(),
            });
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }

    /// How wide it is, in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// How tall it is, in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Its rows, as RGBA bytes.
    #[must_use]
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
}

/// Why a set of bytes is not an icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconError {
    /// No pixels: a zero on either side.
    Empty,
    /// The bytes and the dimensions disagree, at four bytes a pixel.
    WrongLength {
        /// What `width * height * 4` came to.
        expected: usize,
        /// How many bytes arrived.
        found: usize,
    },
}

impl fmt::Display for IconError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "an icon with no pixels in it"),
            Self::WrongLength { expected, found } => {
                write!(f, "an icon of {expected} bytes arrived as {found}")
            }
        }
    }
}

impl std::error::Error for IconError {}

/// Plain-data window configuration.
#[derive(Debug, Clone)]
pub struct WindowConfig {
    /// What the title bar says.
    pub title: String,
    /// How wide the window opens, in logical pixels, before
    /// [`Self::min_logical_width`] is applied to it.
    pub logical_width: f64,
    /// How tall the window opens, in logical pixels, before
    /// [`Self::min_logical_height`] is applied to it.
    pub logical_height: f64,
    /// Whether the user may resize it at all.
    pub resizable: bool,
    /// The narrowest the window may be dragged to, in logical pixels.
    /// Nought is no floor, and is the default.
    ///
    /// **A layout can always be checked against the size the window
    /// opened at; a floor is the only size it can be checked against
    /// and rely on.** The opening size is one a user takes away by
    /// dragging a corner, so a guard measured against it outlives the
    /// thing it measured. This is the size that is still there.
    ///
    /// **Nought rather than an `Option`, because the platform does not
    /// make the distinction.** A floor of nought and no floor produce
    /// the same window everywhere this runs, so an `Option` would carry
    /// a state nothing downstream can tell apart — and two named
    /// scalars match the two fields above them, where a pair could be
    /// written the wrong way round and still compile.
    ///
    /// Anything not finite, or below nought, is **read as no floor**.
    /// See [`floor_of`] for why that is a sanitised value rather than
    /// an error or a pass-through.
    ///
    /// A floor above [`Self::logical_width`] raises it, so that the
    /// window is not smaller than its floor on any platform — the
    /// backends do not agree about that on their own.
    ///
    /// Unsupported on iOS, Android and Orbital, where the windowing
    /// library ignores it.
    pub min_logical_width: f64,
    /// The shortest the window may be dragged to, in logical pixels.
    /// Nought is no floor, and is the default. See
    /// [`Self::min_logical_width`], which this matches in every respect.
    pub min_logical_height: f64,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "renew".to_string(),
            logical_width: 1280.0,
            logical_height: 720.0,
            resizable: true,
            // No floor: what every window did before these two existed,
            // so a caller that says nothing gets what it used to get.
            min_logical_width: 0.0,
            min_logical_height: 0.0,
        }
    }
}

/// One floor dimension as the window seam will use it: finite, not
/// negative, and nought where the caller asked for nonsense.
///
/// **Sanitised here rather than passed on, and the reason is a crash.**
/// The windowing library clamps the requested size between the floor
/// and a ceiling, and that clamp asserts `min <= max` — so a floor of
/// `f64::NAN` or `f64::INFINITY` fails an assertion inside a dependency,
/// during window creation, in a seam whose whole documented character is
/// that failures come back as a `Result`. A negative floor is the
/// quieter half of the same problem: it converts to an unsigned pixel
/// count by saturating at nought, so it silently means no floor already.
///
/// **Sanitised rather than refused**, because refusing needs an error
/// this seam has nowhere to report from: a window is created deep inside
/// a platform callback, which is the one moment there is nobody to hand
/// a `Result` to. `WindowIcon` makes the other choice and validates at
/// construction — it can, because an icon is built by the caller before
/// the loop starts. A config is a struct literal with no constructor to
/// check it in, so the check lives here and its answer is documented
/// rather than surprising.
#[must_use]
pub fn floor_of(asked: f64) -> f64 {
    if asked.is_finite() && asked > 0.0 {
        asked
    } else {
        0.0
    }
}

/// The event vocabulary lives in its own dependency-free crate, which
/// this crate re-exports as [`crate::event`]: naming a key must not
/// require compiling a windowing library, and a crate boundary is the
/// only form of that promise the dependency graph can check.
/// Re-exported here too, so existing paths keep working.
///
/// **Note for anyone extending the translation below.** This crate is
/// *downstream* of enums it used to define. They are deliberately
/// exhaustive — the `#[non_exhaustive]` they once carried is gone
/// (2026-08-04, recorded at the defining crate) — so a new variant
/// breaks every downstream match at compile time, which is the
/// vocabulary's designed forcing function. Constructing values here is
/// unaffected; a match elsewhere failing to compile after a variant
/// lands is that design working, not an accident to suppress.
pub use crate::event::{
    EVERY_EVENT_SHAPE, KeyCode, PointerButton, TouchPhase, WindowEvent, shape_index,
};

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
    /// A change of mind about filling the screen, on the same terms as
    /// the cursor: `None` leaves the window as it is.
    fullscreen: Option<bool>,
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

    /// Fill the screen, or go back to a window.
    ///
    /// **Between events, and for the same reason the cursor is.** An
    /// application learns whether it wants the screen long after the
    /// window opened: it is a line in a settings menu and a key on the
    /// keyboard, and a flag readable only at bring-up gives a player no
    /// way out of a fullscreen they turned on. That argument is
    /// [`Self::hold_cursor`]'s, unchanged; it was made there first and
    /// it applies here exactly.
    ///
    /// Borderless, on whichever monitor the window is on. Exclusive
    /// fullscreen needs a monitor handle and a video mode, which are two
    /// windowing-library types this seam exists to keep out — see the
    /// module doc — and nothing has asked for one. A `bool` is what an
    /// application wants to say.
    ///
    /// Idempotent, and asking twice for the same thing costs one call to
    /// the platform that changes nothing.
    ///
    /// **Applied to the live window and not remembered**, which is the
    /// one way this differs from the cursor beside it. The cursor is
    /// remembered because focus loss makes the OS drop the grab and the
    /// loop has to put it back; nothing takes fullscreen away, so there
    /// is nothing to reapply. The case that would need a memory is a
    /// surface epoch closing and reopening — and it cannot arise:
    /// desktop platforms close no epochs (see the module doc), and the
    /// platform that does is one where the windowing library does not
    /// support fullscreen at all. Written down because "the request is
    /// dropped when there is no window" is otherwise a silent hole, and
    /// the day a platform both closes epochs and honours this is the day
    /// to give it the cursor's treatment.
    pub fn set_fullscreen(&mut self, filling: bool) {
        self.fullscreen = Some(filling);
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

    /// What the app asked about filling the screen this iteration, if
    /// anything. Three-valued for the same reason the cursor is: saying
    /// nothing leaves the window alone, which is a different instruction
    /// from asking for a window back.
    #[must_use]
    pub fn fullscreen_request(&self) -> Option<bool> {
        self.fullscreen
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

    /// An owned, opaque handle to this window for the renderer's surface
    /// creation. The returned value KEEPS THE WINDOW ALIVE for as long
    /// as it (or anything owning it) exists — within the surface epoch,
    /// surface validity is ownership, not convention.
    ///
    /// **Within the epoch** is the load-bearing qualifier: a platform
    /// that revokes windows (a mobile OS backgrounding the process)
    /// invalidates the underlying surface no matter who holds a clone,
    /// which is exactly why [`WindowApp::surface_lost`] obliges the
    /// application to drop every one of these before it returns — and
    /// why the loop verifies that it did.
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
/// same OS window; the window stays alive while any clone exists —
/// until the platform closes the surface epoch, at which point
/// [`WindowApp::surface_lost`] obliges every clone to be dropped,
/// because the OS invalidates the underlying surface regardless of who
/// still holds one.
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
    /// The window exists. Fires once per **surface epoch** — the tenure
    /// between the platform granting a window and taking it away —
    /// before any other callback of that epoch. Desktop platforms grant
    /// exactly one epoch, so there this still fires exactly once; a
    /// platform that revokes the window (Android backgrounding the
    /// process) closes the epoch through [`surface_lost`] and announces
    /// the next one here. If window creation fails, no callback of
    /// that epoch fires and the failure surfaces through
    /// [`run_window_app`].
    ///
    /// [`surface_lost`]: Self::surface_lost
    fn ready(&mut self, window: &WindowRef<'_>);
    /// A translated OS event.
    fn event(&mut self, event: WindowEvent);
    /// Once per loop iteration, after events: tick simulation here,
    /// request redraws and exit through `control`.
    fn update(&mut self, control: &mut LoopControl);
    /// The platform is invalidating the window: the surface epoch is
    /// closing. Every [`NativeWindow`] clone — and every renderer
    /// target built from one — must be dropped before this returns,
    /// because the operating system is about to destroy what they point
    /// at; the loop verifies the release and treats a survivor as a
    /// contract violation (fatal in dev builds). The next window, if
    /// one comes, is announced by [`ready`](Self::ready) again.
    ///
    /// Defaulted to a no-op because desktop platforms never revoke a
    /// window, so a desktop-only application has nothing to release —
    /// an application holding window-derived values that targets a
    /// platform which does revoke them must implement this. Today the
    /// one revoking platform is Android; iOS suspends without revoking
    /// and deliberately does not reach this callback.
    fn surface_lost(&mut self) {}

    /// The icon this application's window should carry, if it has one.
    ///
    /// Asked once per surface epoch, immediately before the window is
    /// created, so an application that rebuilds its icon between epochs
    /// gets the one it has now.
    ///
    /// **Defaulted to nothing, and on the trait rather than in
    /// [`WindowConfig`].** An icon is a picture the application owns, in
    /// a format only the application knows how to produce - it arrives
    /// from a decoder, an asset pack or a build script - where the
    /// config is a handful of plain settings a caller writes by hand.
    /// Defaulting it also means every application that already exists
    /// keeps compiling and keeps the icon the system gives an unadorned
    /// executable, which is what it had before.
    ///
    /// A platform with no notion of a window icon ignores this, which
    /// is why it answers with a picture rather than a promise.
    fn icon(&self) -> Option<WindowIcon> {
        None
    }

    /// The application has stopped being the one in front.
    ///
    /// **Deliberately not "backgrounded", because the platforms do not
    /// agree on how much this means.** Android sends it when the
    /// activity goes to the background. iOS sends it for every
    /// interruption — an incoming call, the app switcher, a
    /// notification shade pulled down — and the application may be
    /// frontmost again a second later without ever having left. What is
    /// common to both, and all this callback promises, is that nobody
    /// is attending to it right now.
    ///
    /// **Separate from [`surface_lost`](Self::surface_lost), because on
    /// one platform they are the same event and on another they are
    /// not.** Android revokes the window when it backgrounds an app, so
    /// both fire; iOS keeps the surface, so only this one does. An
    /// application that wants to pause a clock or stop a sound wants
    /// this callback; one that must release window-derived resources
    /// wants the other.
    ///
    /// Delivered once per interruption: a platform that repeats itself
    /// — Android emits back-to-back suspends — does not repeat this.
    fn suspended(&mut self) {}

    /// The application has come back to the foreground.
    ///
    /// Fires only after a [`suspended`](Self::suspended), so a launch is
    /// not a resume — the first time an application is given a window
    /// it hears [`ready`](Self::ready) and nothing else. On a platform
    /// that revoked the surface, [`ready`](Self::ready) follows this
    /// with the new one.
    fn resumed(&mut self) {}
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
/// **On iOS this function does not return once the loop is entered.**
/// The loop is entered through `UIApplicationMain`, which owns the
/// process from that point on: the app ends when the system ends it, and
/// the code after that call is unreachable on that target. The signature
/// keeps its `Result` anyway, so that one `main` compiles everywhere —
/// the same reason Android's arm below keeps it.
///
/// Nothing about this is a defect to fix; it is how the platform runs
/// applications. It is written down because a reader on a desktop cannot
/// see it, and a caller that logs "the loop returned cleanly" after this
/// call would be describing an event that never happens on iOS.
///
/// # Errors
///
/// [`WindowError::LoopUnavailable`] when the loop cannot be created —
/// on Linux this is the recoverable no-display-server case;
/// [`WindowError::Loop`] when the running loop reports failure.
///
/// **On iOS, in practice, neither.** The windowing crate's iOS backend
/// reports loop-creation problems by panicking rather than by returning,
/// and the one error it can return needs a *second* loop built from
/// another thread — which the first call cannot come back to arrange.
/// So a caller there should expect this function to take the process,
/// not to hand back a `Result` worth matching on.
#[cfg(not(target_os = "android"))]
pub fn run_window_app(config: &WindowConfig, app: &mut dyn WindowApp) -> Result<(), WindowError> {
    let event_loop = EventLoop::new().map_err(|error| WindowError::LoopUnavailable {
        message: error.to_string(),
    })?;
    drive(event_loop, config, app)
}

/// Android's spelling of "the loop cannot be created here".
///
/// On Android the event loop can only be built around the activity
/// handle the OS glue passes to `android_main` — the windowing stack
/// panics without it — so this entry point cannot work there and says
/// so recoverably instead of reaching the panic: the same
/// [`WindowError::LoopUnavailable`] a headless Linux box reports, for
/// the same reason, which keeps every caller of this function compiling
/// and behaving on every target. The working doorway is
/// [`android::run_window_app_android`].
///
/// # Errors
///
/// Always [`WindowError::LoopUnavailable`].
#[cfg(target_os = "android")]
pub fn run_window_app(_config: &WindowConfig, _app: &mut dyn WindowApp) -> Result<(), WindowError> {
    Err(WindowError::LoopUnavailable {
        message: "the Android loop exists only around the activity handle; \
                  enter through window::android::run_window_app_android"
            .to_string(),
    })
}

/// Tell an application it has been interrupted, once per interruption.
///
/// A free function for the reason [`close_epoch`] is one: the adapter's
/// handlers take an `ActiveEventLoop`, which no test can build, so any
/// rule living only on that path is a rule no test can reach. The state
/// is passed in rather than owned so this is the whole transition and
/// the caller keeps none of it.
///
/// **Idempotent, and that is not symmetry for its own sake.** Android
/// emits back-to-back suspends — the epoch's own close tolerates a
/// redundant one for exactly that reason — and an application told
/// twice would pause a clock twice, or unbalance a refcount it had
/// taken once. A second suspend without an intervening resume is the
/// same interruption still in progress, so it is not news.
fn note_suspend(was_suspended: &mut bool, app: &mut dyn WindowApp) {
    if !*was_suspended {
        *was_suspended = true;
        app.suspended();
    }
}

/// Tell an application it is back, if it ever left.
///
/// **Only a real return counts as one.** The windowing stack calls its
/// resume handler once on the way up as well, and an application told
/// it had "resumed" before it ever ran would have to second-guess the
/// word. The flag is what makes the callback mean what its name says.
fn note_resume(was_suspended: &mut bool, app: &mut dyn WindowApp) {
    if *was_suspended {
        *was_suspended = false;
        app.resumed();
    }
}

/// The loop's second half, shared by every entry point: how the loop
/// was *built* differs per platform (a plain `new` on desktop, around
/// an activity handle on Android), but what happens once it exists must
/// not.
///
/// One platform disagrees about the *ending* rather than the beginning:
/// on iOS `run_app` enters `UIApplicationMain` and never comes back, so
/// [`Adapter::outcome`] below is desktop-and-Android-only in practice.
/// It is left in one shared path rather than split, because a second
/// copy of the loop body would be a second place for the two to drift,
/// and the difference is that one of them has no line after the call.
fn drive(
    event_loop: EventLoop<()>,
    config: &WindowConfig,
    app: &mut dyn WindowApp,
) -> Result<(), WindowError> {
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut adapter = Adapter {
        config,
        app,
        epoch: SurfaceEpoch::new(),
        failure: None,
        was_suspended: false,
        commanding: false,
        cursor_wanted: core::cell::Cell::new(false),
    };
    let run = event_loop.run_app(&mut adapter);
    adapter.outcome(run)
}

/// The Android doorway: the entry seam for a process the OS enters
/// through `android_main` rather than `main`.
#[cfg(target_os = "android")]
pub mod android {
    use winit::event_loop::EventLoop;
    use winit::platform::android::EventLoopBuilderExtAndroid;
    /// The activity handle the OS glue passes to `android_main` — the
    /// Android spelling of `main`'s arguments.
    ///
    /// **The documented exception to "no windowing-library type crosses
    /// this boundary."** The value exists before any engine code runs
    /// and only the entry point touches it: an application receives it
    /// in `android_main`, hands it here, and never names it again.
    pub use winit::platform::android::activity::AndroidApp;

    use super::{WindowApp, WindowConfig, WindowError, drive};

    /// Android's [`run_window_app`](super::run_window_app): the same
    /// adapter and the same contract, with the loop built around the
    /// activity handle — without which the windowing stack refuses to
    /// build one at all.
    ///
    /// # Errors
    ///
    /// [`WindowError::LoopUnavailable`] when the loop cannot be built;
    /// [`WindowError::Loop`] when the running loop reports failure.
    pub fn run_window_app_android(
        activity: AndroidApp,
        config: &WindowConfig,
        app: &mut dyn WindowApp,
    ) -> Result<(), WindowError> {
        let event_loop = EventLoop::builder()
            .with_android_app(activity)
            .build()
            .map_err(|error| WindowError::LoopUnavailable {
                message: error.to_string(),
            })?;
        drive(event_loop, config, app)
    }
}

/// The window's tenure between the platform granting it and taking it
/// away — the state the adapter keeps per surface epoch.
///
/// Extracted so its rules — the app is notified *before* the window
/// drops, a close with nothing open notifies nobody, an open into a
/// live epoch is a bug, a released epoch verifies that every clone was
/// dropped — are driven by tests directly: a real window cannot be
/// constructed without a running OS loop, and the rules must not go
/// untested for that reason. Generic for the same purpose the
/// [`WindowSource`] trait object serves — the tested path and the real
/// path are literally the same instructions.
struct SurfaceEpoch<W> {
    window: Option<std::sync::Arc<W>>,
}

impl<W> SurfaceEpoch<W> {
    const fn new() -> Self {
        Self { window: None }
    }

    /// The live window, while an epoch is open.
    fn window(&self) -> Option<&std::sync::Arc<W>> {
        self.window.as_ref()
    }

    /// Open an epoch around a freshly created window.
    fn open(&mut self, window: std::sync::Arc<W>) {
        debug_assert!(
            self.window.is_none(),
            "an epoch must close before the next opens"
        );
        self.window = Some(window);
    }

    /// Close the epoch: tell the app first, verify every clone was
    /// released, then drop the platform's own reference — in that
    /// order, because [`WindowApp::surface_lost`] is the app's only
    /// chance to release before the OS invalidates what the clones
    /// point at. The call happens *here*, inside the tested machine,
    /// so the unit tests drive the exact instruction the live suspend
    /// path executes rather than a stand-in closure.
    /// Returns whether there was an epoch to close: a redundant close
    /// is tolerated silently and notifies nobody, because Android may
    /// emit back-to-back suspends.
    fn close(&mut self, app: &mut dyn WindowApp) -> bool {
        let Some(window) = self.window.take() else {
            return false;
        };
        app.surface_lost();
        debug_assert!(
            std::sync::Arc::strong_count(&window) == 1,
            "surface_lost returned while a NativeWindow clone (or a renderer \
             target holding one) was still alive; every window-derived value \
             must be dropped before the callback returns"
        );
        drop(window);
        true
    }
}

/// Everything a suspend does, in one function generic over the window
/// type — the whole reason it is not a method on the adapter. The
/// adapter's epoch is concretely a winit window, which no unit test can
/// construct, so any instruction living only on the adapter's path is
/// an instruction no test can reach; this function is shared between
/// that path and the tests, which drive it with a plain `W` and an
/// epoch genuinely open (the [`WindowSource`] precedent again).
fn close_epoch<W>(epoch: &mut SurfaceEpoch<W>, app: &mut dyn WindowApp, commanding: &mut bool) {
    epoch.close(app);
    // A modifier held across the suspend releases while no window
    // exists, so its release event can never arrive — a remembered
    // press with a dead invalidation channel would silently swallow
    // the next epoch's first keystrokes as shortcuts. The cursor
    // request survives on purpose (it is the app's intent, reapplied
    // on the new window); this is derived OS state, so it resets.
    *commanding = false;
}

/// The bridge between the OS loop and the engine app.
struct Adapter<'a> {
    config: &'a WindowConfig,
    app: &'a mut dyn WindowApp,
    epoch: SurfaceEpoch<winit::window::Window>,
    /// Failure inside a callback (window creation): carried out of the
    /// loop so it surfaces through `run_window_app`'s Result instead of
    /// a log line.
    failure: Option<String>,
    /// Whether the platform has suspended this application since it
    /// last had the foreground.
    ///
    /// The windowing stack calls its resume handler once on the way up
    /// too, so without this a launch would be reported to the
    /// application as a return from somewhere it had never been.
    was_suspended: bool,
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
    /// Bring the window up and hand it to the app — opening a surface
    /// epoch.
    ///
    /// Does nothing while an epoch is open — a resume with a live
    /// window (desktop platforms can emit one) must never recreate it;
    /// recreation happens only after the platform closed the previous
    /// epoch through [`Adapter::close_surface`]. And nothing after a
    /// refusal, which is final: the loop is on its way out and the
    /// reason already recorded is the one to report.
    fn open(&mut self, create: &WindowSource<'_>) {
        if self.epoch.window().is_some() || self.failure.is_some() {
            return;
        }
        // **The opening size is raised to the floor here, so that every
        // platform opens the same window.** Left to the backends they
        // disagree: two of the three clamp the requested size up to the
        // minimum and the third sets the minimum as a hint and opens at
        // whatever was asked for. A caller who writes a floor larger
        // than the size it opens at has contradicted itself, and the
        // honest reading of "no smaller than this" is that the window is
        // not smaller than this — on every machine.
        let floor = (
            floor_of(self.config.min_logical_width),
            floor_of(self.config.min_logical_height),
        );
        let mut attributes = winit::window::Window::default_attributes()
            .with_title(&self.config.title)
            .with_resizable(self.config.resizable)
            .with_inner_size(winit::dpi::LogicalSize::new(
                self.config.logical_width.max(floor.0),
                self.config.logical_height.max(floor.1),
            ));
        if floor.0 > 0.0 || floor.1 > 0.0 {
            attributes =
                attributes.with_min_inner_size(winit::dpi::LogicalSize::new(floor.0, floor.1));
        }
        // The icon is asked for here rather than held on the config
        // because it is the application's picture; see `WindowApp::icon`.
        // Its bytes were checked against its dimensions when it was
        // built, so the only way the conversion below can refuse is a
        // rule this seam does not know about - and a window that opens
        // without its icon is worth more than one that does not open.
        if let Some(icon) = self.app.icon() {
            let (width, height) = (icon.width(), icon.height());
            if let Ok(icon) = winit::window::Icon::from_rgba(icon.rgba().to_vec(), width, height) {
                attributes = attributes.with_window_icon(Some(icon));
            }
        }
        match create(attributes) {
            Ok(window) => {
                let window = std::sync::Arc::new(window);
                self.app.ready(&WindowRef {
                    window: &window,
                    cursor_wanted: &self.cursor_wanted,
                });
                self.epoch.open(window);
            }
            Err(message) => {
                self.failure = Some(format!("window creation failed: {message}"));
            }
        }
    }

    /// Close the surface epoch: the platform is taking the window away.
    /// The app hears [`WindowApp::surface_lost`] first and must release
    /// every window-derived value; the release is verified before the
    /// platform's own reference drops. With no epoch open this is a
    /// tolerated no-op that reaches no app — Android may emit redundant
    /// suspends.
    fn close_surface(&mut self) {
        let Self {
            epoch,
            app,
            commanding,
            ..
        } = self;
        close_epoch(epoch, &mut **app, commanding);
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
        self.epoch
            .window()
            .is_some_and(|window| grab_on(window, held))
    }

    /// One loop iteration's app work, after the events. Returns whether
    /// the loop must now leave: because the app asked, or because a
    /// window refused to come up — on the first epoch the app never saw
    /// `ready` and must not see `update` either; on a later epoch the
    /// refusal is just as final, and a loop that cannot show anything
    /// again has nothing left to wait for.
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
        if let Some(filling) = control.fullscreen
            && let Some(window) = self.epoch.window()
        {
            // `None` is the windowing library's "the monitor this window
            // is on", which is the only answer available without naming
            // one — and naming one would mean a handle crossing this
            // seam.
            window.set_fullscreen(filling.then_some(winit::window::Fullscreen::Borderless(None)));
        }
        if control.redraw
            && let Some(window) = self.epoch.window()
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
        // The suspend handler may have parked the loop; a resume is the
        // platform granting a window back, and the poll cadence returns
        // with it. Idempotent on desktop, where nothing ever parked it.
        event_loop.set_control_flow(ControlFlow::Poll);
        note_resume(&mut self.was_suspended, self.app);
        self.open(&|attributes| {
            event_loop
                .create_window(attributes)
                .map_err(|error| error.to_string())
        });
    }

    /// A suspend means different things on the platforms that emit one,
    /// and only Android's revokes the window: there the surface dies
    /// with the backgrounded activity and every render surface must be
    /// dropped before this returns, so the epoch closes here, inside
    /// the callback. iOS also fires this — on *every* interruption (an
    /// incoming call, the app switcher, a pulled-down notification
    /// shade) — with the surface still valid, so closing the epoch
    /// there would tear a live renderer down many times per session;
    /// iOS keeps its window until its lifecycle gets its own deliberate
    /// treatment. Desktop platforms never call this at all. `cfg!`
    /// rather than an attribute so every target compiles every path —
    /// the branch is decided at compile time either way.
    fn suspended(&mut self, event_loop: &ActiveEventLoop) {
        // Told first, and on every platform that emits a suspend,
        // because "you have been interrupted" is true everywhere this
        // fires — what differs is only whether the window survives it.
        note_suspend(&mut self.was_suspended, self.app);

        if cfg!(target_os = "android") {
            self.close_surface();
            // A backgrounded app with no window has nothing to draw
            // and nobody watching: parking the loop here is what stops
            // `update` spinning at full poll speed in a pocket until
            // the OS kills the process. Events still wake it; the
            // resume above restores the cadence with the window.
            event_loop.set_control_flow(ControlFlow::Wait);
        }
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
        We::Touch(touch) => WindowEvent::Touch {
            finger: touch.id,
            phase: translate_touch_phase(touch.phase),
            x: touch.location.x,
            y: touch.location.y,
        },
        _ => return None,
    };
    Some(translated)
}

/// One phase for each of the windowing library's four. Two payload
/// fields are deliberately not carried, named here so neither drop is
/// implicit: pressure (platform-variant, no consumer) and the device
/// id — which no event at this seam keeps, the seam-wide convention,
/// so finger identity is per-window rather than per-device.
fn translate_touch_phase(phase: winit::event::TouchPhase) -> crate::event::TouchPhase {
    use winit::event::TouchPhase as Wp;
    match phase {
        Wp::Started => crate::event::TouchPhase::Started,
        Wp::Moved => crate::event::TouchPhase::Moved,
        Wp::Ended => crate::event::TouchPhase::Ended,
        Wp::Cancelled => crate::event::TouchPhase::Cancelled,
    }
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
            Wk::KeyB => KeyCode::KeyB,
            Wk::KeyC => KeyCode::KeyC,
            Wk::KeyE => KeyCode::KeyE,
            Wk::KeyF => KeyCode::KeyF,
            Wk::KeyG => KeyCode::KeyG,
            Wk::KeyH => KeyCode::KeyH,
            Wk::KeyI => KeyCode::KeyI,
            Wk::KeyJ => KeyCode::KeyJ,
            Wk::KeyK => KeyCode::KeyK,
            Wk::KeyL => KeyCode::KeyL,
            Wk::KeyM => KeyCode::KeyM,
            Wk::KeyN => KeyCode::KeyN,
            Wk::KeyO => KeyCode::KeyO,
            Wk::KeyP => KeyCode::KeyP,
            Wk::KeyQ => KeyCode::KeyQ,
            Wk::KeyR => KeyCode::KeyR,
            Wk::KeyT => KeyCode::KeyT,
            Wk::KeyU => KeyCode::KeyU,
            Wk::KeyV => KeyCode::KeyV,
            Wk::KeyX => KeyCode::KeyX,
            Wk::KeyY => KeyCode::KeyY,
            Wk::KeyZ => KeyCode::KeyZ,
            Wk::Digit0 => KeyCode::Digit0,
            Wk::Digit1 => KeyCode::Digit1,
            Wk::Digit2 => KeyCode::Digit2,
            Wk::Digit3 => KeyCode::Digit3,
            Wk::Digit4 => KeyCode::Digit4,
            Wk::Digit5 => KeyCode::Digit5,
            Wk::Digit6 => KeyCode::Digit6,
            Wk::Digit7 => KeyCode::Digit7,
            Wk::Digit8 => KeyCode::Digit8,
            Wk::Digit9 => KeyCode::Digit9,
            Wk::F1 => KeyCode::F1,
            Wk::F2 => KeyCode::F2,
            Wk::F3 => KeyCode::F3,
            Wk::F4 => KeyCode::F4,
            Wk::F5 => KeyCode::F5,
            Wk::F6 => KeyCode::F6,
            Wk::F7 => KeyCode::F7,
            Wk::F8 => KeyCode::F8,
            Wk::F9 => KeyCode::F9,
            Wk::F10 => KeyCode::F10,
            Wk::F11 => KeyCode::F11,
            Wk::F12 => KeyCode::F12,
            Wk::ShiftLeft => KeyCode::ShiftLeft,
            Wk::ShiftRight => KeyCode::ShiftRight,
            Wk::ControlLeft => KeyCode::ControlLeft,
            Wk::ControlRight => KeyCode::ControlRight,
            Wk::AltLeft => KeyCode::AltLeft,
            Wk::AltRight => KeyCode::AltRight,
            Wk::PageUp => KeyCode::PageUp,
            Wk::PageDown => KeyCode::PageDown,
            Wk::Insert => KeyCode::Insert,
            Wk::Minus => KeyCode::Minus,
            Wk::Equal => KeyCode::Equal,
            Wk::BracketLeft => KeyCode::BracketLeft,
            Wk::BracketRight => KeyCode::BracketRight,
            Wk::Semicolon => KeyCode::Semicolon,
            Wk::Quote => KeyCode::Quote,
            Wk::Comma => KeyCode::Comma,
            Wk::Period => KeyCode::Period,
            Wk::Slash => KeyCode::Slash,
            Wk::Backslash => KeyCode::Backslash,
            Wk::Backquote => KeyCode::Backquote,
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

    /// An icon is its bytes and its dimensions agreeing.
    ///
    /// **The mistake this type exists to catch is four bytes a pixel**,
    /// which every caller assumes and none of them states. A window is
    /// created once, inside a platform callback, at the one moment
    /// there is nowhere to report a bad picture to — so the arithmetic
    /// is done where the caller still holds the error, and the wrong
    /// count has to be refused there rather than dropped there.
    ///
    /// Probed by comparing against `width * height` rather than
    /// `width * height * 4`: a quarter-sized buffer is accepted and the
    /// first case fails.
    #[test]
    fn an_icon_is_refused_unless_its_bytes_match_its_size() {
        use super::{IconError, WindowIcon};

        let square = WindowIcon::from_rgba(2, 2, vec![0; 16]).expect("four pixels, sixteen bytes");
        assert_eq!(square.width(), 2);
        assert_eq!(square.height(), 2);
        assert_eq!(square.rgba().len(), 16);

        // A rectangle, so that a rule which multiplied one side by
        // itself would be caught.
        assert!(WindowIcon::from_rgba(4, 2, vec![0; 32]).is_ok());
        assert_eq!(
            WindowIcon::from_rgba(4, 2, vec![0; 16]),
            Err(IconError::WrongLength {
                expected: 32,
                found: 16
            })
        );
        assert_eq!(
            WindowIcon::from_rgba(2, 2, vec![0; 17]),
            Err(IconError::WrongLength {
                expected: 16,
                found: 17
            })
        );
        for (width, height) in [(0, 4), (4, 0), (0, 0)] {
            assert_eq!(
                WindowIcon::from_rgba(width, height, Vec::new()),
                Err(IconError::Empty),
                "{width}x{height} is not a picture"
            );
        }
    }

    /// Every refusal says which one it is and carries its numbers.
    ///
    /// **A message that does not name the sizes is a message that sends
    /// somebody to count bytes by hand.** The whole value of validating
    /// an icon where the caller is standing is that the caller can be
    /// told what went wrong; a `Display` that said "bad icon" would
    /// throw that away at the last step.
    ///
    /// Probed by printing the same sentence for both variants: the two
    /// assertions collapse onto each other and the second fails.
    #[test]
    fn an_icon_refusal_names_itself_and_its_numbers() {
        use super::IconError;

        assert_eq!(IconError::Empty.to_string(), "an icon with no pixels in it");
        let wrong = IconError::WrongLength {
            expected: 64,
            found: 16,
        }
        .to_string();
        assert!(wrong.contains("64"), "the wanted size is missing: {wrong}");
        assert!(wrong.contains("16"), "the found size is missing: {wrong}");
        assert_ne!(
            wrong,
            IconError::Empty.to_string(),
            "two different refusals read the same"
        );
    }

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
            (Wk::KeyB, KeyCode::KeyB),
            (Wk::KeyC, KeyCode::KeyC),
            (Wk::KeyE, KeyCode::KeyE),
            (Wk::KeyF, KeyCode::KeyF),
            (Wk::KeyG, KeyCode::KeyG),
            (Wk::KeyH, KeyCode::KeyH),
            (Wk::KeyI, KeyCode::KeyI),
            (Wk::KeyJ, KeyCode::KeyJ),
            (Wk::KeyK, KeyCode::KeyK),
            (Wk::KeyL, KeyCode::KeyL),
            (Wk::KeyM, KeyCode::KeyM),
            (Wk::KeyN, KeyCode::KeyN),
            (Wk::KeyO, KeyCode::KeyO),
            (Wk::KeyP, KeyCode::KeyP),
            (Wk::KeyQ, KeyCode::KeyQ),
            (Wk::KeyR, KeyCode::KeyR),
            (Wk::KeyT, KeyCode::KeyT),
            (Wk::KeyU, KeyCode::KeyU),
            (Wk::KeyV, KeyCode::KeyV),
            (Wk::KeyX, KeyCode::KeyX),
            (Wk::KeyY, KeyCode::KeyY),
            (Wk::KeyZ, KeyCode::KeyZ),
            (Wk::Digit0, KeyCode::Digit0),
            (Wk::Digit1, KeyCode::Digit1),
            (Wk::Digit2, KeyCode::Digit2),
            (Wk::Digit3, KeyCode::Digit3),
            (Wk::Digit4, KeyCode::Digit4),
            (Wk::Digit5, KeyCode::Digit5),
            (Wk::Digit6, KeyCode::Digit6),
            (Wk::Digit7, KeyCode::Digit7),
            (Wk::Digit8, KeyCode::Digit8),
            (Wk::Digit9, KeyCode::Digit9),
            (Wk::F1, KeyCode::F1),
            (Wk::F2, KeyCode::F2),
            (Wk::F3, KeyCode::F3),
            (Wk::F4, KeyCode::F4),
            (Wk::F5, KeyCode::F5),
            (Wk::F6, KeyCode::F6),
            (Wk::F7, KeyCode::F7),
            (Wk::F8, KeyCode::F8),
            (Wk::F9, KeyCode::F9),
            (Wk::F10, KeyCode::F10),
            (Wk::F11, KeyCode::F11),
            (Wk::F12, KeyCode::F12),
            (Wk::ShiftLeft, KeyCode::ShiftLeft),
            (Wk::ShiftRight, KeyCode::ShiftRight),
            (Wk::ControlLeft, KeyCode::ControlLeft),
            (Wk::ControlRight, KeyCode::ControlRight),
            (Wk::AltLeft, KeyCode::AltLeft),
            (Wk::AltRight, KeyCode::AltRight),
            (Wk::PageUp, KeyCode::PageUp),
            (Wk::PageDown, KeyCode::PageDown),
            (Wk::Insert, KeyCode::Insert),
            (Wk::Minus, KeyCode::Minus),
            (Wk::Equal, KeyCode::Equal),
            (Wk::BracketLeft, KeyCode::BracketLeft),
            (Wk::BracketRight, KeyCode::BracketRight),
            (Wk::Semicolon, KeyCode::Semicolon),
            (Wk::Quote, KeyCode::Quote),
            (Wk::Comma, KeyCode::Comma),
            (Wk::Period, KeyCode::Period),
            (Wk::Slash, KeyCode::Slash),
            (Wk::Backslash, KeyCode::Backslash),
            (Wk::Backquote, KeyCode::Backquote),
        ];
        for (winit_key, engine_key) in mapped {
            assert_eq!(
                translate_key(PhysicalKey::Code(winit_key)),
                engine_key,
                "{winit_key:?}"
            );
        }
        // The numpad is deliberately outside the vocabulary; its plus
        // key stands for everything unmapped.
        assert_eq!(
            translate_key(PhysicalKey::Code(Wk::NumpadAdd)),
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

    /// A touch translates whole — finger identity, phase, and position —
    /// and the pressure field is dropped by the named arm, not lost.
    /// Driven through the full event path because, unlike a key event,
    /// the windowing library's touch payload is constructible.
    #[test]
    fn a_touch_translates_with_its_finger_identity_and_phase() {
        use winit::event::{DeviceId, Touch, TouchPhase as Wp, WindowEvent as We};
        let touched = translate_event(&We::Touch(Touch {
            device_id: DeviceId::dummy(),
            phase: Wp::Started,
            location: winit::dpi::PhysicalPosition::new(120.5, 96.25),
            force: None,
            id: 7,
        }));
        assert_eq!(
            touched,
            Some(WindowEvent::Touch {
                finger: 7,
                phase: TouchPhase::Started,
                x: 120.5,
                y: 96.25,
            })
        );
        // Every phase maps to its namesake; four in, four out, no fold.
        for (theirs, ours) in [
            (Wp::Started, TouchPhase::Started),
            (Wp::Moved, TouchPhase::Moved),
            (Wp::Ended, TouchPhase::Ended),
            (Wp::Cancelled, TouchPhase::Cancelled),
        ] {
            assert_eq!(translate_touch_phase(theirs), ours);
        }
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

    /// **A fullscreen request survives the iteration it was made in and
    /// reaches the window, and asking nothing reaches nothing.**
    ///
    /// The window itself cannot be asserted on — there is none under a
    /// unit test — so what is checked is that the request is *taken*:
    /// the loop reads it, evaluates whether there is a window to hand it
    /// to, and does not fall over when there is not. That last part is
    /// the whole of what a headless iteration can prove, and it is worth
    /// proving: an application that asks for the screen before a window
    /// exists must be an ordinary iteration rather than a panic.
    ///
    /// **Unlike the cursor, nothing is remembered**, and that asymmetry
    /// is deliberate — see `LoopControl::set_fullscreen`. So there is no
    /// memory to assert against, and the absence of one is the claim.
    #[test]
    fn a_fullscreen_request_is_taken_even_with_no_window_to_apply_it_to() {
        let config = WindowConfig::default();

        for asked in [Some(true), Some(false), None] {
            let mut app = Recorder {
                ask_fullscreen: asked,
                ..Recorder::default()
            };
            {
                let mut adapter = new_adapter(&config, &mut app);
                assert!(
                    !adapter.tick(),
                    "asking about the screen ({asked:?}) was read as asking to leave the loop"
                );
                assert!(
                    !adapter.cursor_wanted.get(),
                    "asking about the screen ({asked:?}) also took the cursor"
                );
            }
            assert_eq!(
                app.updates, 1,
                "the iteration did not reach the application"
            );
        }
    }

    /// The accessors report the fields the loop reads, and a silent
    /// iteration is distinguishable from one that asked for something.
    ///
    /// **Two of the four are three-valued and two are not.** Saying
    /// nothing about the cursor leaves the grab alone, which is a
    /// different instruction from asking for it to be released — so a
    /// reader that collapsed `None` into `false` would turn every quiet
    /// iteration into a release request. Fullscreen is the same shape
    /// for the same reason, and is checked here rather than in a test of
    /// its own because what is being asserted is a property of this
    /// type: every request is independent, and a quiet iteration asks
    /// for nothing.
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

        // **Fullscreen is three-valued on the same terms**, and saying
        // nothing about it must not read as asking for a window back —
        // which is the mistake the cursor's own doc above warns of, in
        // the one other place this type can make it.
        assert_eq!(
            quiet.fullscreen_request(),
            None,
            "saying nothing about the screen is not asking for a window back"
        );
        let mut screen = LoopControl::default();
        screen.set_fullscreen(true);
        assert_eq!(screen.fullscreen_request(), Some(true));
        screen.set_fullscreen(false);
        assert_eq!(
            screen.fullscreen_request(),
            Some(false),
            "the last word in an iteration must win, as it does for the cursor"
        );

        // The three requests are independent: asking for one must not
        // set another. A single `Option` reused for two questions would
        // pass every assertion above and fail this one.
        let mut only_screen = LoopControl::default();
        only_screen.set_fullscreen(true);
        assert_eq!(
            only_screen.cursor_request(),
            None,
            "asking for the screen also asked something of the cursor"
        );
        assert!(
            !only_screen.exiting() && !only_screen.redraw_requested(),
            "asking for the screen also asked to exit or to redraw"
        );
        let mut only_cursor = LoopControl::default();
        only_cursor.hold_cursor(true);
        assert_eq!(
            only_cursor.fullscreen_request(),
            None,
            "asking for the cursor also asked something of the screen"
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
            keyboard(
                PhysicalKey::Code(Wk::NumpadAdd),
                ElementState::Released,
                false
            ),
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
        /// How many times the loop said the surface was lost.
        losses: u32,
        ask_redraw: bool,
        ask_exit: bool,
        /// What to ask of the cursor, if anything. `None` asks nothing,
        /// which is the case that must leave the grab alone.
        ask_cursor: Option<bool>,
        /// What to ask about filling the screen, if anything.
        ask_fullscreen: Option<bool>,
    }

    /// **An application that says nothing about an icon has none.**
    ///
    /// The default is what keeps the seam an addition rather than a
    /// break: an implementation written before the method existed has to
    /// keep compiling and keep behaving. Asked of a double that was
    /// written before it, which is the strongest form of that claim
    /// available here.
    ///
    /// Probed by defaulting the trait method to a picture: a recorder
    /// that has never mentioned an icon starts carrying one.
    #[test]
    fn an_application_that_says_nothing_carries_no_icon() {
        assert_eq!(Recorder::default().icon(), None);
    }

    impl WindowApp for Recorder {
        // Unreachable from here, and nothing can change that: a
        // `WindowRef` borrows a live OS window, which needs a running
        // event loop no unit test can host. The callback's own contract
        // — once per surface epoch, before the rest — is proven by the
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
            if let Some(filling) = self.ask_fullscreen {
                control.set_fullscreen(filling);
            }
        }

        fn surface_lost(&mut self) {
            self.losses += 1;
        }
    }

    /// An adapter over a fresh app, in the state the loop starts in.
    fn new_adapter<'a>(config: &'a WindowConfig, app: &'a mut Recorder) -> Adapter<'a> {
        Adapter {
            config,
            app,
            epoch: SurfaceEpoch::new(),
            failure: None,
            was_suspended: false,
            commanding: false,
            cursor_wanted: core::cell::Cell::new(false),
        }
    }

    /// Build an adapter, open one window through a source that records
    /// what it was handed, and give the attributes back.
    ///
    /// **The call is counted, and every caller asserts on the count.**
    /// A test whose assertions all live inside the source closure
    /// passes, having run none of them, the moment a change stops
    /// opening the window at all. The refusal test below had that guard
    /// already; the first draft of the ones above had dropped it.
    fn attributes_for(config: &WindowConfig) -> winit::window::WindowAttributes {
        let seen = core::cell::RefCell::new(None);
        let calls = core::cell::Cell::new(0_u32);
        let source: &WindowSource<'_> = &|attributes| {
            calls.set(calls.get() + 1);
            *seen.borrow_mut() = Some(attributes);
            Err("no display".to_string())
        };
        let mut app = Recorder::default();
        new_adapter(config, &mut app).open(source);
        assert_eq!(calls.get(), 1, "the window was never asked for");
        seen.into_inner()
            .unwrap_or_else(winit::window::Window::default_attributes)
    }

    /// A logical size as the attributes carry it, for comparing.
    fn logical(width: f64, height: f64) -> winit::dpi::Size {
        winit::dpi::LogicalSize::new(width, height).into()
    }

    /// **A floor reaches the window, and no floor reaches it as none.**
    ///
    /// Two separate claims: that a floor asked for is passed on, and
    /// that a window whose caller said nothing is not given one anyway.
    /// The second is what every caller written before this field existed
    /// relies on.
    ///
    /// **The two dimensions are varied independently.** Setting both and
    /// then neither leaves a whole class of mutant alive: nesting one
    /// field's branch inside the other's passes a both-or-neither test
    /// while silently dropping the floor for anyone who sets one.
    #[test]
    fn a_windows_floor_reaches_the_window_and_its_absence_reaches_it_as_absence() {
        let asked = |width: f64, height: f64| WindowConfig {
            title: "renew-floored".to_string(),
            logical_width: 900.0,
            logical_height: 700.0,
            resizable: true,
            min_logical_width: width,
            min_logical_height: height,
        };
        assert_eq!(
            attributes_for(&asked(320.0, 240.0)).min_inner_size,
            Some(logical(320.0, 240.0)),
            "a floor in both dimensions did not reach the window"
        );
        assert_eq!(
            attributes_for(&asked(320.0, 0.0)).min_inner_size,
            Some(logical(320.0, 0.0)),
            "a floor on the width alone did not reach the window"
        );
        assert_eq!(
            attributes_for(&asked(0.0, 240.0)).min_inner_size,
            Some(logical(0.0, 240.0)),
            "a floor on the height alone did not reach the window"
        );
        assert_eq!(
            attributes_for(&asked(0.0, 0.0)).min_inner_size,
            None,
            "a window asked for no floor was given one"
        );
        assert_eq!(
            attributes_for(&WindowConfig::default()).min_inner_size,
            None,
            "the default carries a floor, so every caller that predates it has one"
        );
    }

    /// **Nonsense is read as no floor, and never handed on.**
    ///
    /// Not fussiness. The windowing library clamps the requested size
    /// between the floor and a ceiling, and that clamp asserts
    /// `min <= max` — so a floor of `NaN` or `INFINITY` fails an
    /// assertion inside a dependency, during window creation, in a seam
    /// whose whole character is that failures come back as a `Result`.
    /// This crate builds with `panic = "abort"`, so it would not even
    /// unwind.
    ///
    /// Negative is the quiet half of the same problem: it saturates to
    /// nought on the way to an unsigned pixel count, so it already meant
    /// no floor, silently. It means it out loud now.
    #[test]
    fn a_floor_that_is_not_a_size_is_no_floor() {
        for nonsense in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
            let config = WindowConfig {
                min_logical_width: nonsense,
                min_logical_height: nonsense,
                ..WindowConfig::default()
            };
            let attributes = attributes_for(&config);
            assert_eq!(
                attributes.min_inner_size, None,
                "a floor of {nonsense} reached the window instead of reading as no floor"
            );
            assert_eq!(
                attributes.inner_size,
                Some(logical(1280.0, 720.0)),
                "a floor of {nonsense} moved the size the window opens at"
            );
        }
    }

    /// **A floor above the opening size raises it here, rather than on
    /// two platforms out of three.**
    ///
    /// Left to the backends they disagree: two clamp the requested size
    /// up to the minimum, the third opens at what was asked and sets the
    /// minimum as a hint afterwards. A caller writing a floor larger
    /// than its opening size has contradicted itself, and the honest
    /// reading of "no smaller than this" is that the window is not
    /// smaller than this — on every machine.
    #[test]
    fn a_floor_above_the_opening_size_raises_it_on_every_platform() {
        let config = WindowConfig {
            logical_width: 640.0,
            logical_height: 480.0,
            min_logical_width: 1024.0,
            min_logical_height: 768.0,
            ..WindowConfig::default()
        };
        let attributes = attributes_for(&config);
        assert_eq!(
            attributes.inner_size,
            Some(logical(1024.0, 768.0)),
            "the window opens smaller than the floor it was given"
        );
        assert_eq!(
            attributes.min_inner_size,
            Some(logical(1024.0, 768.0)),
            "the floor was lost while the opening size was being raised"
        );
        // A floor under the opening size leaves it alone, which is the
        // ordinary case and the one the raise must not disturb.
        let ordinary = WindowConfig {
            logical_width: 900.0,
            logical_height: 700.0,
            min_logical_width: 320.0,
            min_logical_height: 240.0,
            ..WindowConfig::default()
        };
        assert_eq!(
            attributes_for(&ordinary).inner_size,
            Some(logical(900.0, 700.0)),
            "an ordinary floor moved the size the window opens at"
        );
    }

    /// **A floor is passed on whether or not the window can be resized.**
    ///
    /// This crate does not decide what a fixed-size window does with a
    /// floor — the platforms do, and they differ — but it must not
    /// quietly drop one on the way, which is the combination the field's
    /// own doc talks about and nothing checked.
    #[test]
    fn a_fixed_size_window_still_carries_the_floor_it_was_given() {
        let config = WindowConfig {
            resizable: false,
            min_logical_width: 320.0,
            min_logical_height: 240.0,
            ..WindowConfig::default()
        };
        let attributes = attributes_for(&config);
        assert!(
            !attributes.resizable,
            "the fixture stopped being fixed-size"
        );
        assert_eq!(
            attributes.min_inner_size,
            Some(logical(320.0, 240.0)),
            "a fixed-size window had its floor dropped on the way to the platform"
        );
    }

    /// **Nothing else the window was asked for moved.** The floor is one
    /// of five things this function sets, and a test reading only the
    /// one it changed cannot see the others being disturbed.
    #[test]
    fn adding_a_floor_disturbs_nothing_else_the_window_was_asked_for() {
        let config = WindowConfig {
            title: "renew-intact".to_string(),
            logical_width: 900.0,
            logical_height: 700.0,
            resizable: true,
            min_logical_width: 320.0,
            min_logical_height: 240.0,
        };
        let attributes = attributes_for(&config);
        assert_eq!(attributes.title, "renew-intact");
        assert!(attributes.resizable);
        assert_eq!(attributes.inner_size, Some(logical(900.0, 700.0)));
        assert_eq!(
            attributes.max_inner_size, None,
            "a ceiling appeared beside the floor, pinning the window to one size"
        );
        assert!(
            attributes.fullscreen.is_none(),
            "the window opened fullscreen, which nothing asked for"
        );
    }

    #[test]
    fn a_refused_window_is_reported_once_and_never_retried() {
        let config = WindowConfig {
            title: "renew-refused".to_string(),
            logical_width: 640.0,
            logical_height: 480.0,
            resizable: false,
            ..WindowConfig::default()
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

    /// **The app is told before the window drops, and the platform's
    /// own reference does not survive the close.** The order is the
    /// contract: `surface_lost` is the app's only chance to release
    /// clones of a window the OS is about to invalidate, so a close
    /// that dropped first would notify about a corpse. The app here is
    /// a real [`WindowApp`] heard through `surface_lost` itself — the
    /// same instruction the live suspend path executes.
    #[test]
    fn a_close_notifies_the_app_before_the_window_drops() {
        use std::sync::atomic::{AtomicBool, Ordering};
        struct Tattle(std::sync::Arc<AtomicBool>);
        impl Drop for Tattle {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Relaxed);
            }
        }
        /// Records what the world looked like when the loss arrived.
        struct Probe {
            dropped: std::sync::Arc<AtomicBool>,
            alive_at_notify: bool,
            losses: u32,
        }
        impl WindowApp for Probe {
            fn ready(&mut self, _window: &WindowRef<'_>) {}
            fn event(&mut self, _event: WindowEvent) {}
            fn update(&mut self, _control: &mut LoopControl) {}
            fn surface_lost(&mut self) {
                self.losses += 1;
                self.alive_at_notify = !self.dropped.load(Ordering::Relaxed);
            }
        }

        let dropped = std::sync::Arc::new(AtomicBool::new(false));
        let mut epoch = SurfaceEpoch::new();
        epoch.open(std::sync::Arc::new(Tattle(std::sync::Arc::clone(&dropped))));

        let mut probe = Probe {
            dropped: std::sync::Arc::clone(&dropped),
            alive_at_notify: false,
            losses: 0,
        };
        let closed = epoch.close(&mut probe);

        assert!(closed, "there was an epoch to close");
        assert_eq!(probe.losses, 1, "one epoch, one loss");
        assert!(
            probe.alive_at_notify,
            "the app must hear it while it can still act"
        );
        assert!(
            dropped.load(Ordering::Relaxed),
            "the platform reference must not outlive the close"
        );
        assert!(epoch.window().is_none(), "a closed epoch holds nothing");
    }

    /// A redundant close is tolerated silently — Android may emit
    /// back-to-back suspends — and tolerating it must not mean telling
    /// the app its surface was lost twice.
    #[test]
    fn a_redundant_close_is_tolerated_and_notifies_nobody() {
        let mut epoch: SurfaceEpoch<()> = SurfaceEpoch::new();
        let mut app = Recorder::default();
        assert!(!epoch.close(&mut app), "nothing was open");
        assert_eq!(app.losses, 0, "nobody to notify when nothing was open");
    }

    /// Recreation is the point of the epoch shape: after a close, the
    /// next open must succeed — that is a backgrounded app coming back.
    #[test]
    fn a_new_epoch_can_open_after_the_last_closed() {
        let mut epoch = SurfaceEpoch::new();
        let mut app = Recorder::default();
        epoch.open(std::sync::Arc::new(1_u8));
        assert!(epoch.close(&mut app));
        assert_eq!(app.losses, 1, "the first epoch's loss was heard");
        epoch.open(std::sync::Arc::new(2_u8));
        assert_eq!(
            epoch.window().map(|w| **w),
            Some(2),
            "the new window is live"
        );
    }

    /// An app that holds no window-derived values and takes the trait's
    /// defaulted `surface_lost` satisfies the release contract as-is —
    /// the default is a real answer for such apps, not a trap.
    #[test]
    fn the_defaulted_surface_lost_satisfies_the_release_contract() {
        struct Holdless;
        impl WindowApp for Holdless {
            fn ready(&mut self, _window: &WindowRef<'_>) {}
            fn event(&mut self, _event: WindowEvent) {}
            fn update(&mut self, _control: &mut LoopControl) {}
            // surface_lost deliberately not implemented: the default.
        }
        let mut epoch = SurfaceEpoch::new();
        epoch.open(std::sync::Arc::new(()));
        assert!(
            epoch.close(&mut Holdless),
            "a holdless app passes the release verification unchanged"
        );
    }

    /// The suspend transition, in every order a platform can produce.
    ///
    /// Drives the defaulted callbacks too: an application that
    /// implements neither is the common case, and a default that
    /// nothing ever calls is a default nobody has checked.
    #[test]
    fn an_application_hears_about_an_interruption_once_and_a_return_only_after_one() {
        #[derive(Default)]
        struct Counting {
            suspends: usize,
            resumes: usize,
        }
        impl WindowApp for Counting {
            fn ready(&mut self, _window: &WindowRef<'_>) {}
            fn event(&mut self, _event: WindowEvent) {}
            fn update(&mut self, _control: &mut LoopControl) {}
            fn suspended(&mut self) {
                self.suspends += 1;
            }
            fn resumed(&mut self) {
                self.resumes += 1;
            }
        }

        // A resume before anything suspended is the loop starting up,
        // not an application coming back.
        let mut flag = false;
        let mut app = Counting::default();
        note_resume(&mut flag, &mut app);
        assert_eq!(
            (app.suspends, app.resumes),
            (0, 0),
            "a launch is not a return"
        );

        note_suspend(&mut flag, &mut app);
        assert_eq!((app.suspends, app.resumes), (1, 0));

        // Android emits back-to-back suspends; the second is the same
        // interruption still in progress.
        note_suspend(&mut flag, &mut app);
        assert_eq!(
            (app.suspends, app.resumes),
            (1, 0),
            "a repeated suspend is not news"
        );

        note_resume(&mut flag, &mut app);
        assert_eq!((app.suspends, app.resumes), (1, 1));

        // And a second resume is not a second return.
        note_resume(&mut flag, &mut app);
        assert_eq!(
            (app.suspends, app.resumes),
            (1, 1),
            "a repeated resume is not news"
        );

        // The cycle repeats cleanly, which is what a lane counting
        // three of each depends on.
        note_suspend(&mut flag, &mut app);
        note_resume(&mut flag, &mut app);
        assert_eq!((app.suspends, app.resumes), (2, 2));
    }

    /// The defaulted pair is a real answer, not an unimplemented one:
    /// an application with no interest in interruptions must be able to
    /// ignore them, and this drives the bodies that let it.
    #[test]
    fn the_defaulted_interruption_callbacks_do_nothing_and_that_is_allowed() {
        struct Incurious;
        impl WindowApp for Incurious {
            fn ready(&mut self, _window: &WindowRef<'_>) {}
            fn event(&mut self, _event: WindowEvent) {}
            fn update(&mut self, _control: &mut LoopControl) {}
            // suspended and resumed deliberately not implemented.
        }
        let mut flag = false;
        note_suspend(&mut flag, &mut Incurious);
        assert!(
            flag,
            "the transition is recorded even when the app ignores it"
        );
        note_resume(&mut flag, &mut Incurious);
        assert!(!flag, "and cleared on the way back");
    }

    /// Opening into a live epoch is a bug in the loop, not a state to
    /// tolerate: the first window would leak unverified.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "an epoch must close before the next opens")]
    fn opening_into_a_live_epoch_is_a_contract_violation() {
        let mut epoch = SurfaceEpoch::new();
        epoch.open(std::sync::Arc::new(()));
        epoch.open(std::sync::Arc::new(()));
    }

    /// A clone surviving `surface_lost` is a dangling surface in
    /// waiting — the OS destroys what it points at regardless of the
    /// strong count — so the close treats it as a contract violation
    /// rather than trusting the release.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "surface_lost returned while")]
    fn a_clone_surviving_the_close_is_a_contract_violation() {
        let mut epoch = SurfaceEpoch::new();
        let window = std::sync::Arc::new(());
        epoch.open(std::sync::Arc::clone(&window));
        epoch.close(&mut Recorder::default());
        drop(window);
    }

    /// **A suspend forgets a held command modifier.** Its release event
    /// can only be delivered to a window, and the suspend takes the
    /// window away — a press remembered across the gap would swallow
    /// the next epoch's first keystrokes as shortcuts. The cursor
    /// request deliberately survives the same boundary: it is the
    /// app's intent, not the OS's state.
    #[test]
    fn a_suspend_forgets_a_held_modifier_and_keeps_the_cursor_request() {
        use winit::event::WindowEvent as We;
        use winit::keyboard::ModifiersState as Ms;

        let config = WindowConfig::default();
        let mut app = Recorder::default();
        let mut adapter = new_adapter(&config, &mut app);
        adapter.dispatch(&We::ModifiersChanged(Ms::CONTROL.into()));
        adapter.cursor_wanted.set(true);
        assert!(adapter.commanding, "the press was remembered");

        adapter.close_surface();
        assert!(
            !adapter.commanding,
            "the release can never arrive, so the press must not outlive the epoch"
        );
        assert!(
            adapter.cursor_wanted.get(),
            "the app's cursor intent survives to be reapplied on the next window"
        );
    }

    /// **The whole suspend, through the shared function, with an epoch
    /// genuinely open.** This is the test that owns the live path's
    /// instructions: the notification, the release verification, the
    /// drop, and the modifier reset all execute here on the exact code
    /// `close_surface` delegates to — severing any one of them turns
    /// this red. What remains outside every test is the chain above
    /// the shared function — the suspend handler's body, with the park
    /// it performs after the close, and `close_surface`'s own
    /// delegation — each pinned elsewhere: the modifier test fails if
    /// `close_surface` stops delegating, and the handler's body waits
    /// on an execution lane no desktop can host, whose first suspend
    /// cycle is its regression test.
    #[test]
    fn a_full_suspend_notifies_releases_and_forgets_the_modifier() {
        let mut epoch = SurfaceEpoch::new();
        let mut app = Recorder::default();
        let mut commanding = true;
        epoch.open(std::sync::Arc::new(7_u8));

        close_epoch(&mut epoch, &mut app, &mut commanding);

        assert_eq!(app.losses, 1, "the open epoch's loss must be heard");
        assert!(epoch.window().is_none(), "the window must not survive");
        assert!(!commanding, "a held modifier must not cross the epoch");
    }

    /// The adapter's close with no epoch open reaches no app — the
    /// redundant-suspend case, one level up through the real
    /// `close_surface`. (The open-epoch half of that path is owned by
    /// the shared-function test above, because a real window cannot
    /// exist under a unit test.)
    #[test]
    fn a_suspend_with_no_window_reaches_no_app() {
        let config = WindowConfig::default();
        let mut app = Recorder::default();
        let mut adapter = new_adapter(&config, &mut app);
        adapter.close_surface();
        adapter.close_surface();
        assert_eq!(app.losses, 0, "no epoch was open, so none was lost");
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
