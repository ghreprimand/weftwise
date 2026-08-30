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
use crate::shell::ExclusiveZone;
use crate::state::{OutputId, OutputView, PresentationLevel};
use crate::widgets::{SELVAGE_HEIGHT, SURFACE_HEIGHT, TopEdgeWidgets};

use super::outputs::{OutputBinding, ShellEvent};

/// Pure input-region rectangle used by native surfaces and deterministic tests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputRegionGeometry {
    /// Region width in GDK logical coordinates.
    pub width: i32,
    /// Region height in GDK logical coordinates.
    pub height: i32,
}

impl InputRegionGeometry {
    /// Compute the pointer-active rectangle for one presentation level.
    #[must_use]
    pub const fn for_level(width: i32, level: PresentationLevel) -> Self {
        Self {
            width: if width < 0 { 0 } else { width },
            height: match level {
                PresentationLevel::Selvage => SELVAGE_HEIGHT,
                PresentationLevel::Ribbon | PresentationLevel::Panel => SURFACE_HEIGHT,
            },
        }
    }
}

/// GTK objects for one compositor output. This type remains on the main thread.
pub(crate) struct ManagedSurface {
    pub id: OutputId,
    pub monitor: gdk::Monitor,
    window: gtk::ApplicationWindow,
    widgets: TopEdgeWidgets,
    level: Rc<Cell<PresentationLevel>>,
    monitor_handler: Option<glib::SignalHandlerId>,
}

impl ManagedSurface {
    /// Configure every layer-shell property before presenting the window.
    pub fn new(
        application: &gtk::Application,
        monitor: &gdk::Monitor,
        id: OutputId,
        zone: ExclusiveZone,
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

        let widgets = TopEdgeWidgets::new(&window, id, action_sink);
        window.set_child(Some(&widgets.root));

        let level = Rc::new(Cell::new(PresentationLevel::Selvage));
        let layout_level = level.clone();
        window.connect_realize(move |window| {
            let Some(surface) = window.surface() else {
                return;
            };
            let configured_level = layout_level.clone();
            surface.connect_layout(move |surface, width, _| {
                apply_input_region_to_surface(surface, width, configured_level.get(), id, "layout");
            });
            apply_input_region(window, PresentationLevel::Selvage, id, "realize");
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
        apply_input_region(&self.window, level, self.id, "state");
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
    output: OutputId,
    source: &'static str,
) {
    let Some(surface) = window.surface() else {
        return;
    };
    apply_input_region_to_surface(&surface, surface.width(), level, output, source);
}

fn apply_input_region_to_surface(
    surface: &gdk::Surface,
    width: i32,
    level: PresentationLevel,
    output: OutputId,
    source: &'static str,
) {
    let geometry = InputRegionGeometry::for_level(width, level);
    let region = if geometry.width == 0 {
        cairo::Region::create()
    } else {
        let rectangle = cairo::RectangleInt::new(0, 0, geometry.width, geometry.height);
        cairo::Region::create_rectangle(&rectangle)
    };
    surface.set_input_region(Some(&region));
    tracing::debug!(
        output = ?output,
        source,
        width = geometry.width,
        height = geometry.height,
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
        assert_eq!(
            InputRegionGeometry::for_level(1920, PresentationLevel::Selvage),
            InputRegionGeometry {
                width: 1920,
                height: SELVAGE_HEIGHT,
            }
        );
    }

    #[test]
    fn expanded_regions_cover_the_fixed_surface() {
        for level in [PresentationLevel::Ribbon, PresentationLevel::Panel] {
            assert_eq!(
                InputRegionGeometry::for_level(1280, level),
                InputRegionGeometry {
                    width: 1280,
                    height: SURFACE_HEIGHT,
                }
            );
        }
    }

    #[test]
    fn unallocated_width_has_an_empty_input_region() {
        assert_eq!(
            InputRegionGeometry::for_level(0, PresentationLevel::Selvage).width,
            0
        );
    }
}
