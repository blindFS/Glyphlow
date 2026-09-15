//! Command line client for the Glyphlow server.
//!
//! Parses its arguments with `clap`, translates them into an [`AppSignal`] and
//! hands that to the running `glyphlow` server over its Unix socket.

use clap::{Parser, Subcommand, ValueEnum};
use glyphlow::{AppSignal, ax_element::Target, ipc};

/// Command line client for the Glyphlow server.
///
/// Requires a `glyphlow` server to be running; the request is fire-and-forget.
#[derive(Debug, Parser)]
#[command(
    name = "glyphlow-cli",
    version,
    about = "Command line client for the Glyphlow server",
    long_about = "Command line client for the Glyphlow server.\n\n\
                  Requests are fire-and-forget: they are handed to the running server \
                  over a Unix socket and no result is sent back. A zero exit status \
                  therefore means the request was delivered, not that the action \
                  succeeded. The server reports failures (unknown workflow name, \
                  unsuitable element, ...) as an on-screen notification."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Activate elements of a kind and pick one interactively
    Activate {
        /// Kind of elements to look for
        #[arg(value_enum)]
        kind: ActivationKind,
    },
    /// Run a pre-defined workflow, matched by (a fragment of) its display name
    Workflow {
        /// Display name of the workflow, e.g. "ProofRead"
        name: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ActivationKind {
    /// Clickable elements, such as buttons and menu items
    Clickable,
    /// Static text
    Text,
    /// Images
    Image,
}

impl ActivationKind {
    fn to_target(self) -> Target {
        match self {
            ActivationKind::Clickable => Target::Clickable,
            ActivationKind::Text => Target::Text,
            ActivationKind::Image => Target::Image,
        }
    }
}

impl std::fmt::Display for ActivationKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ActivationKind::Clickable => "clickable",
            ActivationKind::Text => "text",
            ActivationKind::Image => "image",
        })
    }
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();

    let (signal, confirmation) = match cli.command {
        Command::Activate { kind } => (
            AppSignal::Activate(kind.to_target()),
            format!("Requested activation of {kind} elements."),
        ),
        Command::Workflow { name } => {
            let signal = AppSignal::RunWorkFlowByName(name.clone());
            (signal, format!("Requested workflow matching `{name}`."))
        }
    };

    match ipc::send_signal(&signal).await {
        Ok(()) => {
            println!("{confirmation}");
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("glyphlow-cli: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
