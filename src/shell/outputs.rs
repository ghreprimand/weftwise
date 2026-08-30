//! Per-output layer-surface reconciliation.

use std::rc::Rc;

use relm4::gtk;
use relm4::gtk::gdk;
use relm4::gtk::gio;
use relm4::gtk::glib;
use relm4::gtk::prelude::*;
use thiserror::Error;

use crate::action::AppAction;
use crate::shell::ExclusiveZone;
use crate::state::{OutputId, OutputView, PresentationLevel};
use crate::widgets;

use super::surface::ManagedSurface;

/// Shell lifecycle messages sent to the authoritative root model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShellEvent {
    /// GDK's monitor list changed and must be reconciled.
    OutputsChanged,
    /// Allocation or scale changed and the GDK input region must be recomputed.
    GeometryChanged(OutputId),
}

/// Added and removed identities from one monitor reconciliation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OutputChanges {
    /// Newly created output identities.
    pub added: Vec<OutputId>,
    /// Removed output identities.
    pub removed: Vec<OutputId>,
    /// Current connector bindings for every retained surface.
    pub bindings: Vec<OutputBinding>,
}

/// GDK surface identity bound to a compositor connector.
#[derive(Clone, Eq, PartialEq)]
pub struct OutputBinding {
    /// Process-local surface identity.
    pub id: OutputId,
    /// Connector used only for in-process reconciliation.
    pub connector: Option<String>,
}

impl std::fmt::Debug for OutputBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OutputBinding")
            .field("id", &self.id)
            .field("connector", &self.connector.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// Native surface initialization failure.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum SurfaceError {
    /// No GDK display was available after GTK application startup.
    #[error("no GDK display is available")]
    NoDisplay,
    /// The active GDK backend does not support gtk4-layer-shell.
    #[error("gtk4-layer-shell is unsupported by the active display backend")]
    LayerShellUnsupported,
    /// The active GDK backend cannot restrict pointer input to the visible UI.
    #[error("the active display backend does not support input regions")]
    InputRegionsUnsupported,
}

/// Main-thread owner of one layer surface per current GDK monitor.
pub struct SurfaceManager {
    application: gtk::Application,
    monitors: gio::ListModel,
    monitor_handler: Option<glib::SignalHandlerId>,
    surfaces: Vec<ManagedSurface>,
    next_id: u64,
    zone: ExclusiveZone,
    action_sink: Rc<dyn Fn(AppAction)>,
    shell_sink: Rc<dyn Fn(ShellEvent)>,
}

impl SurfaceManager {
    /// Initialize display watching without creating GTK objects off the main thread.
    pub fn new(
        application: &gtk::Application,
        zone: ExclusiveZone,
        action_sink: Rc<dyn Fn(AppAction)>,
        shell_sink: Rc<dyn Fn(ShellEvent)>,
    ) -> Result<Self, SurfaceError> {
        let display = gdk::Display::default().ok_or(SurfaceError::NoDisplay)?;
        if !gtk4_layer_shell::is_supported() {
            return Err(SurfaceError::LayerShellUnsupported);
        }
        if !display.supports_input_shapes() {
            return Err(SurfaceError::InputRegionsUnsupported);
        }

        widgets::install_style(&display);
        let monitors = display.monitors();
        let changed_sink = shell_sink.clone();
        let monitor_handler = monitors.connect_items_changed(move |_, _, _, _| {
            changed_sink(ShellEvent::OutputsChanged);
        });

        Ok(Self {
            application: application.clone(),
            monitors,
            monitor_handler: Some(monitor_handler),
            surfaces: Vec::new(),
            next_id: 1,
            zone,
            action_sink,
            shell_sink,
        })
    }

    /// Reconcile the current GDK monitor snapshot into native surfaces.
    pub fn reconcile(&mut self) -> OutputChanges {
        let monitors = self.monitor_snapshot();
        let mut changes = OutputChanges::default();

        self.surfaces.retain_mut(|surface| {
            if monitors.iter().any(|monitor| monitor == &surface.monitor) {
                true
            } else {
                surface.close();
                changes.removed.push(surface.id);
                false
            }
        });

        for monitor in monitors {
            if self
                .surfaces
                .iter()
                .any(|surface| surface.monitor == monitor)
            {
                continue;
            }

            let id = OutputId::new(self.next_id);
            self.next_id = self.next_id.wrapping_add(1).max(1);
            let surface = ManagedSurface::new(
                &self.application,
                &monitor,
                id,
                self.zone,
                self.action_sink.clone(),
                self.shell_sink.clone(),
            );
            self.surfaces.push(surface);
            changes.added.push(id);
        }

        changes.bindings = self.surfaces.iter().map(ManagedSurface::binding).collect();

        changes
    }

    /// Render one output projection when it still has a native surface.
    pub fn render(&self, id: OutputId, view: &OutputView) {
        if let Some(surface) = self.surfaces.iter().find(|surface| surface.id == id) {
            surface.render(view);
        }
    }

    /// Recompute an output input region after allocation or scale notification.
    pub fn refresh_input_region(&self, id: OutputId, level: PresentationLevel) {
        if let Some(surface) = self.surfaces.iter().find(|surface| surface.id == id) {
            surface.refresh_input_region(level);
        }
    }

    /// Close every output surface and disconnect monitor observation.
    pub fn shutdown(&mut self) {
        if let Some(handler) = self.monitor_handler.take() {
            self.monitors.disconnect(handler);
        }
        for mut surface in self.surfaces.drain(..) {
            surface.close();
        }
    }

    fn monitor_snapshot(&self) -> Vec<gdk::Monitor> {
        (0..self.monitors.n_items())
            .filter_map(|index| self.monitors.item(index))
            .filter_map(|item| item.downcast::<gdk::Monitor>().ok())
            .filter(gdk::Monitor::is_valid)
            .collect()
    }
}

impl Drop for SurfaceManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}
