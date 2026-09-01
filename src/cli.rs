//! Typed command-line entry points for local activity and compositor bindings.

use std::ffi::OsString;
use std::time::Duration;

use relm4::gtk::gio;
use relm4::gtk::gio::prelude::*;
use thiserror::Error;

use crate::APPLICATION_ID;
use crate::config::{ConfigPathError, ConfigPaths};
use crate::services::activity::transport::{ActivityTransportError, send_event};
use crate::services::activity::{
    ActivityCompletion, ActivityEvent, ActivityKind, ActivityOutcome, ActivityProtocolError,
    ActivityPublication, ActivityUpdate,
};

/// Exported GApplication action invoked by `weftwise reveal`.
pub const REVEAL_ACTION: &str = "reveal";

/// Public command help. Values are intentionally described as display data.
pub const USAGE: &str = "Usage:\n\
  weftwise\n\
  weftwise reveal\n\
  weftwise activity publish <id> <kind> <label> [--accessible-label <text>] [--progress-bp <0-10000>] [--expires-ms <milliseconds>]\n\
  weftwise activity update <id> [--label <text>] [--accessible-label <text>] [--progress-bp <0-10000>] [--expires-ms <milliseconds>]\n\
  weftwise activity complete <id> <succeeded|failed> [--label <text>]\n\
  weftwise activity cancel <id>\n\
\n\
Activity kinds: timer, build, download, render, command-result.\n\
Invoke `weftwise reveal` twice within 1500 ms to pin the Ribbon until focus leaves it.\n\
Labels are bounded display data. Commands, argument vectors, and shell strings are not accepted.";

/// Parsed command that has not yet touched GTK, D-Bus, or the local endpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CliCommand {
    /// Start the native application when no command was supplied.
    LaunchApplication,
    /// Ask the already-running application to reveal the focused Ribbon.
    Reveal,
    /// Send one validated local activity event.
    Activity(ActivityEvent),
    /// Print bounded static usage text.
    Help,
    /// Print the package version.
    Version,
}

/// Result of dispatching process arguments.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CliDisposition {
    /// Continue into normal GTK application startup.
    LaunchApplication,
    /// The command completed without starting another native surface owner.
    Complete,
}

/// Public-safe CLI failure that never retains argument values.
#[derive(Debug, Error)]
pub enum CliError {
    /// One argument could not be interpreted as UTF-8.
    #[error("a command-line argument is not valid UTF-8")]
    NonUtf8Argument,
    /// Command shape, option, enum, or number was invalid.
    #[error("invalid command-line invocation; run `weftwise --help`")]
    InvalidInvocation,
    /// The typed activity constructor rejected a bounded value.
    #[error(transparent)]
    Protocol(#[from] ActivityProtocolError),
    /// XDG locations could not be resolved safely.
    #[error(transparent)]
    ConfigPath(#[from] ConfigPathError),
    /// The authenticated activity endpoint rejected or did not acknowledge the request.
    #[error(transparent)]
    Transport(#[from] ActivityTransportError),
    /// The session bus could not register the remote application launcher.
    #[error("the Weftwise application action could not be registered")]
    RemoteRegistration(#[source] relm4::gtk::glib::Error),
    /// No primary Weftwise GTK instance owns the application identifier.
    #[error("Weftwise is not running in this desktop session")]
    ApplicationNotRunning,
    /// The running instance does not expose the expected typed action.
    #[error("the running Weftwise instance does not support reveal")]
    RevealUnavailable,
}

/// Parse process arguments, execute non-GTK commands, and select normal startup.
pub fn dispatch_from_environment() -> Result<CliDisposition, CliError> {
    match parse_arguments(std::env::args_os().skip(1))? {
        CliCommand::LaunchApplication => Ok(CliDisposition::LaunchApplication),
        CliCommand::Reveal => {
            request_reveal()?;
            Ok(CliDisposition::Complete)
        }
        CliCommand::Activity(event) => {
            let paths = ConfigPaths::discover()?;
            send_event(&paths, &event)?;
            Ok(CliDisposition::Complete)
        }
        CliCommand::Help => {
            println!("{USAGE}");
            Ok(CliDisposition::Complete)
        }
        CliCommand::Version => {
            println!("weftwise {}", env!("CARGO_PKG_VERSION"));
            Ok(CliDisposition::Complete)
        }
    }
}

/// Parse an explicit argument sequence for deterministic tests and dispatch.
pub fn parse_arguments(
    arguments: impl IntoIterator<Item = impl Into<OsString>>,
) -> Result<CliCommand, CliError> {
    let arguments = arguments
        .into_iter()
        .map(|argument| {
            argument
                .into()
                .into_string()
                .map_err(|_| CliError::NonUtf8Argument)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let Some((command, rest)) = arguments.split_first() else {
        return Ok(CliCommand::LaunchApplication);
    };
    match command.as_str() {
        "reveal" if rest.is_empty() => Ok(CliCommand::Reveal),
        "activity" => parse_activity(rest),
        "help" | "--help" | "-h" if rest.is_empty() => Ok(CliCommand::Help),
        "--version" | "-V" if rest.is_empty() => Ok(CliCommand::Version),
        _ => Err(CliError::InvalidInvocation),
    }
}

fn parse_activity(arguments: &[String]) -> Result<CliCommand, CliError> {
    let Some((operation, rest)) = arguments.split_first() else {
        return Err(CliError::InvalidInvocation);
    };
    let event = match operation.as_str() {
        "publish" => parse_publish(rest)?,
        "update" => parse_update(rest)?,
        "complete" => parse_complete(rest)?,
        "cancel" => parse_cancel(rest)?,
        _ => return Err(CliError::InvalidInvocation),
    };
    Ok(CliCommand::Activity(event))
}

fn parse_publish(arguments: &[String]) -> Result<ActivityEvent, CliError> {
    let [id, kind, label, options @ ..] = arguments else {
        return Err(CliError::InvalidInvocation);
    };
    let kind = parse_kind(kind)?;
    let mut accessible_label = None;
    let mut progress = None;
    let mut lifetime = None;
    parse_options(options, |option, value| match option {
        "--accessible-label" if accessible_label.is_none() => {
            accessible_label = Some(value);
            Ok(())
        }
        "--progress-bp" if progress.is_none() => {
            progress = Some(parse_number(value)?);
            Ok(())
        }
        "--expires-ms" if lifetime.is_none() => {
            lifetime = Some(Duration::from_millis(parse_number(value)?));
            Ok(())
        }
        _ => Err(CliError::InvalidInvocation),
    })?;
    ActivityPublication::new(id, kind, label, accessible_label, progress, lifetime)
        .map(ActivityEvent::Publish)
        .map_err(CliError::from)
}

fn parse_update(arguments: &[String]) -> Result<ActivityEvent, CliError> {
    let Some((id, options)) = arguments.split_first() else {
        return Err(CliError::InvalidInvocation);
    };
    let mut label = None;
    let mut accessible_label = None;
    let mut progress = None;
    let mut lifetime = None;
    parse_options(options, |option, value| match option {
        "--label" if label.is_none() => {
            label = Some(value);
            Ok(())
        }
        "--accessible-label" if accessible_label.is_none() => {
            accessible_label = Some(value);
            Ok(())
        }
        "--progress-bp" if progress.is_none() => {
            progress = Some(parse_number(value)?);
            Ok(())
        }
        "--expires-ms" if lifetime.is_none() => {
            lifetime = Some(Duration::from_millis(parse_number(value)?));
            Ok(())
        }
        _ => Err(CliError::InvalidInvocation),
    })?;
    ActivityUpdate::new(id, label, accessible_label, progress, lifetime)
        .map(ActivityEvent::Update)
        .map_err(CliError::from)
}

fn parse_complete(arguments: &[String]) -> Result<ActivityEvent, CliError> {
    let [id, outcome, options @ ..] = arguments else {
        return Err(CliError::InvalidInvocation);
    };
    let outcome = match outcome.as_str() {
        "succeeded" => ActivityOutcome::Succeeded,
        "failed" => ActivityOutcome::Failed,
        _ => return Err(CliError::InvalidInvocation),
    };
    let mut label = None;
    parse_options(options, |option, value| match option {
        "--label" if label.is_none() => {
            label = Some(value);
            Ok(())
        }
        _ => Err(CliError::InvalidInvocation),
    })?;
    ActivityCompletion::new(id, outcome, label)
        .map(ActivityEvent::Complete)
        .map_err(CliError::from)
}

fn parse_cancel(arguments: &[String]) -> Result<ActivityEvent, CliError> {
    let [id] = arguments else {
        return Err(CliError::InvalidInvocation);
    };
    ActivityEvent::cancel(id).map_err(CliError::from)
}

fn parse_options<'a>(
    arguments: &'a [String],
    mut apply: impl FnMut(&str, &'a str) -> Result<(), CliError>,
) -> Result<(), CliError> {
    let mut chunks = arguments.chunks_exact(2);
    for chunk in &mut chunks {
        apply(&chunk[0], &chunk[1])?;
    }
    if chunks.remainder().is_empty() {
        Ok(())
    } else {
        Err(CliError::InvalidInvocation)
    }
}

fn parse_kind(value: &str) -> Result<ActivityKind, CliError> {
    match value {
        "timer" => Ok(ActivityKind::Timer),
        "build" => Ok(ActivityKind::Build),
        "download" => Ok(ActivityKind::Download),
        "render" => Ok(ActivityKind::Render),
        "command-result" => Ok(ActivityKind::CommandResult),
        _ => Err(CliError::InvalidInvocation),
    }
}

fn parse_number<T: std::str::FromStr>(value: &str) -> Result<T, CliError> {
    value.parse().map_err(|_| CliError::InvalidInvocation)
}

fn request_reveal() -> Result<(), CliError> {
    let application =
        gio::Application::new(Some(APPLICATION_ID), gio::ApplicationFlags::IS_LAUNCHER);
    application
        .register(None::<&gio::Cancellable>)
        .map_err(CliError::RemoteRegistration)?;
    if !application.is_remote() {
        return Err(CliError::ApplicationNotRunning);
    }
    if !application.has_action(REVEAL_ACTION) {
        return Err(CliError::RevealUnavailable);
    }
    application.activate_action(REVEAL_ACTION, None);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_arguments_preserve_native_application_startup() {
        assert_eq!(
            parse_arguments(Vec::<String>::new()).expect("no arguments"),
            CliCommand::LaunchApplication
        );
    }

    #[test]
    fn reveal_and_static_information_require_no_values() {
        assert_eq!(
            parse_arguments(["reveal"]).expect("reveal"),
            CliCommand::Reveal
        );
        assert_eq!(parse_arguments(["--help"]).expect("help"), CliCommand::Help);
        assert!(parse_arguments(["reveal", "unexpected"]).is_err());
    }

    #[test]
    fn all_activity_operations_construct_typed_events() {
        let commands = [
            vec![
                "activity",
                "publish",
                "build.synthetic",
                "build",
                "Synthetic build",
                "--progress-bp",
                "1250",
                "--expires-ms",
                "60000",
            ],
            vec![
                "activity",
                "update",
                "build.synthetic",
                "--label",
                "Synthetic update",
            ],
            vec!["activity", "complete", "build.synthetic", "succeeded"],
            vec!["activity", "cancel", "build.synthetic"],
        ];
        for command in commands {
            assert!(matches!(
                parse_arguments(command).expect("typed operation"),
                CliCommand::Activity(_)
            ));
        }
    }

    #[test]
    fn unknown_duplicate_empty_and_shell_shaped_options_are_rejected() {
        for command in [
            vec!["activity", "update", "build.synthetic"],
            vec![
                "activity",
                "update",
                "build.synthetic",
                "--label",
                "First",
                "--label",
                "Second",
            ],
            vec![
                "activity",
                "publish",
                "build.synthetic",
                "build",
                "Synthetic",
                "--command",
                "synthetic-tool --flag",
            ],
            vec!["activity", "cancel", "build.synthetic", "extra"],
        ] {
            assert!(parse_arguments(command).is_err());
        }
    }
}
