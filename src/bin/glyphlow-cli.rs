//! Command line client for the Glyphlow server.
//!
//! Parses its arguments with `clap`, translates them into an [`AppSignal`] and
//! hands that to the running `glyphlow` server over its Unix socket.
//!
//! `workflow list` and `complete` are the exceptions: they never talk to the
//! server. The first reads the config from disk, the second just prints a shell
//! completion script derived from the command definition.

use std::{
    io::{IsTerminal, Write},
    process::ExitCode,
};

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
    settings::{
        Color, Modify, Padding, Style as TableStyle,
        object::{Columns, Rows},
    },
};

// ---------------------------------------------------------------------------
// Styling
// ---------------------------------------------------------------------------

/// Palette shared by clap's own output and the `workflow list` table, so the
/// two never drift apart.
const HEADER: Style = AnsiColor::Cyan.on_default().effects(Effects::BOLD);
const LITERAL: Style = AnsiColor::Yellow.on_default().effects(Effects::BOLD);
const DIM: Style = AnsiColor::BrightBlack.on_default();
const ERROR: Style = AnsiColor::Red.on_default().effects(Effects::BOLD);
const VALID: Style = AnsiColor::Green.on_default().effects(Effects::BOLD);
const INVALID: Style = AnsiColor::Yellow.on_default().effects(Effects::BOLD);

// Colours only the table uses. clap's own output has nothing to say about a role
// or an app list, so these are not part of `STYLES`.
//
// One hue per column, and none of them repeats `HEADER` — an earlier version
// coloured the name column with `HEADER` itself, which made the header row and a
// whole column read as the same cyan and left the table looking two-toned.
const NAME: Style = AnsiColor::Green.on_default().effects(Effects::BOLD);
const ROLE: Style = AnsiColor::Magenta.on_default();
/// `BrightBlue`, not `Blue`: plain blue is close to unreadable on a dark
/// terminal, and this column is mostly bundle ids.
const APPS: Style = AnsiColor::BrightBlue.on_default();

const STYLES: Styles = Styles::styled()
    .header(HEADER)
    .usage(HEADER)
    .literal(LITERAL)
    .placeholder(DIM)
    .error(ERROR)
    .valid(VALID)
    .invalid(INVALID);

/// Per-column styling for the `workflow list` table, in column order. All five
/// differ, and the step count is deliberately the quiet one.
const COLUMN_STYLES: [Style; 5] = [NAME, LITERAL, ROLE, APPS, DIM];

/// Bridge a clap style into a `tabled` colour.
///
/// Both types are foreign, so this cannot be a `From` impl.
fn color_of(style: Style) -> Color {
    Color::new(style.render().to_string(), style.render_reset().to_string())
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

/// Write to stdout without panicking on a closed pipe.
///
/// Rust ignores `SIGPIPE`, so a plain `print!` turns `glyphlow-cli complete
/// bash | head` into an EPIPE panic. Producing less output than the reader
/// wanted is a normal way for a pipeline to end.
fn write_stdout(bytes: &[u8]) {
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(bytes);
    let _ = stdout.flush();
}

fn write_stderr(bytes: &[u8]) {
    let mut stderr = std::io::stderr().lock();
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
///
/// `bash` needs a workaround. `clap_complete` escapes a hyphen in the binary
/// name two different ways — the dispatch table replaces it with `__`, the
/// option blocks with `__subcmd__` — so for a name like `glyphlow-cli` the two
/// never agree and *every* subcommand arm becomes unreachable:
/// `glyphlow-cli workflow <TAB>` completes nothing at all. zsh and fish are
/// unaffected.
///
/// Generating under a hyphen-free name sidesteps the clash. That is enough
/// because the dispatch loop matches the runtime `$1` rather than a baked-in
/// literal, so the script keeps working once registered for the real name.
///
/// Drop this when clap-rs/clap#6428 ships. See #5255 and #6421.
fn completion_script(shell: CompletionShell) -> String {
    let mut command = Cli::command();
    let bin_name = command.get_name().to_owned();
    // Generated into a buffer first so the write can be error-tolerant; clap's
    // own writer panics on a broken pipe.
    let mut buf: Vec<u8> = Vec::new();

    if matches!(shell, CompletionShell::Bash) && bin_name.contains('-') {
        let safe = bin_name.replace('-', "_");
        generate(shell, &mut command, &safe, &mut buf);
        let script = String::from_utf8_lossy(&buf);
        return retarget_bash_registration(&script, &safe, &bin_name);
    }

    generate(shell, &mut command, bin_name, &mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

/// Point `clap_complete`'s own `complete … <safe>` registrations at the real
/// binary name.
///
/// Only the trailing name is rewritten rather than the whole line re-emitted,
/// because the shape of the registration is clap's business — it emits a
/// version-conditional pair, with and without `-o nosort`.
fn retarget_bash_registration(script: &str, safe: &str, bin_name: &str) -> String {
    let needle = format!(" {safe}");
    let mut out = String::with_capacity(script.len());

    for (i, line) in script.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        match line.strip_suffix(&needle) {
            Some(head) => {
                out.push_str(head);
                out.push(' ');
                out.push_str(bin_name);
            }
            None => out.push_str(line),
        }
    }

    if script.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn list_workflows() -> ExitCode {
    let path = match get_config_path() {
        Ok(path) => path,
        Err(e) => {
            write_stderr(format!("glyphlow-cli: {e}\n").as_bytes());
            return ExitCode::FAILURE;
        }
    };

    // Deliberately not `load_config`: listing must not seed a config file.
    let workflows = match GlyphlowConfig::load_config_readonly(&path) {
        Ok(config) => config.workflows,
        Err(e) => {
            write_stderr(format!("glyphlow-cli: {e}\n").as_bytes());
            return ExitCode::FAILURE;
        }
    };

    if workflows.is_empty() {
        write_stdout(format!("No workflows configured in {}\n", path.display()).as_bytes());
        return ExitCode::SUCCESS;
    }

    let table = render_workflow_table(&workflows, colors_enabled());
    write_stdout(table.as_bytes());
    ExitCode::SUCCESS
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Should we emit ANSI escapes? Honours `NO_COLOR` and `CLICOLOR_FORCE`, and
/// otherwise only colours a terminal.
fn colors_enabled() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if std::env::var_os("CLICOLOR_FORCE").is_some_and(|value| value != "0") {
        return true;
    }
    std::io::stdout().is_terminal()
}

/// Render the `workflow list` table. `color` is a parameter rather than a
/// detection so the output can be asserted in tests.
///
/// The rows are the [`WorkFlow`] values themselves: `Table::new` takes the
/// headers and the cells from the `Tabled` derive on the struct, so there is
/// nothing to map by hand here.
///
/// Layout and colour are `tabled`'s job too. It measures cells by display width
/// and, with the `ansi` feature on, by *visible* width — which is what keeps
/// the columns lined up when a workflow name itself carries an escape sequence.
/// The palette is handed over as colours rather than painted into the text, so
/// the escapes are emitted after measuring and cannot affect it at all.
///
/// `Style::empty()` keeps it borderless (no rule characters, no leading indent)
/// and `Padding` restores the two-space gutters this command has always
/// printed.
fn render_workflow_table(workflows: &[WorkFlow], color: bool) -> String {
    let mut table = Table::new(workflows);
    table.with(TableStyle::empty());
    table.with(Padding::new(0, 2, 0, 0));

    if color {
        // Columns first, header row second — the order matters. `tabled`
        // resolves a cell's colour as cells > columns > rows, and setting a row
        // also back-fills a cell entry for every column already registered. So
        // doing the row last is what gives the header row one uniform style
        // instead of letting the column colours show through it.
        for (index, style) in COLUMN_STYLES.into_iter().enumerate() {
            table.with(Modify::new(Columns::one(index)).with(color_of(style)));
        }
        table.with(Modify::new(Rows::first()).with(color_of(HEADER)));
    }

    // `Display` emits no trailing newline; the command always ended with one.
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
    // Only the tests measure widths directly now; the table itself is tabled's
    // job.
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

    /// A cell as the table paints it: `Color::colorize` wraps text in exactly
    /// the prefix/suffix pair `tabled` emits, so this is what to look for in the
    /// output. Preferred over scanning for escape sequences — it asserts the
    /// cell's text *and* its colour together.
    fn painted(style: Style, text: &str) -> String {
        color_of(style).colorize(text)
    }

    #[test]
    fn test_table_has_no_ansi_when_color_is_off() {
        let rows = [workflow("ProofRead", "p", RoleOfInterest::Any, None)];
        let out = render_workflow_table(&rows, false);
        assert!(!out.ansi_has_any(), "unexpected escape in: {out:?}");
    }

    /// Colouring must not disturb the text or the padding.
    #[test]
    fn test_table_colors_are_the_only_difference() {
        let rows = [
            workflow("ProofRead", "p", RoleOfInterest::Any, None),
            workflow("短", "q", RoleOfInterest::Generic, Some(&["Notes"])),
        ];
        let plain = render_workflow_table(&rows, false);
        let colored = render_workflow_table(&rows, true);

        assert!(colored.ansi_has_any(), "no escapes in: {colored:?}");
        assert_eq!(colored.ansi_strip(), plain.as_str());
    }

    /// Column alignment must follow *display* width, not bytes, or a CJK or
    /// nerd-font name would shift every later column.
    #[test]
    fn test_table_aligns_on_display_width() {
        let rows = [
            workflow("短", "aa", RoleOfInterest::Any, None),
            workflow("wide name", "bb", RoleOfInterest::Generic, None),
        ];
        let out = render_workflow_table(&rows, false);
        let lines: Vec<&str> = out.lines().collect();

        assert!(lines[1].starts_with("短"), "got {:?}", lines[1]);
        assert!(lines[2].starts_with("wide name"), "got {:?}", lines[2]);

        // "短" is 2 columns wide and "wide name" is 9, so the name column is 9
        // wide and every later column starts at the same display column.
        assert_eq!(
            column_of(lines[1], "aa"),
            column_of(lines[2], "bb"),
            "KEY misaligned:\n{out}"
        );
        assert_eq!(
            column_of(lines[1], "Any"),
            column_of(lines[2], "Generic"),
            "ROLE misaligned:\n{out}"
        );
        assert_eq!(
            column_of(lines[0], "KEY"),
            column_of(lines[1], "aa"),
            "header does not line up with rows:\n{out}"
        );
    }

    /// `Table::new` prepends `T::headers()`. That is what puts the column names
    /// on the first line, and it also means a `Tabled` impl that returned the
    /// wrong number of headers would show up as a shifted or doubled header
    /// row. Pin the shape.
    #[test]
    fn test_table_has_exactly_one_header_row() {
        let rows = [
            workflow("ProofRead", "R", RoleOfInterest::Any, None),
            workflow("Copy", "C", RoleOfInterest::Generic, None),
        ];
        let out = render_workflow_table(&rows, false);
        let lines: Vec<&str> = out.lines().collect();

        assert_eq!(
            lines.len(),
            rows.len() + 1,
            "expected a header plus one line per workflow, got:\n{out}"
        );
        assert!(
            lines[0].starts_with("NAME"),
            "first line is not the header:\n{out}"
        );
        assert!(out.ends_with('\n'), "output should end with a newline");
    }

    /// The header row has to stay one uniform style rather than picking up the
    /// per-column palette.
    ///
    /// `tabled` resolves a cell's colour as cells > columns > rows, so the
    /// column colours applied first would otherwise win on row 0. It works out
    /// only because setting a row back-fills a cell entry for every column
    /// already registered — which is why the header row has to be applied
    /// *after* the columns. Easy to lose in a refactor; pin it.
    #[test]
    fn test_table_header_row_is_uniformly_coloured() {
        let rows = [workflow("ProofRead", "p", RoleOfInterest::Any, None)];
        let out = render_workflow_table(&rows, true);
        let header = out.lines().next().expect("no header line");

        for name in ["NAME", "KEY", "ROLE", "APPS", "STEPS"] {
            assert!(
                header.contains(&painted(HEADER, name)),
                "header cell {name:?} is not in the header colour:\n{out:?}"
            );
        }
    }

    /// ...and the body must still get the per-column palette, in column order.
    #[test]
    fn test_table_body_uses_the_column_palette() {
        let rows = [workflow("ProofRead", "p", RoleOfInterest::Any, None)];
        let out = render_workflow_table(&rows, true);
        let body = out.lines().nth(1).expect("no body line");

        // The cells `WorkFlow`'s derive produces for these rows.
        let cells = ["ProofRead", "p", "Any", "any", "0"];
        for (style, cell) in COLUMN_STYLES.into_iter().zip(cells) {
            assert!(
                body.contains(&painted(style, cell)),
                "body cell {cell:?} is not in its column colour:\n{out:?}"
            );
        }
    }

    /// No two columns may look alike, and the header row must not reuse a
    /// column's colour.
    ///
    /// The first version coloured the name column with `HEADER`, so cyan covered
    /// both the header row and a whole column and the table read as two-tone.
    /// This pins the invariant rather than the individual hues, so the palette
    /// can still be re-tuned freely.
    #[test]
    fn test_table_colours_are_all_distinct() {
        let mut seen = vec![color_of(HEADER)];

        for style in COLUMN_STYLES {
            let color = color_of(style);
            assert!(
                !seen.contains(&color),
                "a column reuses a colour that is already in the table: {color:?}"
            );
            seen.push(color);
        }
    }

    /// An escape sequence inside a config value must not shift the columns.
    ///
    /// `WorkFlow::display` is user-authored, so it can contain anything. This is
    /// what the `ansi` feature of `tabled` buys: cells are measured by visible
    /// width. Drop the feature and the second row here shifts left by the length
    /// of the escapes.
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
        let out = render_workflow_table(&rows, false);
        let lines: Vec<&str> = out.lines().collect();

        assert_eq!(
            column_of(&lines[2].ansi_strip(), "y"),
            column_of(lines[1], "x"),
            "an escape in a workflow name shifted the columns:\n{out:?}"
        );
        assert_eq!(
            column_of(lines[0], "KEY"),
            column_of(lines[1], "x"),
            "header does not line up:\n{out:?}"
        );
    }

    /// `complete` must produce something for every supported shell, and must
    /// not need a server.
    #[test]
    fn test_completions_generate_for_every_shell() {
        for shell in [
            CompletionShell::Bash,
            CompletionShell::Zsh,
            CompletionShell::Fish,
            CompletionShell::Elvish,
            CompletionShell::PowerShell,
        ] {
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
    }

    /// Regression guard for the hyphen workaround.
    ///
    /// `clap_complete` escapes a hyphen in the binary name as `__` in the
    /// dispatch table but as `__subcmd__` in the option blocks, so for
    /// `glyphlow-cli` every `cmd="…"` assignment misses its `…)` case label and
    /// bash subcommand completion silently does nothing. Assert the two agree.
    #[test]
    fn test_bash_completion_dispatch_is_self_consistent() {
        let script = completion_script(CompletionShell::Bash);

        let assigned: Vec<&str> = script
            .lines()
            .filter_map(|l| l.trim().strip_prefix("cmd=\"")?.strip_suffix('"'))
            .filter(|id| !id.is_empty())
            .collect();
        let labels: Vec<&str> = script
            .lines()
            .filter_map(|l| l.trim().strip_suffix(')'))
            .filter(|id| !id.is_empty() && id.chars().all(|c| c.is_alphanumeric() || c == '_'))
            .collect();

        assert!(!assigned.is_empty(), "no dispatch assignments found");
        assert!(!labels.is_empty(), "no case labels found");
        for id in &assigned {
            assert!(
                labels.contains(id),
                "dispatch assignment {id:?} has no matching case label, so the \
                 bash subcommand arm is unreachable:\n{script}"
            );
        }
    }

    /// The script has to be registered for the name the user actually types,
    /// not for the hyphen-free placeholder it was generated under.
    #[test]
    fn test_bash_completion_registers_the_real_binary_name() {
        let script = completion_script(CompletionShell::Bash);
        let registered: Vec<&str> = script
            .lines()
            .filter(|l| l.trim_start().starts_with("complete "))
            .collect();

        assert!(!registered.is_empty(), "no registration line in:\n{script}");
        for line in &registered {
            assert!(
                line.ends_with("glyphlow-cli"),
                "registration is not for `glyphlow-cli`: {line:?}"
            );
        }
    }

    #[test]
    fn test_retarget_bash_registration() {
        let script =
            "f() {\n    complete -F _a_b -o nosort a_b\nelse\n    complete -F _a_b a_b\nfi\n";
        let out = retarget_bash_registration(script, "a_b", "a-b");

        assert!(out.contains("complete -F _a_b -o nosort a-b"), "{out}");
        assert!(out.contains("complete -F _a_b a-b"), "{out}");
        // The function name must survive: only the trailing name is retargeted.
        assert!(out.contains("_a_b"), "function name was clobbered:\n{out}");
        assert!(out.ends_with('\n'), "trailing newline lost");
        assert_eq!(out.lines().count(), script.lines().count());
    }

    #[test]
    fn test_command_tree_is_well_formed() {
        // clap's own validation: duplicate args, bad defaults, etc.
        Cli::command().debug_assert();
    }
}
