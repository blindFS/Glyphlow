//! Output tests for `glyphlow-cli`.
//!
//! These spawn the real binary, because the thing under test is the *stream*
//! the output goes to rather than a function in the crate. `workflow list`
//! always renders its table with colour, and `anstream` drops the escapes when
//! the destination cannot take them — so the decision is only observable from
//! outside the process.
//!
//! Every run here has stdout on a pipe, which is what `| grep` and `> file`
//! give. The terminal half of the matrix (a pty) needs `openpty`, which is not
//! worth a dependency; check it by hand if you touch the streams. On a terminal
//! the expected verdicts are: plain env → colour, `NO_COLOR=` (empty) → colour,
//! `CLICOLOR=0` → plain, `TERM=dumb` → plain, `CLICOLOR_FORCE=` (empty) → colour.

use std::{fs, path::PathBuf, process::Command};

/// The built client. Cargo puts its path in the environment for integration
/// tests.
const CLI: &str = env!("CARGO_BIN_EXE_glyphlow-cli");

/// Variables that steer colour detection. Cleared from every run so the
/// developer's own shell cannot change the result.
const COLOUR_VARS: [&str; 4] = ["NO_COLOR", "CLICOLOR", "CLICOLOR_FORCE", "TERM"];

/// A throwaway config home holding one workflow, so `workflow list` has a table
/// to print.
struct Fixture {
    config_home: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let config_home =
            std::env::temp_dir().join(format!("glyphlow-cli-output-{name}-{}", std::process::id()));
        let dir = config_home.join("glyphlow");
        fs::create_dir_all(&dir).expect("create config directory");
        fs::write(
            dir.join("config.toml"),
            "[[workflows]]\ndisplay = \"ProofRead\"\nkey = \"p\"\nactions = []\n",
        )
        .expect("write config file");

        Self { config_home }
    }

    /// Run the client with stdout on a pipe and return what it wrote.
    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> String {
        let mut command = Command::new(CLI);
        command
            .args(args)
            .env("XDG_CONFIG_HOME", &self.config_home)
            .env("TERM", "xterm-256color");
        for var in COLOUR_VARS {
            command.env_remove(var);
        }
        command.envs(env.iter().copied());

        let out = command.output().expect("failed to run glyphlow-cli");
        assert!(
            out.status.success(),
            "glyphlow-cli {args:?} exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).expect("stdout is utf-8")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.config_home);
    }
}

fn coloured(text: &str) -> bool {
    text.contains('\u{1b}')
}

/// A pipe must not be sprayed with escape codes.
#[test]
fn test_piped_output_has_no_escapes() {
    let fixture = Fixture::new("piped");
    let table = fixture.run(&["workflow", "list"], &[]);
    let help = fixture.run(&["--help"], &[]);

    assert!(table.contains("ProofRead"), "no table in: {table:?}");
    assert!(!coloured(&table), "escapes in the piped table: {table:?}");
    assert!(!coloured(&help), "escapes in piped --help: {help:?}");
}

/// `CLICOLOR_FORCE` is the escape hatch: it colours even a pipe. Without this
/// the tests above would also pass if colour were simply never emitted.
#[test]
fn test_clicolor_force_colours_a_pipe() {
    let fixture = Fixture::new("force");
    let table = fixture.run(&["workflow", "list"], &[("CLICOLOR_FORCE", "1")]);

    assert!(coloured(&table), "CLICOLOR_FORCE did not colour: {table:?}");
}

/// `NO_COLOR` outranks `CLICOLOR_FORCE`, as the spec requires.
#[test]
fn test_no_color_beats_clicolor_force() {
    let fixture = Fixture::new("precedence");
    let table = fixture.run(
        &["workflow", "list"],
        &[("CLICOLOR_FORCE", "1"), ("NO_COLOR", "1")],
    );

    assert!(!coloured(&table), "NO_COLOR was ignored: {table:?}");
}

/// Both variables are "set" only when non-empty, so an empty value must behave
/// exactly like an unset one. The hand-rolled detection this replaced got that
/// wrong in both directions.
#[test]
fn test_empty_values_count_as_unset() {
    let fixture = Fixture::new("empty");

    let empty_force = fixture.run(&["workflow", "list"], &[("CLICOLOR_FORCE", "")]);
    assert!(
        !coloured(&empty_force),
        "an empty CLICOLOR_FORCE forced colour: {empty_force:?}"
    );

    let empty_no_color = fixture.run(
        &["workflow", "list"],
        &[("NO_COLOR", ""), ("CLICOLOR_FORCE", "1")],
    );
    assert!(
        coloured(&empty_no_color),
        "an empty NO_COLOR suppressed colour: {empty_no_color:?}"
    );
}

/// The table and clap's own output must reach the same verdict. They used to
/// disagree, because the table ran its own detection while clap used
/// `anstream`; these are the cases where that was visible on a pipe.
#[test]
fn test_table_and_help_agree() {
    let fixture = Fixture::new("agree");

    for (case, env) in [
        ("plain env", vec![]),
        ("CLICOLOR_FORCE=1", vec![("CLICOLOR_FORCE", "1")]),
        // Non-empty, so it counts as set — `CLICOLOR_FORCE=0` still forces
        // colour. Surprising enough to be worth pinning.
        ("CLICOLOR_FORCE=0", vec![("CLICOLOR_FORCE", "0")]),
        ("CLICOLOR_FORCE=", vec![("CLICOLOR_FORCE", "")]),
        (
            "NO_COLOR=1 with CLICOLOR_FORCE=1",
            vec![("NO_COLOR", "1"), ("CLICOLOR_FORCE", "1")],
        ),
    ] {
        let table = coloured(&fixture.run(&["workflow", "list"], &env));
        let help = coloured(&fixture.run(&["--help"], &env));
        assert_eq!(table, help, "table and --help disagree under {case}");
    }
}
