//! Relm4 presentation components.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use gtk4_layer_shell::{KeyboardMode, LayerShell};
use relm4::gtk;
use relm4::gtk::gdk;
use relm4::gtk::glib;
use relm4::gtk::prelude::*;

use crate::action::AppAction;
use crate::config::ThemeConfig;
use crate::context::arbitration::{CandidateAction, Severity};
use crate::state::{
    MarkPattern, MarkShape, OutputId, OutputView, PresentationLevel, StatusMark, WorkspaceId,
    WorkspaceMark,
};
use crate::widgets::selvage::{MarkOp, diff_marks};

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
    navigation_retained: RefCell<Vec<RetainedMark<WorkspaceId, WorkspaceMark>>>,
    activity_retained: RefCell<Vec<RetainedMark<usize, StatusMark>>>,
    attention_retained: RefCell<Vec<RetainedMark<usize, StatusMark>>>,
    popover: gtk::Popover,
    first_panel_action: gtk::Button,
    close_button: gtk::Button,
    audio_controls: gtk::Box,
    audio_label: gtk::Label,
    volume_down: gtk::Button,
    volume_up: gtk::Button,
    mute_button: gtk::Button,
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
        popover.set_parent(&ribbon_button);

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
            navigation_retained: RefCell::new(Vec::new()),
            activity_retained: RefCell::new(Vec::new()),
            attention_retained: RefCell::new(Vec::new()),
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
        reconcile_workspace_marks(
            &self.navigation_marks,
            &self.navigation_retained,
            &view.workspaces,
        );
        reconcile_status_marks(
            &self.activity_marks,
            &self.activity_retained,
            &view.activity,
        );
        reconcile_status_marks(
            &self.attention_marks,
            &self.attention_retained,
            &view.attention,
        );
        for (action, button) in &self.candidate_controls {
            button.set_visible(view.candidate_actions.contains(action));
        }
        self.audio_controls.set_visible(view.audio.is_some());
        if let Some(audio) = view.audio {
            let label = if audio.muted {
                "Volume muted".to_owned()
            } else {
                format!("Volume {}%", audio.percent)
            };
            self.audio_label.set_label(&label);
            self.volume_down.set_sensitive(audio.can_set_volume);
            self.volume_up.set_sensitive(audio.can_set_volume);
            self.mute_button.set_sensitive(audio.can_set_mute);
            self.mute_button
                .set_label(if audio.muted { "Unmute" } else { "Mute" });
        }

        if level == PresentationLevel::Panel {
            window.set_keyboard_mode(KeyboardMode::OnDemand);
            let opening = !self.popover.is_visible();
            if opening {
                self.popover.popup();
                if self.audio_controls.is_visible() {
                    self.first_panel_action.grab_focus();
                } else if let Some((_, button)) = self
                    .candidate_controls
                    .iter()
                    .find(|(_, button)| button.is_visible())
                {
                    button.grab_focus();
                } else {
                    self.close_button.grab_focus();
                }
            }
        } else if view.presentation.ribbon_pinned() {
            window.set_keyboard_mode(KeyboardMode::OnDemand);
            if self.popover.is_visible() {
                self.popover.popdown();
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

/// One retained Selvage mark: its stable key, last-rendered model, and widget.
struct RetainedMark<K, M> {
    key: K,
    model: M,
    widget: gtk::Label,
}

/// Reconcile a Selvage region's retained mark widgets against a new projection.
///
/// [`diff_marks`] plans which widgets to reuse, create, or remove; matched
/// widgets are updated in place only when their model actually changed, and are
/// reordered to the projection's order. This keeps tooltips and accessible
/// objects alive across renders instead of destroying and rebuilding them.
fn reconcile_region<K, M>(
    container: &gtk::Box,
    retained: &RefCell<Vec<RetainedMark<K, M>>>,
    next_models: &[M],
    key_of: impl Fn(usize, &M) -> K,
    build: impl Fn(&M) -> gtk::Label,
    apply: impl Fn(&gtk::Label, &M),
    needs_recreate: impl Fn(&M, &M) -> bool,
) where
    K: Eq + Clone,
    M: Clone + PartialEq,
{
    let mut current = retained.borrow_mut();
    let prev_keys: Vec<K> = current.iter().map(|mark| mark.key.clone()).collect();
    let next_keys: Vec<K> = next_models
        .iter()
        .enumerate()
        .map(|(index, model)| key_of(index, model))
        .collect();
    let ops = diff_marks(&prev_keys, &next_keys);

    // Take ownership of the retained widgets so operations can move them.
    let mut old: Vec<Option<RetainedMark<K, M>>> = current.drain(..).map(Some).collect();
    let mut next_retained: Vec<RetainedMark<K, M>> = Vec::with_capacity(next_models.len());
    for op in &ops {
        match *op {
            MarkOp::Remove { prev } => {
                if let Some(mark) = old[prev].take() {
                    container.remove(&mark.widget);
                }
            }
            MarkOp::Reuse { prev, next } => {
                let existing = old[prev].take().expect("reused mark is present");
                let model = &next_models[next];
                if needs_recreate(&existing.model, model) {
                    container.remove(&existing.widget);
                    let widget = build(model);
                    container.append(&widget);
                    next_retained.push(RetainedMark {
                        key: next_keys[next].clone(),
                        model: model.clone(),
                        widget,
                    });
                } else {
                    if existing.model != *model {
                        apply(&existing.widget, model);
                    }
                    next_retained.push(RetainedMark {
                        key: next_keys[next].clone(),
                        model: model.clone(),
                        widget: existing.widget,
                    });
                }
            }
            MarkOp::Create { next } => {
                let model = &next_models[next];
                let widget = build(model);
                container.append(&widget);
                next_retained.push(RetainedMark {
                    key: next_keys[next].clone(),
                    model: model.clone(),
                    widget,
                });
            }
        }
    }

    // Place each widget after its predecessor to match the projection's order.
    let mut previous: Option<gtk::Label> = None;
    for mark in &next_retained {
        container.reorder_child_after(&mark.widget, previous.as_ref());
        previous = Some(mark.widget.clone());
    }

    *current = next_retained;
}

/// Reconcile the navigation region's workspace marks, keyed by workspace id.
fn reconcile_workspace_marks(
    container: &gtk::Box,
    retained: &RefCell<Vec<RetainedMark<WorkspaceId, WorkspaceMark>>>,
    next: &[WorkspaceMark],
) {
    reconcile_region(
        container,
        retained,
        next,
        |_index, model| model.id,
        build_workspace_mark,
        apply_workspace_mark,
        |_previous, _next| false,
    );
}

/// Reconcile an activity or attention region's status marks, keyed by their
/// stable region slot. A slot whose accessible role would change is recreated
/// because a GTK accessible role is fixed at construction.
fn reconcile_status_marks(
    container: &gtk::Box,
    retained: &RefCell<Vec<RetainedMark<usize, StatusMark>>>,
    next: &[StatusMark],
) {
    reconcile_region(
        container,
        retained,
        next,
        |index, _model| index,
        build_status_mark,
        apply_status_mark,
        |previous, next| status_role(previous) != status_role(next),
    );
}

fn build_workspace_mark(workspace: &WorkspaceMark) -> gtk::Label {
    let mark = gtk::Label::builder()
        .accessible_role(gtk::AccessibleRole::ListItem)
        .height_request(SELVAGE_HEIGHT)
        .build();
    apply_workspace_mark(&mark, workspace);
    mark
}

fn apply_workspace_mark(mark: &gtk::Label, workspace: &WorkspaceMark) {
    mark.set_label(&workspace.accessible_label);
    mark.set_width_request(if workspace.active { 22 } else { 14 });
    mark.set_css_classes(&workspace_mark_classes(workspace));
    mark.set_tooltip_text(Some(&workspace.accessible_label));
}

fn workspace_mark_classes(workspace: &WorkspaceMark) -> Vec<&'static str> {
    let mut classes = vec![
        "weftwise-workspace-mark",
        shape_class(workspace.shape),
        pattern_class(workspace.pattern),
    ];
    if workspace.active {
        classes.push("active");
    } else if workspace.occupied {
        classes.push("occupied");
    }
    classes
}

fn status_role(status: &StatusMark) -> gtk::AccessibleRole {
    if status.progress_basis_points.is_some() {
        gtk::AccessibleRole::ProgressBar
    } else if status.severity >= Severity::Warning {
        gtk::AccessibleRole::Alert
    } else {
        gtk::AccessibleRole::Status
    }
}

fn status_mark_width(status: &StatusMark) -> i32 {
    status
        .progress_basis_points
        .map_or(14, |progress| 8 + i32::from(progress) * 22 / 10_000)
}

fn build_status_mark(status: &StatusMark) -> gtk::Label {
    let mark = gtk::Label::builder()
        .accessible_role(status_role(status))
        .height_request(SELVAGE_HEIGHT)
        .build();
    apply_status_mark(&mark, status);
    mark
}

fn apply_status_mark(mark: &gtk::Label, status: &StatusMark) {
    mark.set_label(&status.accessible_label);
    mark.set_width_request(status_mark_width(status));
    mark.set_css_classes(&status_mark_classes(status));
    mark.set_tooltip_text(Some(&status.accessible_label));
}

fn status_mark_classes(status: &StatusMark) -> Vec<&'static str> {
    let mut classes = vec![
        "weftwise-status-mark",
        shape_class(status.shape),
        pattern_class(status.pattern),
    ];
    classes.push(match status.severity {
        Severity::Normal => "normal",
        Severity::Notice => "notice",
        Severity::Warning => "warning",
        Severity::Critical => "critical",
    });
    if status.selected {
        classes.push("selected");
    }
    classes
}

fn shape_class(shape: MarkShape) -> &'static str {
    match shape {
        MarkShape::Dot => "shape-dot",
        MarkShape::Bar => "shape-bar",
        MarkShape::Diamond => "shape-diamond",
        MarkShape::Triangle => "shape-triangle",
    }
}

fn pattern_class(pattern: MarkPattern) -> &'static str {
    match pattern {
        MarkPattern::Outline => "pattern-outline",
        MarkPattern::Solid => "pattern-solid",
        MarkPattern::Striped => "pattern-striped",
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
