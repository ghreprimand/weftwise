//! Layer-shell surface configuration and input-region ownership.

use std::cell::Cell;
use std::rc::Rc;

use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use relm4::gtk;
use relm4::gtk::cairo;
use relm4::gtk::gdk;
use relm4::gtk::glib;
use relm4::gtk::prelude::*;

use crate::action::AppAction;
use crate::config::{ActivationAnchor, ActivationConfig, ActivationMode};
use crate::shell::ExclusiveZone;
use crate::state::{OutputId, OutputView, PresentationLevel};
use crate::widgets::{SURFACE_HEIGHT, TopEdgeWidgets};

use super::outputs::{OutputBinding, ShellEvent};

/// Pure input-region rectangle used by native surfaces and deterministic tests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputRegionGeometry {
    /// Region offset from the surface's left edge.
    pub x: i32,
    /// Region offset from the surface's top edge.
    pub y: i32,
    /// Region width in GDK logical coordinates.
    pub width: i32,
    /// Region height in GDK logical coordinates.
    pub height: i32,
}

impl InputRegionGeometry {
    /// Compute the pointer-active rectangle for one presentation level.
    #[must_use]
    pub const fn for_level(
        width: i32,
        level: PresentationLevel,
        activation: ActivationRegion,
    ) -> Self {
        let width = if width < 0 { 0 } else { width };
        match level {
            PresentationLevel::Selvage => {
                let x = if activation.x < 0 {
                    0
                } else if activation.x > width {
                    width
                } else {
                    activation.x
                };
                let available = width.saturating_sub(x);
                Self {
                    x,
                    y: 0,
                    width: if width <= 1 || activation.width < 0 {
                        0
                    } else if activation.width > available {
                        available
                    } else {
                        activation.width
                    },
                    height: activation.height,
                }
            }
            PresentationLevel::Ribbon | PresentationLevel::Panel => Self {
                x: 0,
                y: 0,
                width,
                height: SURFACE_HEIGHT,
            },
        }
    }

    /// Mirror a non-central top island across the output's horizontal center.
    #[must_use]
    pub const fn mirrored_top_island(
        width: i32,
        level: PresentationLevel,
        activation: ActivationRegion,
    ) -> Self {
        let primary = Self::for_level(width, level, activation);
        if !matches!(level, PresentationLevel::Selvage) || primary.width <= 0 || primary.height <= 0
        {
            return Self {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            };
        }
        let width = if width < 0 { 0 } else { width };
        let mirrored_x = width.saturating_sub(primary.x.saturating_add(primary.width));
        let mirrored_end = mirrored_x.saturating_add(primary.width);
        let primary_end = primary.x.saturating_add(primary.width);
        if mirrored_x < primary_end && primary.x < mirrored_end {
            return Self {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            };
        }
        Self {
            x: mirrored_x,
            y: primary.y,
            width: primary.width,
            height: primary.height,
        }
    }

    /// Compute the narrow left-edge leg that makes a collapsed target reachable
    /// horizontally even when another output covers the target's top edge.
    #[must_use]
    pub const fn left_edge_leg(
        width: i32,
        level: PresentationLevel,
        activation: ActivationRegion,
    ) -> Self {
        let width = if width < 0 { 0 } else { width };
        if !matches!(level, PresentationLevel::Selvage) || width <= 1 || activation.width <= 0 {
            return Self {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            };
        }
        let leg_width = if activation.height < 0 {
            0
        } else if activation.height > width {
            width
        } else {
            activation.height
        };
        Self {
            x: 0,
            y: 0,
            width: leg_width,
            height: SURFACE_HEIGHT,
        }
    }

    /// Compute the matching narrow right-edge activation leg.
    #[must_use]
    pub const fn right_edge_leg(
        width: i32,
        level: PresentationLevel,
        activation: ActivationRegion,
    ) -> Self {
        let width = if width < 0 { 0 } else { width };
        if !matches!(level, PresentationLevel::Selvage) || width <= 1 || activation.width <= 0 {
            return Self {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            };
        }
        let leg_width = if activation.height < 0 {
            0
        } else if activation.height > width {
            width
        } else {
            activation.height
        };
        Self {
            x: width.saturating_sub(leg_width),
            y: 0,
            width: leg_width,
            height: SURFACE_HEIGHT,
        }
    }
}

/// Collapsed pointer activation island in surface-local logical coordinates.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ActivationRegion {
    /// Offset from the output's left edge.
    pub x: i32,
    /// Bounded island width.
    pub width: i32,
    /// Bounded island height.
    pub height: i32,
    /// Reveal on every bounded entry because all adjoining edges are internal.
    pub immediate: bool,
}

/// Pure output rectangle used to select exposed top-edge segments.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutputRectangle {
    /// Global logical X coordinate.
    pub x: i32,
    /// Global logical Y coordinate.
    pub y: i32,
    /// Logical width.
    pub width: i32,
    /// Logical height.
    pub height: i32,
}

impl From<gdk::Rectangle> for OutputRectangle {
    fn from(rectangle: gdk::Rectangle) -> Self {
        Self {
            x: rectangle.x(),
            y: rectangle.y(),
            width: rectangle.width(),
            height: rectangle.height(),
        }
    }
}

/// Select a bounded activation island from the widest exposed top-edge segment.
#[must_use]
pub fn activation_region(
    target: OutputRectangle,
    outputs: &[OutputRectangle],
    config: &ActivationConfig,
) -> ActivationRegion {
    let height = i32::from(config.height).min(SURFACE_HEIGHT);
    if target.width <= 0 {
        return ActivationRegion {
            height,
            ..ActivationRegion::default()
        };
    }
    if config.mode == ActivationMode::FullWidth {
        return ActivationRegion {
            x: 0,
            width: target.width,
            height,
            immediate: false,
        };
    }

    let target_start = target.x;
    let target_end = target.x.saturating_add(target.width);
    let mut covered = outputs
        .iter()
        .filter(|other| *other != &target)
        .filter(|other| other.y.saturating_add(other.height).abs_diff(target.y) <= 2)
        .filter_map(|other| {
            let start = other.x.max(target_start);
            let end = other.x.saturating_add(other.width).min(target_end);
            (start < end).then_some((start, end))
        })
        .collect::<Vec<_>>();
    covered.sort_unstable();

    let mut exposed = Vec::new();
    let mut cursor = target_start;
    for (start, end) in covered {
        if cursor < start {
            exposed.push((cursor, start));
        }
        cursor = cursor.max(end);
    }
    if cursor < target_end {
        exposed.push((cursor, target_end));
    }
    let top_edge_is_internal = exposed.is_empty();
    let left_edge_is_internal = outputs.iter().any(|other| {
        other != &target
            && other.x.saturating_add(other.width).abs_diff(target_start) <= 2
            && other.y < target.y.saturating_add(SURFACE_HEIGHT)
            && other.y.saturating_add(other.height) > target.y
    });
    let right_edge_is_internal = outputs.iter().any(|other| {
        other != &target
            && other.x.abs_diff(target_end) <= 2
            && other.y < target.y.saturating_add(SURFACE_HEIGHT)
            && other.y.saturating_add(other.height) > target.y
    });
    let (start, end) = exposed
        .into_iter()
        .max_by_key(|(start, end)| (end - start, *start))
        .unwrap_or((target_start, target_end));
    let segment_width = end - start;
    let width = i32::from(config.width).min(segment_width).max(0);
    let margin = i32::from(config.margin).min((segment_width - width).max(0));
    let global_x = match config.anchor {
        ActivationAnchor::Start => start.saturating_add(margin),
        ActivationAnchor::Center => start.saturating_add((segment_width - width) / 2),
        ActivationAnchor::End => end.saturating_sub(width).saturating_sub(margin),
    };
    ActivationRegion {
        x: global_x.saturating_sub(target.x),
        width,
        height,
        immediate: config.reveal_on_entry
            || (top_edge_is_internal && left_edge_is_internal && right_edge_is_internal),
    }
}

/// GTK objects for one compositor output. This type remains on the main thread.
pub(crate) struct ManagedSurface {
    pub id: OutputId,
    pub monitor: gdk::Monitor,
    window: gtk::ApplicationWindow,
    widgets: TopEdgeWidgets,
    level: Rc<Cell<PresentationLevel>>,
    activation: Rc<Cell<ActivationRegion>>,
    immediate_corner: Rc<Cell<bool>>,
    corner_width: Rc<Cell<i32>>,
    monitor_handler: Option<glib::SignalHandlerId>,
}

impl ManagedSurface {
    /// Configure every layer-shell property before presenting the window.
    pub fn new(
        application: &gtk::Application,
        monitor: &gdk::Monitor,
        id: OutputId,
        zone: ExclusiveZone,
        activation: ActivationRegion,
        action_sink: Rc<dyn Fn(AppAction)>,
        shell_sink: Rc<dyn Fn(ShellEvent)>,
    ) -> Self {
        let window = gtk::ApplicationWindow::builder()
            .application(application)
            .decorated(false)
            .title("Weftwise")
            .build();
        window.add_css_class("weftwise-surface");
        window.set_default_size(1, SURFACE_HEIGHT);
        window.set_focusable(true);

        window.init_layer_shell();
        window.set_monitor(Some(monitor));
        window.set_layer(Layer::Overlay);
        window.set_anchor(Edge::Top, true);
        window.set_anchor(Edge::Left, true);
        window.set_anchor(Edge::Right, true);
        window.set_namespace(Some("weftwise"));
        window.set_exclusive_zone(zone.value());
        window.set_keyboard_mode(KeyboardMode::None);

        let immediate_corner = Rc::new(Cell::new(activation.immediate));
        let corner_width = Rc::new(Cell::new(activation.height));
        let widgets = TopEdgeWidgets::new(
            &window,
            id,
            immediate_corner.clone(),
            corner_width.clone(),
            action_sink,
        );
        window.set_child(Some(&widgets.root));

        let level = Rc::new(Cell::new(PresentationLevel::Selvage));
        let activation = Rc::new(Cell::new(activation));
        let layout_level = level.clone();
        let layout_activation = activation.clone();
        window.connect_realize(move |window| {
            let Some(surface) = window.surface() else {
                return;
            };
            let configured_level = layout_level.clone();
            let configured_activation = layout_activation.clone();
            surface.connect_layout(move |surface, width, _| {
                apply_input_region_to_surface(
                    surface,
                    width,
                    configured_level.get(),
                    configured_activation.get(),
                    id,
                    "layout",
                );
            });
            apply_input_region(
                window,
                PresentationLevel::Selvage,
                layout_activation.get(),
                id,
                "realize",
            );
        });
        let scale_sink = shell_sink.clone();
        window.connect_scale_factor_notify(move |_| {
            scale_sink(ShellEvent::GeometryChanged(id));
        });

        let invalidated_sink = shell_sink.clone();
        let monitor_handler = monitor.connect_invalidate(move |_| {
            invalidated_sink(ShellEvent::OutputsChanged);
        });

        window.present();

        Self {
            id,
            monitor: monitor.clone(),
            window,
            widgets,
            level,
            activation,
            immediate_corner,
            corner_width,
            monitor_handler: Some(monitor_handler),
        }
    }

    /// Render state and recompute the GDK input region after every level change.
    pub fn render(&self, view: &OutputView) {
        self.widgets.render(&self.window, view);
        self.refresh_input_region(view.presentation.level());
    }

    /// Recompute the current input region after allocation or scale changes.
    pub fn refresh_input_region(&self, level: PresentationLevel) {
        self.level.set(level);
        apply_input_region(&self.window, level, self.activation.get(), self.id, "state");
    }

    /// Replace the collapsed activation island after output layout changes.
    pub fn set_activation_region(&self, activation: ActivationRegion) {
        self.activation.set(activation);
        self.immediate_corner.set(activation.immediate);
        self.corner_width.set(activation.height);
        self.refresh_input_region(self.level.get());
    }

    /// Bind the GDK surface to compositor state without logging its connector.
    pub fn binding(&self) -> OutputBinding {
        OutputBinding {
            id: self.id,
            connector: self.monitor.connector().map(|value| value.to_string()),
        }
    }

    /// Destroy the native surface and release compositor ownership.
    pub fn close(&mut self) {
        if let Some(handler) = self.monitor_handler.take() {
            self.monitor.disconnect(handler);
        }
        self.widgets.detach();
        self.window.destroy();
    }
}

fn apply_input_region(
    window: &gtk::ApplicationWindow,
    level: PresentationLevel,
    activation: ActivationRegion,
    output: OutputId,
    source: &'static str,
) {
    let Some(surface) = window.surface() else {
        return;
    };
    apply_input_region_to_surface(&surface, surface.width(), level, activation, output, source);
}

fn apply_input_region_to_surface(
    surface: &gdk::Surface,
    width: i32,
    level: PresentationLevel,
    activation: ActivationRegion,
    output: OutputId,
    source: &'static str,
) {
    let geometry = InputRegionGeometry::for_level(width, level, activation);
    let mirrored_island = InputRegionGeometry::mirrored_top_island(width, level, activation);
    let left_edge_leg = InputRegionGeometry::left_edge_leg(width, level, activation);
    let right_edge_leg = InputRegionGeometry::right_edge_leg(width, level, activation);
    let rectangles = [geometry, mirrored_island, left_edge_leg, right_edge_leg]
        .into_iter()
        .filter(|rectangle| rectangle.width > 0 && rectangle.height > 0)
        .map(|rectangle| {
            cairo::RectangleInt::new(rectangle.x, rectangle.y, rectangle.width, rectangle.height)
        })
        .collect::<Vec<_>>();
    let region = cairo::Region::create_rectangles(&rectangles);
    surface.set_input_region(Some(&region));
    tracing::debug!(
        output = ?output,
        source,
        x = geometry.x,
        width = geometry.width,
        height = geometry.height,
        mirrored_x = mirrored_island.x,
        mirrored_width = mirrored_island.width,
        left_edge_width = left_edge_leg.width,
        right_edge_width = right_edge_leg.width,
        empty = geometry.width == 0,
        ?level,
        "native input region installed"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapsed_region_only_covers_the_selvage() {
        let activation = ActivationRegion {
            x: 1200,
            width: 96,
            height: 8,
            immediate: false,
        };
        assert_eq!(
            InputRegionGeometry::for_level(1920, PresentationLevel::Selvage, activation),
            InputRegionGeometry {
                x: 1200,
                y: 0,
                width: 96,
                height: 8,
            }
        );
    }

    #[test]
    fn expanded_regions_cover_the_fixed_surface() {
        for level in [PresentationLevel::Ribbon, PresentationLevel::Panel] {
            assert_eq!(
                InputRegionGeometry::for_level(1280, level, ActivationRegion::default()),
                InputRegionGeometry {
                    x: 0,
                    y: 0,
                    width: 1280,
                    height: SURFACE_HEIGHT,
                }
            );
        }
    }

    #[test]
    fn unallocated_width_has_an_empty_input_region() {
        assert_eq!(
            InputRegionGeometry::for_level(
                0,
                PresentationLevel::Selvage,
                ActivationRegion::default(),
            )
            .width,
            0
        );
    }

    #[test]
    fn activation_uses_the_widest_exposed_top_edge_segment() {
        let target = OutputRectangle {
            x: 100,
            y: 100,
            width: 400,
            height: 200,
        };
        let above = OutputRectangle {
            x: 50,
            y: 0,
            width: 250,
            height: 100,
        };
        let region = activation_region(target, &[target, above], &ActivationConfig::default());
        assert_eq!(
            region,
            ActivationRegion {
                x: 292,
                width: 96,
                height: 12,
                immediate: false,
            }
        );
    }

    #[test]
    fn full_width_activation_is_an_explicit_rollback_mode() {
        let target = OutputRectangle {
            x: 0,
            y: 0,
            width: 320,
            height: 200,
        };
        let config = ActivationConfig {
            mode: ActivationMode::FullWidth,
            ..ActivationConfig::default()
        };
        assert_eq!(
            activation_region(target, &[target], &config),
            ActivationRegion {
                x: 0,
                width: 320,
                height: 12,
                immediate: false,
            }
        );
    }

    #[test]
    fn collapsed_region_has_matching_corner_legs_for_horizontal_entry() {
        let activation = ActivationRegion {
            x: 400,
            width: 96,
            height: 12,
            immediate: false,
        };
        assert_eq!(
            InputRegionGeometry::left_edge_leg(1920, PresentationLevel::Selvage, activation,),
            InputRegionGeometry {
                x: 0,
                y: 0,
                width: 12,
                height: SURFACE_HEIGHT,
            }
        );
        assert_eq!(
            InputRegionGeometry::right_edge_leg(1920, PresentationLevel::Selvage, activation,),
            InputRegionGeometry {
                x: 1908,
                y: 0,
                width: 12,
                height: SURFACE_HEIGHT,
            }
        );
        assert_eq!(
            InputRegionGeometry::left_edge_leg(1920, PresentationLevel::Ribbon, activation,).width,
            0
        );
        assert_eq!(
            InputRegionGeometry::right_edge_leg(1920, PresentationLevel::Ribbon, activation,).width,
            0
        );
    }

    #[test]
    fn end_anchored_top_island_is_mirrored_at_the_start_edge() {
        let activation = ActivationRegion {
            x: 1_812,
            width: 96,
            height: 12,
            immediate: false,
        };
        assert_eq!(
            InputRegionGeometry::mirrored_top_island(1_920, PresentationLevel::Selvage, activation,),
            InputRegionGeometry {
                x: 12,
                y: 0,
                width: 96,
                height: 12,
            }
        );
        assert_eq!(
            InputRegionGeometry::mirrored_top_island(1_920, PresentationLevel::Ribbon, activation,)
                .width,
            0
        );
    }

    #[test]
    fn centered_or_full_width_island_is_not_duplicated() {
        for activation in [
            ActivationRegion {
                x: 912,
                width: 96,
                height: 12,
                immediate: false,
            },
            ActivationRegion {
                x: 0,
                width: 1_920,
                height: 12,
                immediate: false,
            },
        ] {
            assert_eq!(
                InputRegionGeometry::mirrored_top_island(
                    1_920,
                    PresentationLevel::Selvage,
                    activation,
                )
                .width,
                0
            );
        }
    }

    #[test]
    fn fully_internal_activation_uses_immediate_entry_with_fractional_rounding_tolerance() {
        let target = OutputRectangle {
            x: 100,
            y: 101,
            width: 400,
            height: 200,
        };
        let above = OutputRectangle {
            x: 100,
            y: 0,
            width: 400,
            height: 100,
        };
        let left = OutputRectangle {
            x: -100,
            y: 101,
            width: 200,
            height: 200,
        };
        let right = OutputRectangle {
            x: 501,
            y: 101,
            width: 200,
            height: 200,
        };
        let region = activation_region(
            target,
            &[target, above, left, right],
            &ActivationConfig::default(),
        );
        assert!(region.immediate);
    }

    #[test]
    fn a_physical_side_keeps_top_island_dwell_when_the_top_edge_is_internal() {
        let target = OutputRectangle {
            x: 100,
            y: 100,
            width: 400,
            height: 200,
        };
        let above = OutputRectangle {
            x: 100,
            y: 0,
            width: 400,
            height: 100,
        };
        let right = OutputRectangle {
            x: 500,
            y: 100,
            width: 200,
            height: 200,
        };

        let region = activation_region(
            target,
            &[target, above, right],
            &ActivationConfig::default(),
        );

        assert!(!region.immediate);
    }

    #[test]
    fn physical_top_segment_keeps_top_island_dwell() {
        let target = OutputRectangle {
            x: 100,
            y: 100,
            width: 400,
            height: 200,
        };
        let above = OutputRectangle {
            x: 100,
            y: 0,
            width: 400,
            height: 100,
        };

        let region = activation_region(target, &[target, above], &ActivationConfig::default());

        assert!(!region.immediate);
    }

    #[test]
    fn configured_entry_reveal_bypasses_dwell_for_any_output_topology() {
        let target = OutputRectangle {
            x: 100,
            y: 100,
            width: 400,
            height: 200,
        };
        let config = ActivationConfig {
            reveal_on_entry: true,
            ..ActivationConfig::default()
        };

        let region = activation_region(target, &[target], &config);

        assert!(region.immediate);
    }
}
