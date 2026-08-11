//! The pause menu: the widget tree's first embedding.
//!
//! The menu owns a [`Ui`] — a real retained tree, solved by the
//! fixed-point solver, hit-tested by the same code every future
//! document will use — and the game folds the tree's decisions into
//! its reported digest, because a menu that can restart the run is
//! gameplay, not chrome. The world's own digest never learns the menu
//! exists; the *session's* digest is the fold of both, which is why
//! the reported hash moved when this file landed.
//!
//! **Event routing is the driver's, pausing is here.** The menu hears
//! every window event: the pause key toggles it, and pointer events
//! reach the tree only while it is open — quantized through the one
//! documented seam, so a recorded trace replays into the same
//! integers. The driver checks [`Menu::is_open`] *before* handing the
//! same event to gameplay input, so the click that presses Resume
//! never also flaps the bird.

use renew_event::{KeyCode, PointerButton, WindowEvent};
use renew_frame::StateHash;
use renew_math::quantize_pointer;
use renew_sample_glide_world::{VIEW_HEIGHT, VIEW_WIDTH};
use renew_ui::{
    Align, Direction, Fixed, NodeId, Size, Style, Ui, UiEvent, UiLimits, UiOutput, text,
};

/// A decision the menu made this frame, for the driver to act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    /// Play on: the menu has already closed itself.
    Resume,
    /// Start over from the same seed: the menu has closed, and the
    /// driver owns making the world new.
    Restart,
}

/// The pause menu: a tree, its two buttons, and whether it is shown.
#[derive(Debug)]
pub struct Menu {
    ui: Ui,
    resume: NodeId,
    restart: NodeId,
    open: bool,
}

/// Space around a button label, in pixels.
const PAD_X: i32 = 12;
const PAD_Y: i32 = 5;

impl Menu {
    /// Build and solve the tree once: the menu's layout is static,
    /// and the buttons size themselves from their labels through the
    /// same integer measurement the digest never fears.
    #[must_use]
    pub fn new() -> Self {
        let mut ui = Ui::new(UiLimits { nodes: 8 });
        let root = ui.root();
        ui.set_style(
            root,
            Style {
                direction: Direction::Column,
                justify: Align::Center,
                align_cross: Align::Center,
                gap: Fixed::from_int(8),
                ..Style::default()
            },
        );
        let resume = button(&mut ui, root, "Resume");
        let restart = button(&mut ui, root, "Restart");
        ui.solve(
            Fixed::from_int(i32::try_from(VIEW_WIDTH).unwrap_or(i32::MAX)),
            Fixed::from_int(i32::try_from(VIEW_HEIGHT).unwrap_or(i32::MAX)),
        );
        Self {
            ui,
            resume,
            restart,
            open: false,
        }
    }

    /// Whether the menu is currently shown — the driver's cue to
    /// pause the world and to route events here instead of gameplay.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Feed one window event. The pause key toggles visibility from
    /// either state; everything else reaches the tree only while the
    /// menu is open, as the integers the seam makes of it.
    pub fn handle(&mut self, event: &WindowEvent) {
        if let WindowEvent::Key {
            code: KeyCode::Escape,
            pressed: true,
            repeat: false,
        } = event
        {
            self.open = !self.open;
            // A press left dangling across a toggle must not pair
            // with a release from a later episode: park the pointer
            // outside the tree and release, which the tree treats as
            // an abandoned press — no activation, nothing pending.
            self.ui.handle(UiEvent::PointerMoved { x: -1, y: -1 });
            self.ui.handle(UiEvent::PointerReleased);
            return;
        }
        if !self.open {
            return;
        }
        match event {
            WindowEvent::PointerMoved { x, y } => {
                self.ui.handle(UiEvent::PointerMoved {
                    x: quantize_pointer(*x),
                    y: quantize_pointer(*y),
                });
            }
            WindowEvent::PointerButton {
                button: PointerButton::Left,
                pressed,
            } => {
                self.ui.handle(if *pressed {
                    UiEvent::PointerPressed
                } else {
                    UiEvent::PointerReleased
                });
            }
            _ => {}
        }
    }

    /// The decisions since the last drain, applied: an activated
    /// button closes the menu, and the caller learns what to do next.
    pub fn drain(&mut self) -> impl Iterator<Item = MenuAction> + '_ {
        let resume = self.resume;
        let restart = self.restart;
        let open = &mut self.open;
        self.ui.drain_outputs().filter_map(move |output| {
            let UiOutput::Activated(node) = output;
            if node == resume {
                *open = false;
                Some(MenuAction::Resume)
            } else if node == restart {
                *open = false;
                Some(MenuAction::Restart)
            } else {
                None
            }
        })
    }

    /// Fold the menu into a digest: whether it is open — a bit that
    /// decides whether the world steps, so it must be visible — then
    /// every discrete decision the tree holds.
    #[must_use]
    pub fn absorb(&self, hash: StateHash) -> StateHash {
        self.ui.absorb(hash.absorb_u32(u32::from(self.open)))
    }

    /// The tree, for presentation: the presenter snapshots it and the
    /// labels draw at its solved rectangles.
    #[must_use]
    pub fn ui(&self) -> &Ui {
        &self.ui
    }

    /// The buttons and their labels, in draw order, for the label
    /// pass: each label centres inside its button's solved rectangle.
    #[must_use]
    pub fn labels(&self) -> [(NodeId, &'static str); 2] {
        [(self.resume, "Resume"), (self.restart, "Restart")]
    }
}

impl Default for Menu {
    fn default() -> Self {
        Self::new()
    }
}

/// One button: sized from its label by the integer advance table,
/// padded, dark until the compiled style tables bring richer looks.
fn button(ui: &mut Ui, parent: NodeId, label: &str) -> NodeId {
    let node = ui.insert(parent).unwrap_or(parent);
    let width = text::measure(label) + Fixed::from_int(2 * PAD_X);
    let line = i32::try_from(text::LINE_HEIGHT).unwrap_or(16);
    let height = Fixed::from_int(line + 2 * PAD_Y);
    ui.set_style(
        node,
        Style {
            width: Size::Px(width),
            height: Size::Px(height),
            background: [40, 44, 52, 230],
            ..Style::default()
        },
    );
    node
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press_at(menu: &mut Menu, x: f64, y: f64) {
        menu.handle(&WindowEvent::PointerMoved { x, y });
        menu.handle(&WindowEvent::PointerButton {
            button: PointerButton::Left,
            pressed: true,
        });
        menu.handle(&WindowEvent::PointerButton {
            button: PointerButton::Left,
            pressed: false,
        });
    }

    fn centre_of(menu: &Menu, node: NodeId) -> (f64, f64) {
        let rect = menu.ui().rect(node).expect("solved");
        let x = rect.x + rect.width / Fixed::from_int(2);
        let y = rect.y + rect.height / Fixed::from_int(2);
        // A centre well inside a button fits an i32 with room to
        // spare; the widening to f64 is exact.
        let px = |value: Fixed| f64::from(i32::try_from(value.trunc_int()).unwrap_or(0));
        (px(x), px(y))
    }

    /// The pause key toggles; pointer events reach the tree only
    /// while open — a closed menu decides nothing however hard it is
    /// clicked.
    #[test]
    fn a_closed_menu_hears_only_the_pause_key() {
        let mut menu = Menu::new();
        let (x, y) = centre_of(&menu, menu.resume);
        press_at(&mut menu, x, y);
        assert_eq!(menu.drain().count(), 0, "a closed menu decides nothing");
        menu.handle(&WindowEvent::Key {
            code: KeyCode::Escape,
            pressed: true,
            repeat: false,
        });
        assert!(menu.is_open());
    }

    /// Clicking Resume closes and says so; clicking Restart closes
    /// and says so; the two are different decisions.
    #[test]
    fn the_buttons_decide_and_close() {
        let mut menu = Menu::new();
        menu.handle(&WindowEvent::Key {
            code: KeyCode::Escape,
            pressed: true,
            repeat: false,
        });
        let (x, y) = centre_of(&menu, menu.resume);
        press_at(&mut menu, x, y);
        let actions: Vec<_> = menu.drain().collect();
        assert_eq!(actions, vec![MenuAction::Resume]);
        assert!(!menu.is_open(), "an activated button closes the menu");

        menu.handle(&WindowEvent::Key {
            code: KeyCode::Escape,
            pressed: true,
            repeat: false,
        });
        let (x, y) = centre_of(&menu, menu.restart);
        press_at(&mut menu, x, y);
        let actions: Vec<_> = menu.drain().collect();
        assert_eq!(actions, vec![MenuAction::Restart]);
    }

    /// The digest sees the open bit and the decisions: pausing alone
    /// moves it, and the same session twice digests identically.
    #[test]
    fn the_menu_digests_its_decisions() {
        let closed = Menu::new().absorb(StateHash::new()).finish();
        let mut paused = Menu::new();
        paused.handle(&WindowEvent::Key {
            code: KeyCode::Escape,
            pressed: true,
            repeat: false,
        });
        assert_ne!(
            paused.absorb(StateHash::new()).finish(),
            closed,
            "whether the world steps must be visible in the digest"
        );
        let run = || {
            let mut menu = Menu::new();
            menu.handle(&WindowEvent::Key {
                code: KeyCode::Escape,
                pressed: true,
                repeat: false,
            });
            let (x, y) = centre_of(&menu, menu.resume);
            press_at(&mut menu, x, y);
            let _ = menu.drain().count();
            menu.absorb(StateHash::new()).finish()
        };
        assert_eq!(run(), run());
    }

    /// The default menu is the new menu: closed, solved, ready.
    #[test]
    fn the_default_menu_starts_closed() {
        assert!(!Menu::default().is_open());
    }

    /// A click on empty space activates the root, which is not a
    /// button: drain maps it to nothing, and the menu stays open.
    #[test]
    fn empty_space_decides_nothing() {
        let mut menu = Menu::new();
        menu.handle(&WindowEvent::Key {
            code: KeyCode::Escape,
            pressed: true,
            repeat: false,
        });
        press_at(&mut menu, 2.0, 2.0);
        assert_eq!(menu.drain().count(), 0, "the root decides nothing");
        assert!(
            menu.is_open(),
            "an empty-space click does not close the menu"
        );
    }
}
