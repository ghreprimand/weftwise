//! Revealed Ribbon component boundary.
//!
//! The Ribbon is the dwell-revealed band that sits inside one fixed-height
//! output surface. It owns the slide revealer, the activation button, and the
//! navigation, context, and status labels. Construction lives here; the root
//! `TopEdgeWidgets` renders authoritative projections into these widgets.

use std::rc::Rc;

use relm4::gtk;
use relm4::gtk::prelude::*;

use crate::action::AppAction;
use crate::state::OutputId;

use super::{RIBBON_TRANSITION_MILLIS, SURFACE_HEIGHT};

/// GTK objects that make up one output's Ribbon.
pub(super) struct RibbonWidgets {
    /// Slide revealer wrapping the activation button.
    pub(super) revealer: gtk::Revealer,
    /// Activation button; the Panel popover parents onto this widget.
    pub(super) button: gtk::Button,
    /// Left region: workspace and navigation summary.
    pub(super) navigation_label: gtk::Label,
    /// Center region: active-context summary.
    pub(super) context_label: gtk::Label,
    /// Right region: clock and status summary.
    pub(super) status_label: gtk::Label,
}

/// Build the Ribbon revealer, button, and region labels for one output.
///
/// A click on the activation button requests that the Panel open for `output`.
pub(super) fn build(output: OutputId, emit: Rc<dyn Fn(AppAction)>) -> RibbonWidgets {
    let button = gtk::Button::builder()
        .height_request(SURFACE_HEIGHT)
        .hexpand(true)
        .focusable(true)
        .build();
    button.add_css_class("weftwise-ribbon");

    let navigation_label = gtk::Label::builder()
        .label("")
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .xalign(0.0)
        .build();
    navigation_label.add_css_class("weftwise-ribbon-navigation");
    let context_label = gtk::Label::builder()
        .label("")
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .hexpand(true)
        .xalign(0.5)
        .build();
    context_label.add_css_class("weftwise-ribbon-context");
    let status_label = gtk::Label::builder()
        .label("--:--")
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .xalign(1.0)
        .build();
    status_label.add_css_class("weftwise-ribbon-status");

    let ribbon_regions = gtk::CenterBox::new();
    ribbon_regions.set_start_widget(Some(&navigation_label));
    ribbon_regions.set_center_widget(Some(&context_label));
    ribbon_regions.set_end_widget(Some(&status_label));
    ribbon_regions.set_hexpand(true);
    ribbon_regions.add_css_class("weftwise-ribbon-regions");
    button.set_child(Some(&ribbon_regions));

    let reveal_emit = emit.clone();
    button.connect_clicked(move |_| reveal_emit(AppAction::OpenPanel(output)));

    let revealer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideDown)
        .transition_duration(RIBBON_TRANSITION_MILLIS)
        .reveal_child(false)
        .child(&button)
        .build();

    RibbonWidgets {
        revealer,
        button,
        navigation_label,
        context_label,
        status_label,
    }
}
