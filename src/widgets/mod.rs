//! Relm4 presentation components.

use std::{cell::Cell, rc::Rc};

use gtk4_layer_shell::{KeyboardMode, LayerShell};
use relm4::gtk;
use relm4::gtk::gdk;
use relm4::gtk::glib;
use relm4::gtk::prelude::*;

use crate::action::AppAction;
use crate::config::ThemeConfig;
use crate::context::arbitration::{CandidateAction, Severity};
use crate::state::{
    MarkPattern, MarkShape, OutputId, OutputView, PresentationLevel, StatusMark, WorkspaceMark,
};

pub mod active_context;
pub mod clock;
pub mod media;
pub mod panel;
pub mod ribbon;
pub mod selvage;

/// Fixed transparent layer-surface height in logical pixels.
pub const SURFACE_HEIGHT: i32 = 30;

/// Visual collapsed Selvage height in logical pixels.
pub const SELVAGE_HEIGHT: i32 = 3;

const RIBBON_TRANSITION_MILLIS: u32 = 160;

/// Install validated semantic theme tokens plus the retained GTK stylesheet.
pub(crate) fn install_style(display: &gdk::Display, theme: &ThemeConfig) {
    let provider = gtk::CssProvider::new();
    let css = format!(
        "@define-color weftwise_background {};\n\
         @define-color weftwise_surface {};\n\
         @define-color weftwise_text {};\n\
         @define-color weftwise_muted {};\n\
         @define-color weftwise_accent {};\n\
         @define-color weftwise_border {};\n\
         @define-color weftwise_warning {};\n\
         @define-color weftwise_critical {};\n\
         * {{ font-family: \"{}\"; }}\n\
         button.weftwise-ribbon {{ font-size: {}pt; border-radius: {}px; }}\n\
         popover.weftwise-panel > contents {{ border-radius: {}px; }}\n{}",
        theme.background,
        theme.surface,
        theme.text,
        theme.muted,
        theme.accent,
        theme.border,
        theme.warning,
        theme.critical,
        theme.font_family,
        theme.font_size,
        theme.radius,
        theme.radius,
        include_str!("../../assets/style.css"),
    );
    provider.load_from_data(&css);
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
    ribbon_navigation_label: gtk::Label,
    ribbon_context_label: gtk::Label,
    ribbon_status_label: gtk::Label,
    selvage: gtk::Box,
    navigation_marks: gtk::Box,
    activity_marks: gtk::Box,
    attention_marks: gtk::Box,
    popover: gtk::Popover,
    first_panel_action: gtk::Button,
    candidate_controls: Vec<(CandidateAction, gtk::Button)>,
}

impl TopEdgeWidgets {
    /// Build a Selvage, Ribbon, and attached Panel for one output.
    pub fn new(
        window: &gtk::ApplicationWindow,
        output: OutputId,
        immediate_corner: Rc<Cell<bool>>,
        corner_width: Rc<Cell<i32>>,
        emit: Rc<dyn Fn(AppAction)>,
    ) -> Self {
        let root = gtk::Overlay::builder()
            .height_request(SURFACE_HEIGHT)
            .hexpand(true)
            .build();
        root.add_css_class("weftwise-root");

        let ribbon_button = gtk::Button::builder()
            .height_request(SURFACE_HEIGHT)
            .hexpand(true)
            .focusable(true)
            .build();
        ribbon_button.add_css_class("weftwise-ribbon");

        let ribbon_navigation_label = gtk::Label::builder()
            .label("")
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .xalign(0.0)
            .build();
        ribbon_navigation_label.add_css_class("weftwise-ribbon-navigation");
        let ribbon_context_label = gtk::Label::builder()
            .label("")
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .hexpand(true)
            .xalign(0.5)
            .build();
        ribbon_context_label.add_css_class("weftwise-ribbon-context");
        let ribbon_status_label = gtk::Label::builder()
            .label("--:--")
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .xalign(1.0)
            .build();
        ribbon_status_label.add_css_class("weftwise-ribbon-status");
        let ribbon_regions = gtk::CenterBox::new();
        ribbon_regions.set_start_widget(Some(&ribbon_navigation_label));
        ribbon_regions.set_center_widget(Some(&ribbon_context_label));
        ribbon_regions.set_end_widget(Some(&ribbon_status_label));
        ribbon_regions.set_hexpand(true);
        ribbon_regions.add_css_class("weftwise-ribbon-regions");
        ribbon_button.set_child(Some(&ribbon_regions));

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
            .homogeneous(true)
            .hexpand(true)
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Start)
            .build();
        selvage.add_css_class("weftwise-selvage");
        let navigation_marks = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(2)
            .hexpand(true)
            .halign(gtk::Align::Start)
            .accessible_role(gtk::AccessibleRole::List)
            .build();
        navigation_marks.add_css_class("weftwise-navigation-region");
        let activity_marks = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(2)
            .hexpand(true)
            .halign(gtk::Align::Center)
            .accessible_role(gtk::AccessibleRole::Group)
            .build();
        activity_marks.add_css_class("weftwise-activity-region");
        let attention_marks = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(2)
            .hexpand(true)
            .halign(gtk::Align::End)
            .accessible_role(gtk::AccessibleRole::Group)
            .build();
        attention_marks.add_css_class("weftwise-attention-region");
        selvage.append(&navigation_marks);
        selvage.append(&activity_marks);
        selvage.append(&attention_marks);
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
        motion.set_propagation_phase(gtk::PropagationPhase::Capture);
        let enter_emit = emit.clone();
        motion.connect_enter(move |controller, x, _| {
            tracing::debug!(output = ?output, "pointer entered native surface");
            let surface_width = controller
                .widget()
                .map(|widget| widget.width())
                .unwrap_or_default();
            enter_emit(
                if pointer_entry_is_immediate(
                    immediate_corner.get(),
                    corner_width.get(),
                    surface_width,
                    x,
                ) {
                    AppAction::PointerEnteredImmediate(output)
                } else {
                    AppAction::PointerEntered(output)
                },
            );
        });
        motion.connect_leave(move |_| {
            tracing::debug!(output = ?output, "pointer left native surface");
            emit(AppAction::PointerLeft(output));
        });
        root.add_controller(motion);

        Self {
            root,
            revealer,
            ribbon_navigation_label,
            ribbon_context_label,
            ribbon_status_label,
            selvage,
            navigation_marks,
            activity_marks,
            attention_marks,
            popover,
            first_panel_action,
            candidate_controls,
        }
    }

    /// Render one authoritative presentation projection.
    pub fn render(&self, window: &gtk::ApplicationWindow, view: &OutputView) {
        let level = view.presentation.level();
        self.revealer
            .set_transition_duration(if view.presentation.reduced_motion() {
                0
            } else {
                RIBBON_TRANSITION_MILLIS
            });
        self.revealer
            .set_reveal_child(level != PresentationLevel::Selvage);
        self.selvage
            .set_visible(level == PresentationLevel::Selvage);
        self.ribbon_navigation_label
            .set_label(&view.ribbon_navigation_label);
        self.ribbon_context_label
            .set_label(&view.ribbon_context_label);
        self.ribbon_status_label
            .set_label(&view.ribbon_status_label);
        for label in [
            &self.ribbon_navigation_label,
            &self.ribbon_context_label,
            &self.ribbon_status_label,
        ] {
            label.set_tooltip_text(Some(&view.ribbon_accessible_label));
        }
        while let Some(child) = self.navigation_marks.first_child() {
            self.navigation_marks.remove(&child);
        }
        for workspace in &view.workspaces {
            self.navigation_marks.append(&workspace_mark(workspace));
        }
        while let Some(child) = self.activity_marks.first_child() {
            self.activity_marks.remove(&child);
        }
        for status in &view.activity {
            self.activity_marks.append(&status_mark(status));
        }
        while let Some(child) = self.attention_marks.first_child() {
            self.attention_marks.remove(&child);
        }
        for status in &view.attention {
            self.attention_marks.append(&status_mark(status));
        }
        for (action, button) in &self.candidate_controls {
            button.set_visible(view.candidate_actions.contains(action));
        }

        if level == PresentationLevel::Panel {
            window.set_keyboard_mode(KeyboardMode::OnDemand);
            let opening = !self.popover.is_visible();
            if opening {
                self.popover.popup();
                self.first_panel_action.grab_focus();
            }
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

fn pointer_entry_is_immediate(
    topology_immediate: bool,
    corner_width: i32,
    surface_width: i32,
    x: f64,
) -> bool {
    if topology_immediate {
        return true;
    }
    if corner_width <= 0 || surface_width <= 1 || !x.is_finite() {
        return false;
    }
    let corner_width = corner_width.min(surface_width).max(0);
    x < f64::from(corner_width) || x >= f64::from(surface_width.saturating_sub(corner_width))
}

fn workspace_mark(workspace: &WorkspaceMark) -> gtk::Label {
    let mark = gtk::Label::builder()
        .label(&workspace.accessible_label)
        .accessible_role(gtk::AccessibleRole::ListItem)
        .width_request(if workspace.active { 22 } else { 14 })
        .height_request(SELVAGE_HEIGHT)
        .build();
    mark.add_css_class("weftwise-workspace-mark");
    add_mark_semantics(&mark, workspace.shape, workspace.pattern);
    if workspace.active {
        mark.add_css_class("active");
    } else if workspace.occupied {
        mark.add_css_class("occupied");
    }
    mark.set_tooltip_text(Some(&workspace.accessible_label));
    mark
}

fn status_mark(status: &StatusMark) -> gtk::Label {
    let width = status
        .progress_basis_points
        .map_or(14, |progress| 8 + i32::from(progress) * 22 / 10_000);
    let mark = gtk::Label::builder()
        .label(&status.accessible_label)
        .accessible_role(if status.progress_basis_points.is_some() {
            gtk::AccessibleRole::ProgressBar
        } else if status.severity >= Severity::Warning {
            gtk::AccessibleRole::Alert
        } else {
            gtk::AccessibleRole::Status
        })
        .width_request(width)
        .height_request(SELVAGE_HEIGHT)
        .build();
    mark.add_css_class("weftwise-status-mark");
    add_mark_semantics(&mark, status.shape, status.pattern);
    match status.severity {
        Severity::Normal => mark.add_css_class("normal"),
        Severity::Notice => mark.add_css_class("notice"),
        Severity::Warning => mark.add_css_class("warning"),
        Severity::Critical => mark.add_css_class("critical"),
    }
    if status.selected {
        mark.add_css_class("selected");
    }
    mark.set_tooltip_text(Some(&status.accessible_label));
    mark
}

fn add_mark_semantics(mark: &impl IsA<gtk::Widget>, shape: MarkShape, pattern: MarkPattern) {
    match shape {
        MarkShape::Dot => mark.add_css_class("shape-dot"),
        MarkShape::Bar => mark.add_css_class("shape-bar"),
        MarkShape::Diamond => mark.add_css_class("shape-diamond"),
        MarkShape::Triangle => mark.add_css_class("shape-triangle"),
    }
    match pattern {
        MarkPattern::Outline => mark.add_css_class("pattern-outline"),
        MarkPattern::Solid => mark.add_css_class("pattern-solid"),
        MarkPattern::Striped => mark.add_css_class("pattern-striped"),
    }
}

#[cfg(test)]
mod pointer_entry_tests {
    use super::pointer_entry_is_immediate;

    #[test]
    fn either_corner_leg_reveals_immediately_while_the_top_island_keeps_dwell() {
        assert!(pointer_entry_is_immediate(false, 12, 1_920, 1.0));
        assert!(pointer_entry_is_immediate(false, 12, 1_920, 1_919.0));
        assert!(!pointer_entry_is_immediate(false, 12, 1_920, 960.0));
        assert!(pointer_entry_is_immediate(true, 12, 1_920, 960.0));
    }
}
