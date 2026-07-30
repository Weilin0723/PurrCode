//! The harness: a fake daemon, a real workbench in a real PTY, and waits that
//! are always tied to observable state.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::fake_daemon::{DaemonScript, FakeDaemon, RecordedRequest};
use crate::fixtures::{purrcode_binary, Workspace};
use crate::keys::{self, Key};
use crate::pty::PtySession;
use crate::screen::Screen;
use crate::DEFAULT_TIMEOUT;

#[derive(Clone, Debug)]
pub struct HarnessOptions {
    pub columns: u16,
    pub rows: u16,
    /// Extra environment for the workbench process, e.g. `NO_COLOR`.
    pub environment: Vec<(String, String)>,
}

impl Default for HarnessOptions {
    fn default() -> Self {
        Self {
            columns: 120,
            rows: 30,
            environment: Vec::new(),
        }
    }
}

impl HarnessOptions {
    pub fn size(columns: u16, rows: u16) -> Self {
        Self {
            columns,
            rows,
            ..Self::default()
        }
    }

    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.environment.push((key.to_owned(), value.to_owned()));
        self
    }
}

pub struct Harness {
    workspace: Workspace,
    daemon: FakeDaemon,
    session: PtySession,
    /// Keys sent so far, for the `keys.jsonl` artifact.
    sent: Vec<String>,
    /// The screen before the most recent key, for the `screen-before.txt`
    /// artifact.
    screen_before: Option<String>,
    options: HarnessOptions,
}

impl Harness {
    /// Start a fake daemon with `script`, then launch the workbench against it.
    pub fn start(script: DaemonScript) -> Result<Self> {
        Self::start_with(script, HarnessOptions::default())
    }

    pub fn start_with(script: DaemonScript, options: HarnessOptions) -> Result<Self> {
        let workspace = Workspace::new()?;
        Self::start_in(workspace, script, options)
    }

    /// Launch against an existing workspace, so a restart test can reuse the same
    /// repository and saved state.
    pub fn start_in(
        workspace: Workspace,
        script: DaemonScript,
        options: HarnessOptions,
    ) -> Result<Self> {
        let daemon = FakeDaemon::start(script, workspace.token())?;
        let session = Self::spawn_workbench(&workspace, &daemon, &options)?;
        Ok(Self {
            workspace,
            daemon,
            session,
            sent: Vec::new(),
            screen_before: None,
            options,
        })
    }

    fn spawn_workbench(
        workspace: &Workspace,
        daemon: &FakeDaemon,
        options: &HarnessOptions,
    ) -> Result<PtySession> {
        let binary = purrcode_binary()?;
        // `workbench` attaches to a daemon the caller manages; unlike the bare
        // invocation it never starts one, which is what lets the test point the
        // real binary at the fake daemon.
        let arguments = vec![
            "--daemon-url".to_owned(),
            daemon.url(),
            "--database".to_owned(),
            workspace.database_path().display().to_string(),
            "workbench".to_owned(),
            "--repository".to_owned(),
            workspace.repository().display().to_string(),
            "--token-file".to_owned(),
            workspace.token_file().display().to_string(),
        ];
        let mut environment = vec![
            (
                "PURRCODE_STATE_DIR".to_owned(),
                workspace.state_dir().display().to_string(),
            ),
            // Deterministic theme unless the test overrides it.
            ("PURRCODE_THEME".to_owned(), "dark".to_owned()),
            ("RUST_LOG".to_owned(), "warn".to_owned()),
            // reqwest respects HTTP(S)_PROXY / system proxy settings by
            // default, and the workbench's client sets no explicit
            // `.no_proxy()`. Some CI runner images configure a proxy at the
            // environment or OS level; without an explicit exclusion, every
            // request to this test's loopback fake daemon would be routed
            // through it and silently stall until the client's own timeout,
            // rather than actually connecting. Exclude the loopback
            // addresses explicitly so the daemon is always reached directly,
            // matching what a real deployment would want for localhost
            // anyway.
            ("NO_PROXY".to_owned(), "127.0.0.1,localhost".to_owned()),
            ("no_proxy".to_owned(), "127.0.0.1,localhost".to_owned()),
        ];
        environment.extend(options.environment.clone());
        PtySession::spawn(
            &binary,
            &arguments,
            &environment,
            &workspace.repository(),
            options.columns,
            options.rows,
        )
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub fn daemon(&self) -> &FakeDaemon {
        &self.daemon
    }

    pub fn screen(&self) -> Screen {
        self.session.screen()
    }

    pub fn size(&self) -> (u16, u16) {
        self.session.size()
    }

    /// Send keys, remembering the screen beforehand for failure artifacts.
    pub fn send(&mut self, keys: &[Key]) -> Result<()> {
        self.screen_before = Some(self.screen().text());
        for key in keys {
            self.sent.push(keys::describe(key));
        }
        self.session.write(&keys::encode(keys))
    }

    pub fn key(&mut self, key: Key) -> Result<()> {
        self.send(&[key])
    }

    pub fn type_text(&mut self, text: &str) -> Result<()> {
        self.send(&Key::typed(text))
    }

    /// Type a slash command and submit it.
    pub fn run_command(&mut self, command: &str) -> Result<()> {
        self.type_text(command)?;
        self.key(Key::Ctrl('g'))
    }

    pub fn resize(&mut self, columns: u16, rows: u16) -> Result<()> {
        self.options.columns = columns;
        self.options.rows = rows;
        self.session.resize(columns, rows)
    }

    /// Wait until `needle` is visible on one row.
    ///
    /// This is the only sanctioned way to wait for the interface: it either
    /// observes the state or fails with the screen it saw, so a test can never
    /// pass because a sleep happened to be long enough.
    pub fn wait_for_text(&mut self, needle: &str) -> Result<Screen> {
        self.wait_until(DEFAULT_TIMEOUT, &format!("text {needle:?}"), |screen| {
            screen.contains(needle)
        })
    }

    /// Wait for a sentence that may legitimately wrap across rows.
    pub fn wait_for_wrapped(&mut self, needle: &str) -> Result<Screen> {
        self.wait_until(
            DEFAULT_TIMEOUT,
            &format!("wrapped text {needle:?}"),
            |screen| screen.contains_wrapped(needle),
        )
    }

    /// Wait until every needle is visible.
    pub fn wait_for_all(&mut self, needles: &[&str]) -> Result<Screen> {
        self.wait_until(DEFAULT_TIMEOUT, &format!("all of {needles:?}"), |screen| {
            needles.iter().all(|needle| screen.contains(needle))
        })
    }

    /// Wait until every `present` needle is visible and every `absent` needle
    /// is not, all on the same screen.
    ///
    /// Use this instead of a `wait_for_text` followed by a separate
    /// `assert_absent` on that same snapshot: a terminal frame can arrive
    /// across multiple partial PTY reads, so a screen that already shows one
    /// expected string does not guarantee an unrelated region (e.g. the
    /// footer hint bar) has finished re-rendering to drop stale content from
    /// the previous frame. Checking both together keeps waiting until they
    /// hold simultaneously, on one consistent render.
    pub fn wait_for_all_and_absent(&mut self, present: &[&str], absent: &[&str]) -> Result<Screen> {
        self.wait_until(
            DEFAULT_TIMEOUT,
            &format!("all of {present:?} and none of {absent:?}"),
            |screen| {
                present.iter().all(|needle| screen.contains(needle))
                    && absent.iter().all(|needle| !screen.contains(needle))
            },
        )
    }

    /// Wait until `needle` is no longer visible.
    pub fn wait_until_absent(&mut self, needle: &str) -> Result<Screen> {
        self.wait_until(
            DEFAULT_TIMEOUT,
            &format!("absence of {needle:?}"),
            |screen| !screen.contains(needle),
        )
    }

    /// Wait until the fake daemon has served a matching request. Proves a key
    /// press reached the daemon rather than only changing the screen.
    pub fn wait_for_request(&self, method: &str, fragment: &str) -> Result<Vec<RecordedRequest>> {
        let deadline = Instant::now() + DEFAULT_TIMEOUT;
        while Instant::now() < deadline {
            if self.daemon.saw(method, fragment) {
                return Ok(self.daemon.requests());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        bail!(
            "never saw {method} request matching {fragment:?}.\nRequests:\n{}\nScreen:\n{}",
            self.daemon.request_log(),
            self.screen().text()
        )
    }

    /// Wait until durable daemon state satisfies `predicate`.
    pub fn wait_for_durable(
        &self,
        description: &str,
        mut predicate: impl FnMut(&DaemonScript) -> bool,
    ) -> Result<()> {
        let deadline = Instant::now() + DEFAULT_TIMEOUT;
        while Instant::now() < deadline {
            let mut satisfied = false;
            self.daemon.update(|script| satisfied = predicate(script));
            if satisfied {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        bail!(
            "durable state never satisfied {description}.\nRequests:\n{}\nScreen:\n{}",
            self.daemon.request_log(),
            self.screen().text()
        )
    }

    /// Durable events recorded for a session, for post-interaction inspection.
    pub fn session_events(&self, session_id: &str) -> Vec<Value> {
        let mut events = Vec::new();
        self.daemon.update(|script| {
            if let Some(session) = script.sessions.get(session_id) {
                events = session.events.clone();
            }
        });
        events
    }

    fn wait_until(
        &mut self,
        timeout: Duration,
        description: &str,
        mut predicate: impl FnMut(&Screen) -> bool,
    ) -> Result<Screen> {
        let deadline = Instant::now() + timeout;
        let mut last = self.screen();
        loop {
            if predicate(&last) {
                return Ok(last);
            }
            if Instant::now() >= deadline {
                // A blank screen after the full timeout is a completely
                // different failure than a screen that rendered something
                // else: distinguish "the child never started producing
                // output" (still running vs. exited, and with what code) from
                // "it rendered the wrong thing" so a failure on a platform we
                // cannot attach a debugger to is still diagnosable from the
                // CI log alone.
                let process_state = match self.session.exited() {
                    Ok(Some(code)) => format!("child exited with code {code}"),
                    Ok(None) => "child is still running".to_owned(),
                    Err(error) => format!("child exit status unknown: {error}"),
                };
                bail!(
                    "timed out waiting for {description} at {}x{}.\n{process_state}; output stream closed: {}\nScreen:\n{}\n\nRequests:\n{}",
                    self.options.columns,
                    self.options.rows,
                    self.session.output_closed(),
                    last.text(),
                    self.daemon.request_log()
                );
            }
            std::thread::sleep(Duration::from_millis(20));
            last = self.screen();
        }
    }

    /// Quit the workbench and wait for the process to exit.
    pub fn quit(&mut self) -> Result<Option<u32>> {
        // `q` on an empty composer is the registered quit action.
        let _ = self.key(Key::Char('q'));
        self.session.wait_for_exit(Duration::from_secs(10))
    }

    /// Restart the workbench against the same workspace and daemon script.
    ///
    /// The daemon keeps running, so this exercises exactly what a client crash or
    /// a user reopening the terminal does: durable state survives, client state
    /// does not.
    pub fn restart(mut self) -> Result<Self> {
        let _ = self.quit();
        let session = Self::spawn_workbench(&self.workspace, &self.daemon, &self.options)?;
        Ok(Self {
            workspace: self.workspace,
            daemon: self.daemon,
            session,
            sent: Vec::new(),
            screen_before: None,
            options: self.options,
        })
    }

    /// Write the failure artifacts the release process requires and return the
    /// directory they landed in.
    pub fn preserve_artifacts(&self, label: &str) -> Result<std::path::PathBuf> {
        let directory = self.workspace.artifacts_dir();
        std::fs::create_dir_all(&directory).context("create artifact directory")?;

        write(
            &directory.join("screen-before.txt"),
            self.screen_before
                .as_deref()
                .unwrap_or("(no earlier screen)"),
        )?;
        write(&directory.join("screen-after.txt"), &self.screen().text())?;
        std::fs::write(
            directory.join("ansi-stream.log"),
            self.session.ansi_stream(),
        )
        .context("write ansi stream")?;
        write(
            &directory.join("keys.jsonl"),
            &self
                .sent
                .iter()
                .map(|key| serde_json::json!({"key": key}).to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        )?;
        write(&directory.join("daemon.log"), &self.daemon.request_log())?;
        write(
            &directory.join("provider.log"),
            "the provider is scripted as durable events; see session-events.jsonl",
        )?;

        let mut events = Vec::new();
        self.daemon.update(|script| {
            for (id, session) in &script.sessions {
                for event in &session.events {
                    events.push(serde_json::json!({"session": id, "event": event}).to_string());
                }
            }
        });
        write(&directory.join("session-events.jsonl"), &events.join("\n"))?;
        write(
            &directory.join("test-environment.json"),
            &serde_json::to_string_pretty(&serde_json::json!({
                "label": label,
                "columns": self.options.columns,
                "rows": self.options.rows,
                "environment": self.options.environment,
                "daemon_url": self.daemon.url(),
                "repository": self.workspace.repository(),
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
                "purrcode_version": env!("CARGO_PKG_VERSION"),
            }))
            .context("serialize test environment")?,
        )?;
        Ok(directory)
    }
}

fn write(path: &std::path::Path, contents: &str) -> Result<()> {
    std::fs::write(path, contents).with_context(|| format!("write {}", path.display()))
}

/// Run `body`, preserving artifacts under `label` if it fails.
///
/// Every PTY test goes through this, which is what makes the artifact set the
/// release process asks for automatic rather than something a test must remember.
pub fn with_artifacts<T>(
    label: &str,
    harness: &mut Harness,
    body: impl FnOnce(&mut Harness) -> Result<T>,
) -> T {
    match body(harness) {
        Ok(value) => value,
        Err(error) => {
            let location = harness
                .preserve_artifacts(label)
                .and_then(|_| {
                    // Move the whole workspace out of the temporary directory so
                    // the artifacts outlive this process.
                    Ok(crate::fixtures::artifact_root()?.join(label))
                })
                .map(|path| path.display().to_string())
                .unwrap_or_else(|artifact_error| {
                    format!("(artifacts could not be written: {artifact_error})")
                });
            panic!("{label} failed: {error}\nArtifacts: {location}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_options_use_a_standard_wide_terminal() {
        let options = HarnessOptions::default();
        assert_eq!(options.columns, 120);
        assert_eq!(options.rows, 30);
        assert!(options.environment.is_empty());
    }

    #[test]
    fn sizes_and_environment_are_configurable() {
        let options = HarnessOptions::size(60, 24).env("NO_COLOR", "1");
        assert_eq!((options.columns, options.rows), (60, 24));
        assert_eq!(
            options.environment,
            vec![("NO_COLOR".to_owned(), "1".to_owned())]
        );
    }

    /// The artifact set the release process requires. Asserted on a harness whose
    /// child exited immediately, so it holds even for a workbench that failed to
    /// start — which is exactly when the artifacts matter most.
    ///
    /// Drives `/bin/sh` directly as a trivial, controlled child (not the real
    /// `purrcode` binary), which does not exist on Windows -- POSIX-only.
    #[test]
    #[cfg(unix)]
    fn a_failing_run_writes_every_required_artifact() {
        let workspace = Workspace::new().expect("workspace");
        let daemon = FakeDaemon::start(DaemonScript::default(), workspace.token()).expect("daemon");
        let session = PtySession::spawn(
            &std::path::PathBuf::from("/bin/sh"),
            &["-c".to_owned(), "printf 'started\\n'".to_owned()],
            &[],
            &workspace.repository(),
            80,
            24,
        )
        .expect("spawn");
        let harness = Harness {
            workspace,
            daemon,
            session,
            sent: vec!["Ctrl+G".to_owned()],
            screen_before: Some("before".to_owned()),
            options: HarnessOptions::size(80, 24),
        };
        let directory = harness
            .preserve_artifacts("harness-artifact-test")
            .expect("write artifacts");
        for name in [
            "screen-before.txt",
            "screen-after.txt",
            "ansi-stream.log",
            "keys.jsonl",
            "daemon.log",
            "provider.log",
            "session-events.jsonl",
            "test-environment.json",
        ] {
            assert!(
                directory.join(name).is_file(),
                "missing required artifact {name}"
            );
        }
        let environment: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(directory.join("test-environment.json"))
                .expect("read environment"),
        )
        .expect("parse environment");
        assert_eq!(environment["columns"], 80);
        assert_eq!(environment["label"], "harness-artifact-test");
        assert!(environment["os"].is_string());
        assert_eq!(
            std::fs::read_to_string(directory.join("keys.jsonl")).expect("read keys"),
            "{\"key\":\"Ctrl+G\"}"
        );
    }

    /// Drives `/bin/sh` directly as a trivial, controlled child (not the real
    /// `purrcode` binary), which does not exist on Windows -- POSIX-only.
    #[test]
    #[cfg(unix)]
    fn a_wait_that_never_succeeds_reports_the_screen_it_saw() {
        let workspace = Workspace::new().expect("workspace");
        let daemon = FakeDaemon::start(DaemonScript::default(), workspace.token()).expect("daemon");
        let session = PtySession::spawn(
            &std::path::PathBuf::from("/bin/sh"),
            &[
                "-c".to_owned(),
                "printf 'unexpected output\\n'; sleep 5".to_owned(),
            ],
            &[],
            &workspace.repository(),
            60,
            10,
        )
        .expect("spawn");
        let mut harness = Harness {
            workspace,
            daemon,
            session,
            sent: Vec::new(),
            screen_before: None,
            options: HarnessOptions::size(60, 10),
        };
        let error = harness
            .wait_until(
                Duration::from_millis(200),
                "text \"never appears\"",
                |screen| screen.contains("never appears"),
            )
            .expect_err("the wait must fail");
        let message = error.to_string();
        assert!(message.contains("timed out waiting for"));
        assert!(
            message.contains("unexpected output"),
            "the failure must include the screen the test actually saw: {message}"
        );
        assert!(message.contains("60x10"), "the size must be reported");
    }
}
