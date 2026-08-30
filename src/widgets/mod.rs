//! Relm4 presentation components.

use std::rc::Rc;

use gtk4_layer_shell::{KeyboardMode, LayerShell};
use relm4::gtk;
use relm4::gtk::gdk;
use relm4::gtk::glib;
use relm4::gtk::prelude::*;

use crate::action::AppAction;
use crate::state::{OutputId, PresentationLevel};

pub mod active_context;
pub mod clock;
pub mod media;
pub mod panel;
pub mod ribbon;
pub mod selvage;

/// Fixed transparent layer-surface height in logical pixels.
pub const SURFACE_HEIGHT: i32 = 30;

/// Pointer-active collapsed Selvage height in logical pixels.
pub const SELVAGE_HEIGHT: i32 = 3;

const RIBBON_TRANSITION_MILLIS: u32 = 160;

/// Install the static semantic GTK stylesheet for native proof surfaces.
pub(crate) fn install_style(display: &gdk::Display) {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(include_str!("../../assets/style.css"));
    gtk::style_context_add_provider_for_display(
        display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

/// GTK objects rendered inside one fixed-height output surface.
pub(crate) struct TopEdgeWidgets {
    /// Root overlay assigned to the layer window.
    pub root: gtk::Overlay,
    revealer: gtk::Revealer,
    popover: gtk::Popover,
    first_panel_action: gtk::Button,
}

impl TopEdgeWidgets {
    /// Build a Selvage, Ribbon, and attached Panel for one output.
    pub fn new(
        window: &gtk::ApplicationWindow,
        output: OutputId,
        emit: Rc<dyn Fn(AppAction)>,
    ) -> Self {
        let root = gtk::Overlay::builder()
            .height_request(SURFACE_HEIGHT)
            .hexpand(true)
            .build();
        root.add_css_class("weftwise-root");

        let ribbon_button = gtk::Button::builder()
            .label("Weftwise")
            .height_request(SURFACE_HEIGHT)
            .hexpand(true)
            .focusable(true)
            .build();
        ribbon_button.add_css_class("weftwise-ribbon");

        let reveal_emit = emit.clone();
        ribbon_button.connect_clicked(move |_| reveal_emit(AppAction::OpenPanel(output)));

        let revealer = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .transition_duration(RIBBON_TRANSITION_MILLIS)
            .reveal_child(false)
            .child(&ribbon_button)
            .build();
        root.set_child(Some(&revealer));

        let selvage = gtk::Box::builder()
            .height_request(SELVAGE_HEIGHT)
            .hexpand(true)
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Start)
            .build();
        selvage.add_css_class("weftwise-selvage");
        root.add_overlay(&selvage);

        let panel_content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();

        let heading = gtk::Label::new(Some("Panel proof"));
        heading.set_xalign(0.0);
        heading.add_css_class("title-4");
        panel_content.append(&heading);

        let first_panel_action = gtk::Button::with_label("Workspace navigation placeholder");
        first_panel_action.set_focusable(true);
        panel_content.append(&first_panel_action);

        let close_button = gtk::Button::with_label("Close Panel");
        close_button.set_focusable(true);
        panel_content.append(&close_button);

        let popover = gtk::Popover::builder()
            .autohide(true)
            .has_arrow(false)
            .position(gtk::PositionType::Bottom)
            .child(&panel_content)
            .build();
        popover.add_css_class("weftwise-panel");
        popover.set_parent(&ribbon_button);

        let close_popover = popover.downgrade();
        close_button.connect_clicked(move |_| {
            if let Some(popover) = close_popover.upgrade() {
                popover.popdown();
            }
        });

        let escape_controller = gtk::EventControllerKey::new();
        let escape_popover = popover.downgrade();
        escape_controller.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                if let Some(popover) = escape_popover.upgrade() {
                    popover.popdown();
                }
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        popover.add_controller(escape_controller);

        let close_emit = emit.clone();
        let close_window = window.downgrade();
        popover.connect_closed(move |_| {
            if let Some(window) = close_window.upgrade() {
                window.set_keyboard_mode(KeyboardMode::None);
                GtkWindowExt::set_focus(&window, None::<&gtk::Widget>);
            }
            close_emit(AppAction::ClosePanel(output));
        });

        let motion = gtk::EventControllerMotion::new();
        let enter_emit = emit.clone();
        motion.connect_enter(move |_, _, _| enter_emit(AppAction::PointerEntered(output)));
        motion.connect_leave(move |_| emit(AppAction::PointerLeft(output)));
        window.add_controller(motion);

        Self {
            root,
            revealer,
            popover,
            first_panel_action,
        }
    }

    /// Render one authoritative presentation projection.
    pub fn render(
        &self,
        window: &gtk::ApplicationWindow,
        level: PresentationLevel,
        reduced_motion: bool,
    ) {
        self.revealer.set_transition_duration(if reduced_motion {
            0
        } else {
            RIBBON_TRANSITION_MILLIS
        });
        self.revealer
            .set_reveal_child(level != PresentationLevel::Selvage);

        if level == PresentationLevel::Panel {
            window.set_keyboard_mode(KeyboardMode::OnDemand);
            if !self.popover.is_visible() {
                self.popover.popup();
            }
            self.first_panel_action.grab_focus();
        } else {
            window.set_keyboard_mode(KeyboardMode::None);
            if self.popover.is_visible() {
                self.popover.popdown();
            }
        }
    }

    /// Detach the popover before its relative widget is destroyed.
    pub(crate) fn detach(&self) {
        if self.popover.parent().is_some() {
            self.popover.unparent();
        }
    }
}
