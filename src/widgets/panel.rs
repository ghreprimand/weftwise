//! Interactive Panel component boundary.
//!
//! The Panel is the explicitly opened surface: an attached GTK popover holding
//! audio controls, media transport actions, and a close action. Construction,
//! Escape dismissal, and focus restoration wiring live here; the root
//! `TopEdgeWidgets` drives popup/popdown and sensitivity from projections.

use std::rc::Rc;

use gtk4_layer_shell::{KeyboardMode, LayerShell};
use relm4::gtk;
use relm4::gtk::gdk;
use relm4::gtk::glib;
use relm4::gtk::prelude::*;

use crate::action::AppAction;
use crate::context::arbitration::CandidateAction;
use crate::state::OutputId;

/// GTK objects that make up one output's Panel popover.
pub(super) struct PanelWidgets {
    /// Attached popover holding the Panel content.
    pub(super) popover: gtk::Popover,
    /// Widget focused first when the Panel opens with audio controls present.
    pub(super) first_panel_action: gtk::Button,
    /// Explicit close action.
    pub(super) close_button: gtk::Button,
    /// Audio control row; hidden when no audio adapter is present.
    pub(super) audio_controls: gtk::Box,
    /// Current volume or mute summary label.
    pub(super) audio_label: gtk::Label,
    /// Decrease-volume action.
    pub(super) volume_down: gtk::Button,
    /// Increase-volume action.
    pub(super) volume_up: gtk::Button,
    /// Mute toggle action.
    pub(super) mute_button: gtk::Button,
    /// Media transport actions, each shown only when advertised.
    pub(super) candidate_controls: Vec<(CandidateAction, gtk::Button)>,
}

/// Build the Panel popover for one output and parent it onto `ribbon_button`.
///
/// Escape dismisses the popover; closing restores keyboard mode and releases
/// focus on `window`, then requests the reducer close the Panel.
pub(super) fn build(
    window: &gtk::ApplicationWindow,
    output: OutputId,
    ribbon_button: &gtk::Button,
    emit: Rc<dyn Fn(AppAction)>,
) -> PanelWidgets {
    let panel_content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    let heading = gtk::Label::new(Some("Weftwise controls"));
    heading.set_xalign(0.0);
    heading.add_css_class("title-4");
    panel_content.append(&heading);

    let audio_controls = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .accessible_role(gtk::AccessibleRole::Group)
        .build();
    let volume_down = gtk::Button::with_label("Volume -10");
    volume_down.set_focusable(true);
    let audio_label = gtk::Label::new(Some("Volume unavailable"));
    audio_label.set_xalign(0.5);
    let volume_up = gtk::Button::with_label("Volume +10");
    volume_up.set_focusable(true);
    let mute_button = gtk::Button::with_label("Mute");
    mute_button.set_focusable(true);
    audio_controls.append(&volume_down);
    audio_controls.append(&audio_label);
    audio_controls.append(&volume_up);
    audio_controls.append(&mute_button);
    panel_content.append(&audio_controls);

    let first_panel_action = volume_down.clone();
    let volume_down_emit = emit.clone();
    volume_down.connect_clicked(move |_| {
        volume_down_emit(AppAction::AdjustOutputVolume(output, -10));
    });
    let volume_up_emit = emit.clone();
    volume_up.connect_clicked(move |_| {
        volume_up_emit(AppAction::AdjustOutputVolume(output, 10));
    });
    let mute_emit = emit.clone();
    mute_button.connect_clicked(move |_| {
        mute_emit(AppAction::ToggleOutputMute(output));
    });

    let media_controls = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .accessible_role(gtk::AccessibleRole::Group)
        .build();
    let candidate_controls = [
        (CandidateAction::MediaSeek(-10_000), "Back 10 seconds"),
        (CandidateAction::MediaPrevious, "Previous"),
        (CandidateAction::MediaPlayPause, "Play or pause"),
        (CandidateAction::MediaNext, "Next"),
        (CandidateAction::MediaSeek(10_000), "Forward 10 seconds"),
    ]
    .into_iter()
    .map(|(action, label)| {
        let button = gtk::Button::with_label(label);
        button.set_focusable(true);
        button.set_visible(false);
        let action_emit = emit.clone();
        button.connect_clicked(move |_| {
            action_emit(AppAction::Candidate(output, action));
        });
        media_controls.append(&button);
        (action, button)
    })
    .collect::<Vec<_>>();
    panel_content.append(&media_controls);

    let close_button = gtk::Button::with_label("Close Panel");
    close_button.set_focusable(true);
    panel_content.append(&close_button);

    let popover = gtk::Popover::builder()
        .autohide(true)
        .has_arrow(false)
        .position(gtk::PositionType::Bottom)
        .accessible_role(gtk::AccessibleRole::Dialog)
        .child(&panel_content)
        .build();
    popover.add_css_class("weftwise-panel");
    popover.set_parent(ribbon_button);

    let close_emit = emit.clone();
    close_button.connect_clicked(move |_| {
        close_emit(AppAction::ClosePanel(output));
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

    PanelWidgets {
        popover,
        first_panel_action,
        close_button,
        audio_controls,
        audio_label,
        volume_down,
        volume_up,
        mute_button,
        candidate_controls,
    }
}
