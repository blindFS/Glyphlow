//! Command line client for the Glyphlow server.
//!
//! Parses its arguments with `clap`, translates them into an [`AppSignal`] and
//! hands that to the running `glyphlow` server over its Unix socket.
//!
//! `workflow list` and `complete` are the exceptions: they never talk to the
//! server. The first reads the config from disk, the second just prints a shell
//! completion script derived from the command definition.

use std::{io::Write, process::ExitCode};

use clap::{
    CommandFactory, Parser, Subcommand, ValueEnum,
    builder::styling::{AnsiColor, Effects, Style, Styles},
};
use clap_complete::{Shell as CompletionShell, generate};
use glyphlow::{
    AppSignal,
    ax_element::Target,
    config::{GlyphlowConfig, WorkFlow, get_config_path},
    ipc,
};
use tabled::{
    Table,
    // clap's `Style` is already in scope; this one is the table's.
    settings::{Color, Modify, Padding, Style as TableStyle, object::Rows},
};

// ---------------------------------------------------------------------------
// Styling
// ---------------------------------------------------------------------------

/// Palette for clap's own output.
const HEADER: Style = AnsiColor::Cyan.on_default().effects(Effects::BOLD);
const LITERAL: Style = AnsiColor::Yellow.on_default().effects(Effects::BOLD);
const DIM: Style = AnsiColor::BrightBlack.on_default();
const ERROR: Style = AnsiColor::Red.on_default().effects(Effects::BOLD);
const VALID: Style = AnsiColor::Green.on_default().effects(Effects::BOLD);
const INVALID: Style = AnsiColor::Yellow.on_default().effects(Effects::BOLD);

const STYLES: Styles = Styles::styled()
    .header(HEADER)
    .usage(HEADER)
    .literal(LITERAL)
    .placeholder(DIM)
    .error(ERROR)
    .valid(VALID)
    .invalid(INVALID);

fn table_header_color() -> Color {
    Color::FG_CYAN | Color::BOLD
}

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

/// Command line client for the Glyphlow server.
///
/// Requires a `glyphlow` server to be running; requests are fire-and-forget.
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
                  unsuitable element, ...) as an on-screen notification.\n\n\
                  `workflow list` and `complete` are the exceptions: they answer \
                  locally and never contact the server.",
    styles = STYLES,
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
    /// Inspect or run pre-defined workflows
    Workflow {
        #[command(subcommand)]
        command: WorkflowCommand,
    },
    /// Print a shell completion script to stdout
    Complete {
        /// Shell to emit completions for
        #[arg(value_enum)]
        shell: CompletionShell,
    },
}

#[derive(Debug, Subcommand)]
enum WorkflowCommand {
    /// List the configured workflows
    List,
    /// Run a workflow, matched by (a fragment of) its display name
    Run {
        /// Display name of the workflow, e.g. "ProofRead"
        name: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ActivationKind {
    Clickable,
    Text,
    Image,
    Ocr,
    Input,
    Editor,
    ChildElement,
    Scrollable,
}

impl ActivationKind {
    fn to_target(self) -> Target {
        match self {
            ActivationKind::Clickable => Target::Clickable,
            ActivationKind::Text => Target::Text,
            ActivationKind::Image => Target::Image,
            ActivationKind::Ocr => Target::ImageOCR,
            ActivationKind::Input => Target::Editable,
            ActivationKind::Editor => Target::Edit,
            ActivationKind::ChildElement => Target::ChildElement,
            ActivationKind::Scrollable => Target::Scrollable,
        }
    }
}

impl std::fmt::Display for ActivationKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ActivationKind::Clickable => "clickable",
            ActivationKind::Text => "text",
            ActivationKind::Image => "image",
            ActivationKind::Ocr => "ocr",
            ActivationKind::Input => "input",
            ActivationKind::Editor => "editor",
            ActivationKind::ChildElement => "child-element",
            ActivationKind::Scrollable => "scrollable",
        })
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Command::Activate { kind } => {
            let signal = AppSignal::Activate(kind.to_target());
            send(signal, format!("Requested activation of {kind} elements.")).await
        }
        Command::Workflow { command } => match command {
            WorkflowCommand::List => list_workflows(),
            WorkflowCommand::Run { name } => {
                let signal = AppSignal::RunWorkFlowByName(name.clone());
                send(signal, format!("Requested workflow matching `{name}`.")).await
            }
        },
        Command::Complete { shell } => {
            print_completions(shell);
            ExitCode::SUCCESS
        }
    }
}

async fn send(signal: AppSignal, confirmation: String) -> ExitCode {
    match ipc::send_signal(&signal).await {
        Ok(()) => {
            write_stdout(format!("{confirmation}\n").as_bytes());
            ExitCode::SUCCESS
        }
        Err(e) => {
            write_stderr(format!("glyphlow-cli: {e}\n").as_bytes());
            ExitCode::FAILURE
        }
    }
}

/// Write to stdout without panicking on a closed pipe, and let `anstream` drop
/// ANSI escapes when they would be unwelcome.
///
/// Rust ignores `SIGPIPE`, so a plain `print!` turns `glyphlow-cli complete bash
/// | head` into an EPIPE panic, while producing less output than the reader
/// wanted is a normal way for a pipeline to end. Routing through `anstream` also
/// keeps `--help` and `workflow list` in agreement about `NO_COLOR`, `CLICOLOR`,
/// `TERM=dumb` and CI, because clap filters its own output the same way.
fn write_stdout(bytes: &[u8]) {
    let mut stdout = anstream::stdout().lock();
    let _ = stdout.write_all(bytes);
    let _ = stdout.flush();
}

/// Same as [`write_stdout`], for the same reasons. Nothing written here is
/// coloured today.
fn write_stderr(bytes: &[u8]) {
    let mut stderr = anstream::stderr().lock();
    let _ = stderr.write_all(bytes);
    let _ = stderr.flush();
}

fn print_completions(shell: CompletionShell) {
    // Written in one go: a partial script on a closed pipe is not worth
    // reporting, and splitting the write would only widen the window.
    write_stdout(completion_script(shell).as_bytes());
}

/// Build the completion script for `shell`.
///
/// Split out from [`print_completions`] so tests can assert on the script
/// rather than having to capture stdout.
fn completion_script(shell: CompletionShell) -> String {
    let mut command = Cli::command();
    let bin_name = command.get_name().to_owned();
    // Generated into a buffer first so the write can be error-tolerant; clap's
    // own writer panics on a broken pipe.
    let mut buf: Vec<u8> = Vec::new();

    generate(shell, &mut command, bin_name, &mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

fn list_workflows() -> ExitCode {
    // Deliberately not `load_config`: listing must not seed a config file.
    let workflows =
        match get_config_path().and_then(|path| GlyphlowConfig::load_config_readonly(&path)) {
            Ok(config) => config.workflows,
            Err(e) => {
                write_stderr(format!("glyphlow-cli: {e}\n").as_bytes());
                return ExitCode::FAILURE;
            }
        };

    if workflows.is_empty() {
        write_stdout("No workflows configured.\n".as_bytes());
        return ExitCode::SUCCESS;
    }

    let table = render_workflow_table(&workflows);
    write_stdout(table.as_bytes());
    ExitCode::SUCCESS
}

fn render_workflow_table(workflows: &[WorkFlow]) -> String {
    let mut table = Table::new(workflows);
    table.with(TableStyle::markdown());
    table.with(Padding::new(0, 2, 0, 0));
    table.with(Modify::new(Rows::first()).with(table_header_color()));

    format!("{table}\n")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ansi_str::AnsiStr;
    use glyphlow::config::RoleOfInterest;
    use rstest::rstest;
    use unicode_width::UnicodeWidthStr;

    fn workflow(display: &str, key: &str, role: RoleOfInterest, apps: Option<&[&str]>) -> WorkFlow {
        WorkFlow {
            display: display.to_owned(),
            key: key.to_owned(),
            valid_app_ids: apps.map(|ids| ids.iter().map(|s| s.to_string()).collect()),
            starting_role: role,
            actions: vec![],
        }
    }

    /// Display column at which `needle` starts. Byte offsets are useless here:
    /// rows with multi-byte characters have different byte lengths for the same
    /// display width.
    fn column_of(line: &str, needle: &str) -> usize {
        let idx = line.find(needle).expect("needle not found");
        line[..idx].width()
    }

    /// The table's data rows, with the header and the markdown rule skipped.
    ///
    /// `Style::markdown()` puts a `|---|` rule on line 1, so the body starts at
    /// line 2 rather than line 1. Going through this helper keeps that from
    /// being spelled out — and mis-spelled — in every layout assertion.
    fn body_lines(table: &str) -> Vec<&str> {
        table.lines().skip(2).collect()
    }

    /// Colouring must not disturb the text or the padding.
    ///
    /// The expectation is written out rather than derived from the renderer, so
    /// that this is a check on the output and not a tautology.
    #[test]
    fn test_table_colors_are_the_only_difference() {
        let rows = [
            workflow("ProofRead", "p", RoleOfInterest::Any, None),
            workflow("短", "q", RoleOfInterest::Generic, Some(&["Notes"])),
        ];
        let colored = render_workflow_table(&rows);

        assert!(colored.ansi_has_any(), "no escapes in: {colored:?}");
        assert_eq!(
            colored.ansi_strip(),
            "\
|NAME       |KEY  |ROLE     |APPS   |STEPS  |
|-----------|-----|---------|-------|-------|
|ProofRead  |p    |Any      |any    |0      |
|短         |q    |Generic  |Notes  |0      |
"
        );
    }

    /// Column alignment must follow *display* width, not bytes, or a CJK or
    /// nerd-font name would shift every later column.
    #[test]
    fn test_table_aligns_on_display_width() {
        let rows = [
            workflow("短", "aa", RoleOfInterest::Any, None),
            workflow("wide name", "bb", RoleOfInterest::Generic, None),
        ];
        // Stripped: this is about layout, and `test_table_colors_are_the_only_
        // difference` already covers colour not disturbing it.
        let out = render_workflow_table(&rows);
        let out = out.ansi_strip();
        let header = out.lines().next().expect("no header line");
        let lines = body_lines(&out);

        assert!(lines[0].starts_with("|短"), "got {:?}", lines[0]);
        assert!(lines[1].starts_with("|wide name"), "got {:?}", lines[1]);

        // "短" is 2 columns wide and "wide name" is 9, so the name column is 9
        // wide and every later column starts at the same display column.
        assert_eq!(
            column_of(lines[0], "aa"),
            column_of(lines[1], "bb"),
            "KEY misaligned:\n{out}"
        );
        assert_eq!(
            column_of(lines[0], "Any"),
            column_of(lines[1], "Generic"),
            "ROLE misaligned:\n{out}"
        );
        assert_eq!(
            column_of(header, "KEY"),
            column_of(lines[0], "aa"),
            "header does not line up with rows:\n{out}"
        );
    }

    /// `Table::new` prepends `T::headers()`, and `Style::markdown()` adds a rule
    /// under it. Pin the shape: a `Tabled` impl that returned the wrong number
    /// of headers would show up here as a shifted or doubled header row.
    #[test]
    fn test_table_has_one_header_row_and_one_row_per_workflow() {
        let rows = [
            workflow("ProofRead", "R", RoleOfInterest::Any, None),
            workflow("Copy", "C", RoleOfInterest::Generic, None),
        ];
        let out = render_workflow_table(&rows);
        let out = out.ansi_strip();
        let lines: Vec<&str> = out.lines().collect();

        assert_eq!(
            lines.len(),
            rows.len() + 2,
            "expected a header, a rule, then one line per workflow, got:\n{out}"
        );
        assert!(
            lines[0].starts_with("|NAME"),
            "first line is not the header:\n{out}"
        );
        assert!(
            lines[1].starts_with("|---"),
            "second line is not the markdown rule:\n{out}"
        );
        assert!(
            lines[2].starts_with("|ProofRead"),
            "wrong first row:\n{out}"
        );
        assert!(lines[3].starts_with("|Copy"), "wrong second row:\n{out}");
        assert!(out.ends_with('\n'), "output should end with a newline");
    }

    #[test]
    fn test_table_header_row_is_coloured() {
        let rows = [workflow("ProofRead", "p", RoleOfInterest::Any, None)];
        let out = render_workflow_table(&rows);
        let header = out.lines().next().expect("no header line");

        for name in ["NAME", "KEY", "ROLE", "APPS", "STEPS"] {
            assert!(
                header.contains(&table_header_color().colorize(name)),
                "header cell {name:?} is not coloured:\n{out:?}"
            );
        }
    }

    /// ...and nothing else is. Pins the "no per-column palette" decision: a
    /// table where every column has its own hue is harder to scan than one
    /// where only the header stands out.
    #[test]
    fn test_table_body_is_not_coloured() {
        let rows = [workflow("ProofRead", "p", RoleOfInterest::Any, None)];
        let out = render_workflow_table(&rows);
        let body = body_lines(&out)[0];

        assert!(!body.ansi_has_any(), "a body cell is coloured:\n{out:?}");
    }

    /// `WorkFlow::display` is user-authored, so it can contain escape sequences.
    /// This is what the `ansi` feature of `tabled` buys: cells measured by visible
    /// width. Drop it and the second row shifts left by the length of the escapes.
    ///
    /// The lines are stripped *after* rendering, which is what makes this a
    /// measurement test: if `tabled` counted the escapes as width, the
    /// misalignment would already be baked into the spacing by now.
    #[test]
    fn test_table_measures_escapes_in_config_values_by_visible_width() {
        let rows = [
            workflow("Plain Name", "x", RoleOfInterest::Any, None),
            workflow(
                "\u{1b}[31mRed Name\u{1b}[0m",
                "y",
                RoleOfInterest::Any,
                None,
            ),
        ];
        let out = render_workflow_table(&rows);
        let stripped = out.ansi_strip();
        let body = body_lines(&stripped);
        let header = stripped.lines().next().expect("no header line");

        assert_eq!(
            column_of(body[1], "y"),
            column_of(body[0], "x"),
            "an escape in a workflow name shifted the columns:\n{out:?}"
        );
        assert_eq!(
            column_of(header, "KEY"),
            column_of(body[0], "x"),
            "header does not line up:\n{out:?}"
        );
    }

    /// Must not need a server.
    #[rstest]
    #[case::bash(CompletionShell::Bash)]
    #[case::zsh(CompletionShell::Zsh)]
    #[case::fish(CompletionShell::Fish)]
    #[case::elvish(CompletionShell::Elvish)]
    #[case::powershell(CompletionShell::PowerShell)]
    fn test_completions_generate_for_every_shell(#[case] shell: CompletionShell) {
        let script = completion_script(shell);

        assert!(
            !script.trim().is_empty(),
            "{shell:?} produced an empty script"
        );
        assert!(
            script.contains("glyphlow"),
            "{shell:?} script does not mention the command"
        );
    }

    /// `activate` maps each CLI kind onto a wire-level [`Target`]. `ocr` is not a
    /// plain name match — it becomes `ImageOCR`.
    #[rstest]
    #[case::clickable("clickable", Target::Clickable)]
    #[case::text("text", Target::Text)]
    #[case::image("image", Target::Image)]
    #[case::ocr("ocr", Target::ImageOCR)]
    #[case::input("input", Target::Editable)]
    #[case::editor("editor", Target::Edit)]
    #[case::child_element("child-element", Target::ChildElement)]
    #[case::scrollable("scrollable", Target::Scrollable)]
    fn activate_maps_kind_to_wire_target(#[case] arg: &str, #[case] expected: Target) {
        let cli = Cli::try_parse_from(["glyphlow-cli", "activate", arg])
            .unwrap_or_else(|e| panic!("`activate {arg}` should parse: {e}"));

        let Command::Activate { kind } = cli.command else {
            panic!("`activate {arg}` did not produce the activate subcommand");
        };
        assert_eq!(kind.to_target(), expected);
    }

    #[test]
    fn activate_rejects_an_unknown_kind() {
        assert!(Cli::try_parse_from(["glyphlow-cli", "activate", "hyperlink"]).is_err());
    }

    /// A workflow name only ever appears under `workflow run`. A bare
    /// `workflow <name>` must be a usage error (exit code 2) rather than a
    /// silent lookup.
    #[test]
    fn bare_workflow_name_is_a_usage_error() {
        let err = Cli::try_parse_from(["glyphlow-cli", "workflow", "ProofRead"])
            .expect_err("a bare workflow name must not parse");

        assert_eq!(err.exit_code(), 2, "clap usage errors exit with 2");
    }

    /// The name reaches the server untouched: matching is the server's job, so
    /// the CLI must not fold case or trim whitespace on the way through.
    #[test]
    fn workflow_run_keeps_the_name_verbatim() {
        let cli = Cli::try_parse_from(["glyphlow-cli", "workflow", "run", " ProofRead "])
            .expect("`workflow run <name>` should parse");

        let Command::Workflow {
            command: WorkflowCommand::Run { name },
        } = cli.command
        else {
            panic!("expected the `workflow run` subcommand");
        };
        assert_eq!(name, " ProofRead ");
    }

    #[test]
    fn workflow_list_parses() {
        let cli = Cli::try_parse_from(["glyphlow-cli", "workflow", "list"])
            .expect("`workflow list` should parse");

        assert!(matches!(
            cli.command,
            Command::Workflow {
                command: WorkflowCommand::List
            }
        ));
    }

    #[rstest]
    #[case::bash("bash")]
    #[case::zsh("zsh")]
    #[case::fish("fish")]
    #[case::elvish("elvish")]
    #[case::powershell("powershell")]
    fn complete_accepts_every_supported_shell(#[case] shell: &str) {
        assert!(
            Cli::try_parse_from(["glyphlow-cli", "complete", shell]).is_ok(),
            "`complete {shell}` should parse"
        );
    }

    #[test]
    fn complete_rejects_an_unknown_shell() {
        assert!(Cli::try_parse_from(["glyphlow-cli", "complete", "tcsh"]).is_err());
    }
}
