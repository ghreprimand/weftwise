//! Relm4 application lifecycle.

use relm4::gtk;
use relm4::gtk::prelude::*;
use relm4::prelude::*;
use thiserror::Error;

use crate::APPLICATION_ID;
use crate::message::AppMessage;
use crate::state::AppState;
use crate::supervisor::{RuntimeConfigurationError, Supervisor, configure_relm_runtime};

/// Errors that prevent the application from starting.
#[derive(Debug, Error)]
pub enum StartupError {
    /// The shared Relm4 runtime was initialized with incompatible limits.
    #[error(transparent)]
    Runtime(#[from] RuntimeConfigurationError),
}

struct AppModel {
    state: AppState,
    supervisor: Supervisor,
}

#[relm4::component]
impl SimpleComponent for AppModel {
    type Init = AppState;
    type Input = AppMessage;
    type Output = ();

    view! {
        #[root]
        gtk::Window {
            set_title: Some("Weftwise"),
            set_visible: false,
        }
    }

    fn init(
        state: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let _ = sender;
        let model = Self {
            state,
            supervisor: Supervisor::default(),
        };
        let widgets = view_output!();

        ComponentParts { model, widgets }
    }

    fn update(&mut self, message: Self::Input, _sender: ComponentSender<Self>) {
        match message {
            AppMessage::Shutdown => {
                self.supervisor.shutdown();
                relm4::main_application().quit();
            }
        }
    }
}

impl Drop for AppModel {
    fn drop(&mut self) {
        self.supervisor.shutdown();
        tracing::debug!(
            schema_version = self.state.config.schema_version,
            "root model stopped"
        );
    }
}

/// Configure bounded workers, diagnostics, and the hidden Phase 0 root component.
pub fn run() -> Result<(), StartupError> {
    configure_relm_runtime()?;
    init_diagnostics();

    tracing::info!("starting application scaffold");
    RelmApp::new(APPLICATION_ID)
        .visible_on_activate(false)
        .run::<AppModel>(AppState::default());
    tracing::info!("application scaffold stopped");

    Ok(())
}

fn init_diagnostics() {
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::prelude::*;

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("weftwise=info"));
    let _subscriber_already_installed = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .try_init();
}
