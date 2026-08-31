//! Pure three-region Ribbon label projection.

use crate::context::arbitration::PresentationProjection;

use super::{AppState, CompositorOutput};

impl AppState {
    pub(super) fn ribbon_region_labels(
        &self,
        compositor_output: Option<&CompositorOutput>,
        focused: bool,
        detailed_candidate: Option<&PresentationProjection>,
    ) -> (String, String, String) {
        let workspace = compositor_output
            .and_then(|output| output.active_workspace)
            .and_then(|workspace| self.desktop.workspaces.get(&workspace))
            .map(|workspace| workspace.name.as_str().to_owned())
            .filter(|label| !label.is_empty())
            .unwrap_or_default();
        let active_client = self
            .desktop
            .active
            .client
            .as_ref()
            .and_then(|address| self.desktop.clients.get(address))
            .map(|client| client.title.as_str())
            .filter(|title| !title.is_empty())
            .or_else(|| {
                (!self.desktop.active.title.is_empty())
                    .then_some(self.desktop.active.title.as_str())
            })
            .unwrap_or_default();

        let navigation = if self.config.ribbon.show_workspace {
            workspace
        } else {
            String::new()
        };
        let context = if self.config.ribbon.show_context {
            detailed_candidate
                .map(|projection| projection.label.clone())
                .filter(|label| !label.is_empty())
                .or_else(|| focused.then(|| active_client.to_owned()))
                .unwrap_or_default()
        } else {
            String::new()
        };
        let status = if self.config.ribbon.show_clock {
            self.clock_fallback()
        } else {
            String::new()
        };

        (navigation, context, status)
    }
}
