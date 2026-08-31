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
use crate::config::{Config, ConfigLoadError, ConfigPathError, ConfigPaths};
use crate::context::arbitration::CandidateAction;
use crate::message::{AppMessage, TimerKind};
#[cfg(feature = "audio-transport")]
use crate::services::audio::AudioCommand;
use crate::services::mpris::{self, MediaCommand, MediaCommandKind};
use crate::services::{activity, clock, hyprland, logind};
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
    /// XDG configuration locations could not be resolved safely.
    #[error(transparent)]
    ConfigPath(#[from] ConfigPathError),
    /// The bounded versioned configuration could not be loaded.
    #[error(transparent)]
    Config(#[from] ConfigLoadError),
    /// GTK could not initialize the active display backend.
    #[error("GTK could not initialize the active display backend")]
    GtkInitialization,
    /// GTK could not create supported layer surfaces.
    #[error(transparent)]
    Surface(#[from] SurfaceError),
}

struct AppInit {
    state: AppState,
    config_paths: ConfigPaths,
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
    media_commands: tokio::sync::mpsc::Sender<MediaCommand>,
    #[cfg(feature = "audio-transport")]
    _audio_commands: tokio::sync::mpsc::Sender<AudioCommand>,
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
        let hyprland_sender = sender.input_sender().clone();
        supervisor.spawn_cancellable_adapter(move |cancellation| async move {
            hyprland::run(
                move |update| {
                    let _ = hyprland_sender.send(AppMessage::Hyprland(update));
                },
                cancellation,
            )
            .await;
        });
        let (media_commands, media_receiver) = mpris::command_channel();
        let media_sender = sender.input_sender().clone();
        supervisor.spawn_cancellable_adapter(move |cancellation| async move {
            mpris::run(
                move |update| {
                    let _ = media_sender.send(AppMessage::Media(update));
                },
                media_receiver,
                cancellation,
            )
            .await;
        });
        #[cfg(feature = "audio-transport")]
        let audio_commands = {
            let (audio_commands, audio_receiver) = crate::services::audio::command_channel();
            let audio_sender = sender.input_sender().clone();
            let capture_sender = sender.input_sender().clone();
            supervisor.spawn_cancellable_adapter(move |cancellation| async move {
                crate::services::audio::run(
                    move |update| {
                        let _ = audio_sender.send(AppMessage::Audio(update));
                    },
                    move |observation| {
                        let _ = capture_sender.send(AppMessage::Privacy {
                            update: observation.update,
                            observed_millis: observation.observed_millis,
                        });
                    },
                    audio_receiver,
                    cancellation,
                )
                .await;
            });
            audio_commands
        };
        let privacy_sender = sender.input_sender().clone();
        supervisor.spawn_cancellable_adapter(move |cancellation| async move {
            logind::run(
                move |observation| {
                    let _ = privacy_sender.send(AppMessage::Privacy {
                        update: observation.update,
                        observed_millis: observation.observed_millis,
                    });
                },
                cancellation,
            )
            .await;
        });
        let clock_sender = sender.input_sender().clone();
        supervisor.spawn_cancellable_adapter(move |cancellation| async move {
            clock::run(
                move |tick| {
                    let _ = clock_sender.send(AppMessage::Clock(tick));
                },
                cancellation,
            )
            .await;
        });
        let activity_sender = sender.input_sender().clone();
        let activity_paths = init.config_paths.clone();
        supervisor.spawn_cancellable_adapter(move |cancellation| async move {
            match activity::transport::ActivityEndpoint::bind(&activity_paths).await {
                Ok(endpoint) => {
                    endpoint
                        .run(
                            move |observation| {
                                let _ = activity_sender.send(AppMessage::Activity(observation));
                            },
                            cancellation,
                        )
                        .await;
                }
                Err(error) => {
                    tracing::warn!(reason = %error, "local activity endpoint unavailable");
                }
            }
        });

        let animation_watch = AnimationPreferenceWatch::new(sender.input_sender().clone());
        let reduced_motion = init.options.reduced_motion
            || init.state.config.reduced_motion.unwrap_or(false)
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
            state.config.clone(),
            action_sink,
            shell_sink,
        ) {
            Ok(mut surfaces) => {
                let changes = surfaces.reconcile();
                state.reconcile_outputs(changes.added, changes.removed, reduced_motion);
                state.bind_outputs(
                    changes
                        .bindings
                        .iter()
                        .map(|binding| (binding.id, binding.connector.clone())),
                );
                for id in state.output_ids() {
                    if let Some(view) = state.output_view(id) {
                        surfaces.render(id, &view);
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
            media_commands,
            #[cfg(feature = "audio-transport")]
            _audio_commands: audio_commands,
            shutting_down: false,
        };

        ComponentParts { model, widgets }
    }

    fn update(&mut self, message: Self::Input, sender: ComponentSender<Self>) {
        match message {
            AppMessage::Action(action) => self.handle_action(action, &sender),
            AppMessage::Shell(event) => self.handle_shell_event(event),
            AppMessage::Hyprland(update) => {
                let outputs = self.state.apply_hyprland_update(update);
                self.render_outputs(outputs);
            }
            AppMessage::Clock(tick) => self.handle_clock(tick),
            AppMessage::Media(update) => {
                let outputs = self.state.apply_media_update(update);
                self.render_outputs(outputs);
            }
            AppMessage::Audio(update) => {
                let outputs = self.state.apply_audio_update(update);
                self.render_outputs(outputs);
            }
            AppMessage::Activity(observation) => {
                let outputs = self.state.apply_activity_observation(observation);
                self.render_outputs(outputs);
            }
            AppMessage::Privacy {
                update,
                observed_millis,
            } => {
                let outputs = self.state.apply_privacy_update(update, observed_millis);
                self.render_outputs(outputs);
            }
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
        tracing::debug!(?action, "received native surface action");
        let (output, input) = match action {
            AppAction::PointerEntered(output) => (output, InteractionInput::PointerEntered),
            AppAction::PointerEnteredImmediate(output) => {
                (output, InteractionInput::PointerEnteredImmediate)
            }
            AppAction::PointerLeft(output) => (output, InteractionInput::PointerLeft),
            AppAction::OpenPanel(output) => (output, InteractionInput::OpenPanel),
            AppAction::ClosePanel(output) => (output, InteractionInput::ClosePanel),
            AppAction::Candidate(output, action) => {
                self.handle_candidate_action(output, action);
                return;
            }
            AppAction::Quit => {
                self.shutdown_owned();
                return;
            }
        };
        self.apply_interaction(output, input, sender);
    }

    fn handle_candidate_action(&self, output: OutputId, action: CandidateAction) {
        let advertised = self
            .state
            .output_view(output)
            .is_some_and(|view| view.candidate_actions.contains(&action));
        if !advertised {
            return;
        }
        let Some(player) = self.state.selected_media_player(output) else {
            return;
        };
        let kind = match action {
            CandidateAction::MediaPlayPause => MediaCommandKind::PlayPause,
            CandidateAction::MediaPrevious => MediaCommandKind::Previous,
            CandidateAction::MediaNext => MediaCommandKind::Next,
            CandidateAction::MediaSeek(delta) => MediaCommandKind::SeekMillis(delta),
            CandidateAction::RevealDetails | CandidateAction::Dismiss => return,
        };
        if self
            .media_commands
            .try_send(MediaCommand {
                player: player.id.clone(),
                owner_generation: player.owner_generation,
                kind,
            })
            .is_err()
        {
            tracing::warn!("MPRIS command queue unavailable or full");
        }
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
        let bindings = changes.bindings;
        self.state
            .reconcile_outputs(changes.added, changes.removed, reduced_motion);
        self.state.bind_outputs(
            bindings
                .into_iter()
                .map(|binding| (binding.id, binding.connector)),
        );
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
            || self.state.config.reduced_motion.unwrap_or(false)
            || self
                .animation_watch
                .as_ref()
                .is_none_or(|watch| !watch.animations_enabled())
    }

    fn render(&self, output: OutputId) {
        if let (Some(surfaces), Some(view)) =
            (self.surfaces.as_ref(), self.state.output_view(output))
        {
            surfaces.render(output, &view);
        }
    }

    fn render_outputs(&self, outputs: impl IntoIterator<Item = OutputId>) {
        for output in outputs {
            self.render(output);
        }
    }

    fn handle_clock(&mut self, tick: clock::ClockTick) {
        let label = glib::DateTime::from_unix_local(tick.unix_seconds)
            .and_then(|value| value.format("%H:%M"))
            .map(|value| value.to_string())
            .unwrap_or_else(|_| "--:--".to_owned());
        let outputs = self.state.set_clock_label(label);
        self.render_outputs(outputs);
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
    let config_paths = ConfigPaths::discover()?;
    let config = Config::load(&config_paths.config_file)?;
    init_diagnostics();

    let startup_failure = Rc::new(RefCell::new(None));
    gtk::init().map_err(|_| StartupError::GtkInitialization)?;
    let application = gtk::Application::builder()
        .application_id(APPLICATION_ID)
        .build();
    let app = RelmApp::from_app(application).visible_on_activate(false);
    let configured_reduced_motion = config.reduced_motion.unwrap_or(false);
    tracing::info!(
        exclusive_zone = options.exclusive_zone.value(),
        reduced_motion = options.reduced_motion || configured_reduced_motion,
        "starting native surface proof"
    );
    let mut state = AppState::default();
    state.config = config;
    app.run::<AppModel>(AppInit {
        state,
        config_paths,
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
