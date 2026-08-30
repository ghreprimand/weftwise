//! Relm4 application lifecycle and root message dispatch.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use relm4::gtk;
use relm4::gtk::glib;
use relm4::gtk::prelude::*;
use relm4::prelude::*;
use thiserror::Error;

use crate::APPLICATION_ID;
use crate::action::AppAction;
use crate::message::{AppMessage, TimerKind};
use crate::shell::outputs::{OutputChanges, ShellEvent, SurfaceError, SurfaceManager};
use crate::shell::{ProofOptionError, ProofOptions};
use crate::state::{
    AppState, DISMISS_DELAY, DWELL_DELAY, InteractionEffect, InteractionInput, InteractionToken,
    OutputId,
};
use crate::supervisor::{RuntimeConfigurationError, Supervisor, configure_relm_runtime};

/// Errors that prevent the application from starting.
#[derive(Debug, Error)]
pub enum StartupError {
    /// The shared Relm4 runtime was initialized with incompatible limits.
    #[error(transparent)]
    Runtime(#[from] RuntimeConfigurationError),
    /// A public-safe native-proof environment switch was invalid.
    #[error(transparent)]
    ProofOption(#[from] ProofOptionError),
    /// GTK could not initialize the active display backend.
    #[error("GTK could not initialize the active display backend")]
    GtkInitialization,
    /// GTK could not create supported layer surfaces.
    #[error(transparent)]
    Surface(#[from] SurfaceError),
}

struct AppInit {
    state: AppState,
    options: ProofOptions,
    startup_failure: Rc<RefCell<Option<SurfaceError>>>,
}

struct AppModel {
    state: AppState,
    options: ProofOptions,
    startup_failure: Rc<RefCell<Option<SurfaceError>>>,
    supervisor: Supervisor,
    surfaces: Option<SurfaceManager>,
    timers: UiTimers,
    animation_watch: Option<AnimationPreferenceWatch>,
    shutting_down: bool,
}

#[relm4::component]
impl SimpleComponent for AppModel {
    type Init = AppInit;
    type Input = AppMessage;
    type Output = ();

    view! {
        #[root]
        gtk::Window {
            set_title: Some("Weftwise lifecycle owner"),
            set_visible: false,
            connect_close_request[sender] => move |_| {
                sender.input(AppMessage::Shutdown);
                glib::Propagation::Stop
            },
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let widgets = view_output!();
        let mut supervisor = Supervisor::default();
        let shutdown_sender = sender.input_sender().clone();
        supervisor.spawn_adapter(move || async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                let _ = shutdown_sender.send(AppMessage::Shutdown);
            } else {
                tracing::error!("failed to install the process shutdown signal listener");
            }
        });

        let animation_watch = AnimationPreferenceWatch::new(sender.input_sender().clone());
        let reduced_motion = init.options.reduced_motion
            || animation_watch
                .as_ref()
                .is_none_or(|watch| !watch.animations_enabled());

        let action_sender = sender.input_sender().clone();
        let action_sink = Rc::new(move |action| {
            let _ = action_sender.send(AppMessage::Action(action));
        });
        let shell_sender = sender.input_sender().clone();
        let shell_sink = Rc::new(move |event| {
            let _ = shell_sender.send(AppMessage::Shell(event));
        });

        let mut state = init.state;
        let application = relm4::main_application();
        let surfaces = match SurfaceManager::new(
            &application,
            init.options.exclusive_zone,
            action_sink,
            shell_sink,
        ) {
            Ok(mut surfaces) => {
                let changes = surfaces.reconcile();
                state.reconcile_outputs(changes.added, changes.removed, reduced_motion);
                for id in state.output_ids() {
                    if let Some(presentation) = state.output(id) {
                        surfaces.render(id, presentation);
                    }
                }
                Some(surfaces)
            }
            Err(error) => {
                tracing::error!(reason = %error, "native surface startup failed");
                *init.startup_failure.borrow_mut() = Some(error);
                let failure_sender = sender.input_sender().clone();
                glib::idle_add_local_once(move || {
                    let _ = failure_sender.send(AppMessage::Shutdown);
                });
                None
            }
        };

        let model = Self {
            state,
            options: init.options,
            startup_failure: init.startup_failure,
            supervisor,
            surfaces,
            timers: UiTimers::default(),
            animation_watch,
            shutting_down: false,
        };

        ComponentParts { model, widgets }
    }

    fn update(&mut self, message: Self::Input, sender: ComponentSender<Self>) {
        match message {
            AppMessage::Action(action) => self.handle_action(action, &sender),
            AppMessage::Shell(event) => self.handle_shell_event(event),
            AppMessage::TimerElapsed {
                output,
                kind,
                token,
            } => self.handle_timer(output, kind, token, &sender),
            AppMessage::AnimationPreferenceChanged => self.update_motion_preference(&sender),
            AppMessage::Shutdown => self.shutdown_owned(),
        }
    }

    fn shutdown(&mut self, _widgets: &mut Self::Widgets, _output: relm4::Sender<Self::Output>) {
        self.shutdown_owned();
    }
}

impl AppModel {
    fn handle_action(&mut self, action: AppAction, sender: &ComponentSender<Self>) {
        let (output, input) = match action {
            AppAction::PointerEntered(output) => (output, InteractionInput::PointerEntered),
            AppAction::PointerLeft(output) => (output, InteractionInput::PointerLeft),
            AppAction::OpenPanel(output) => (output, InteractionInput::OpenPanel),
            AppAction::ClosePanel(output) => (output, InteractionInput::ClosePanel),
            AppAction::Quit => {
                self.shutdown_owned();
                return;
            }
        };
        self.apply_interaction(output, input, sender);
    }

    fn handle_shell_event(&mut self, event: ShellEvent) {
        match event {
            ShellEvent::OutputsChanged => {
                let changes = self
                    .surfaces
                    .as_mut()
                    .map(SurfaceManager::reconcile)
                    .unwrap_or_default();
                self.reconcile_output_state(changes);
            }
            ShellEvent::GeometryChanged(output) => {
                if let (Some(surfaces), Some(presentation)) =
                    (self.surfaces.as_ref(), self.state.output(output))
                {
                    surfaces.refresh_input_region(output, presentation.level());
                }
            }
        }
    }

    fn reconcile_output_state(&mut self, changes: OutputChanges) {
        let reduced_motion = self.effective_reduced_motion();
        for output in &changes.removed {
            self.timers.cancel_output(*output);
        }
        let added = changes.added.clone();
        self.state
            .reconcile_outputs(changes.added, changes.removed, reduced_motion);
        for output in added {
            self.render(output);
        }
    }

    fn handle_timer(
        &mut self,
        output: OutputId,
        kind: TimerKind,
        token: InteractionToken,
        sender: &ComponentSender<Self>,
    ) {
        let input = match kind {
            TimerKind::Dwell => InteractionInput::DwellElapsed(token),
            TimerKind::Dismiss => InteractionInput::DismissElapsed(token),
        };
        self.apply_interaction(output, input, sender);
    }

    fn apply_interaction(
        &mut self,
        output: OutputId,
        input: InteractionInput,
        sender: &ComponentSender<Self>,
    ) {
        let Some(presentation) = self.state.output_mut(output) else {
            return;
        };
        let effects = presentation.update(input);
        for effect in effects {
            match effect {
                InteractionEffect::ScheduleDwell(token) => self.timers.schedule(
                    output,
                    TimerKind::Dwell,
                    token,
                    DWELL_DELAY,
                    sender.input_sender().clone(),
                ),
                InteractionEffect::ScheduleDismiss(token) => self.timers.schedule(
                    output,
                    TimerKind::Dismiss,
                    token,
                    DISMISS_DELAY,
                    sender.input_sender().clone(),
                ),
                InteractionEffect::Render => self.render(output),
            }
        }
    }

    fn update_motion_preference(&mut self, sender: &ComponentSender<Self>) {
        let reduced_motion = self.effective_reduced_motion();
        let outputs: Vec<_> = self.state.output_ids().collect();
        for output in outputs {
            self.apply_interaction(
                output,
                InteractionInput::SetReducedMotion(reduced_motion),
                sender,
            );
        }
    }

    fn effective_reduced_motion(&self) -> bool {
        self.options.reduced_motion
            || self
                .animation_watch
                .as_ref()
                .is_none_or(|watch| !watch.animations_enabled())
    }

    fn render(&self, output: OutputId) {
        if let (Some(surfaces), Some(presentation)) =
            (self.surfaces.as_ref(), self.state.output(output))
        {
            surfaces.render(output, presentation);
        }
    }

    fn shutdown_owned(&mut self) {
        if self.shutting_down {
            return;
        }
        self.shutting_down = true;
        self.timers.shutdown();
        if let Some(mut surfaces) = self.surfaces.take() {
            surfaces.shutdown();
        }
        self.animation_watch.take();
        self.supervisor.shutdown();
        relm4::main_application().quit();
    }
}

impl Drop for AppModel {
    fn drop(&mut self) {
        self.shutdown_owned();
        tracing::debug!(
            schema_version = self.state.config.schema_version,
            startup_failed = self.startup_failure.borrow().is_some(),
            "root model stopped"
        );
    }
}

#[derive(Default)]
struct UiTimers {
    sources: Rc<RefCell<HashMap<(OutputId, TimerKind), glib::SourceId>>>,
}

impl UiTimers {
    fn schedule(
        &mut self,
        output: OutputId,
        kind: TimerKind,
        token: InteractionToken,
        delay: std::time::Duration,
        sender: relm4::Sender<AppMessage>,
    ) {
        if let Some(source) = self.sources.borrow_mut().remove(&(output, kind)) {
            source.remove();
        }
        let sources = self.sources.clone();
        let source = glib::timeout_add_local_once(delay, move || {
            sources.borrow_mut().remove(&(output, kind));
            let _ = sender.send(AppMessage::TimerElapsed {
                output,
                kind,
                token,
            });
        });
        self.sources.borrow_mut().insert((output, kind), source);
    }

    fn cancel_output(&mut self, output: OutputId) {
        let keys: Vec<_> = self
            .sources
            .borrow()
            .keys()
            .filter(|(candidate, _)| *candidate == output)
            .copied()
            .collect();
        for key in keys {
            if let Some(source) = self.sources.borrow_mut().remove(&key) {
                source.remove();
            }
        }
    }

    fn shutdown(&mut self) {
        for (_, source) in self.sources.borrow_mut().drain() {
            source.remove();
        }
    }
}

impl Drop for UiTimers {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct AnimationPreferenceWatch {
    settings: gtk::Settings,
    handler: Option<glib::SignalHandlerId>,
}

impl AnimationPreferenceWatch {
    fn new(sender: relm4::Sender<AppMessage>) -> Option<Self> {
        let settings = gtk::Settings::default()?;
        let handler = settings.connect_gtk_enable_animations_notify(move |_| {
            let _ = sender.send(AppMessage::AnimationPreferenceChanged);
        });
        Some(Self {
            settings,
            handler: Some(handler),
        })
    }

    fn animations_enabled(&self) -> bool {
        self.settings.is_gtk_enable_animations()
    }
}

impl Drop for AnimationPreferenceWatch {
    fn drop(&mut self) {
        if let Some(handler) = self.handler.take() {
            self.settings.disconnect(handler);
        }
    }
}

/// Configure bounded workers, diagnostics, and native output surfaces.
pub fn run() -> Result<(), StartupError> {
    configure_relm_runtime()?;
    let options = ProofOptions::from_environment()?;
    init_diagnostics();

    let startup_failure = Rc::new(RefCell::new(None));
    gtk::init().map_err(|_| StartupError::GtkInitialization)?;
    let application = gtk::Application::builder()
        .application_id(APPLICATION_ID)
        .build();
    let app = RelmApp::from_app(application).visible_on_activate(false);
    tracing::info!(
        exclusive_zone = options.exclusive_zone.value(),
        reduced_motion = options.reduced_motion,
        "starting native surface proof"
    );
    app.run::<AppModel>(AppInit {
        state: AppState::default(),
        options,
        startup_failure: startup_failure.clone(),
    });

    if let Some(error) = startup_failure.borrow_mut().take() {
        return Err(error.into());
    }
    tracing::info!("native surface proof stopped");
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
