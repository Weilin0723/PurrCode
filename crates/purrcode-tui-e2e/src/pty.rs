//! The pseudo-terminal the workbench runs inside.
//!
//! A reader thread drains the child's output continuously into both the ANSI
//! parser and a raw log. Draining continuously matters: a child that fills the
//! PTY buffer while nobody reads it blocks, which would look like a hung
//! interface rather than a stalled test.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, MasterPty, PtySize};

use crate::screen::Screen;

/// Shared terminal state, updated by the reader thread.
struct Terminal {
    parser: vt100::Parser,
    /// Every byte the child wrote, for the `ansi-stream.log` artifact.
    raw: Vec<u8>,
    /// Set when the child's output stream ends.
    closed: bool,
}

pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    terminal: Arc<Mutex<Terminal>>,
    size: (u16, u16),
}

impl PtySession {
    /// Spawn `program` with `arguments` in a PTY of the given size.
    pub fn spawn(
        program: &PathBuf,
        arguments: &[String],
        environment: &[(String, String)],
        working_directory: &PathBuf,
        columns: u16,
        rows: u16,
    ) -> Result<Self> {
        let system = portable_pty::native_pty_system();
        let pair = system
            .openpty(PtySize {
                rows,
                cols: columns,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("open pseudo-terminal")?;

        let mut command = CommandBuilder::new(program);
        for argument in arguments {
            command.arg(argument);
        }
        command.cwd(working_directory);
        // A deterministic terminal profile: without this the test inherits the
        // developer's TERM and COLORTERM and stops being reproducible.
        command.env("TERM", "xterm-256color");
        command.env("COLUMNS", columns.to_string());
        command.env("LINES", rows.to_string());
        for (key, value) in environment {
            command.env(key, value);
        }

        let child = pair.slave.spawn_command(command).context("spawn child")?;
        drop(pair.slave);

        let terminal = Arc::new(Mutex::new(Terminal {
            parser: vt100::Parser::new(rows, columns, 0),
            raw: Vec::new(),
            closed: false,
        }));
        let mut reader = pair.master.try_clone_reader().context("clone pty reader")?;
        let sink = Arc::clone(&terminal);
        std::thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => {
                        if let Ok(mut terminal) = sink.lock() {
                            terminal.closed = true;
                        }
                        return;
                    }
                    Ok(count) => {
                        if let Ok(mut terminal) = sink.lock() {
                            terminal.parser.process(&buffer[..count]);
                            terminal.raw.extend_from_slice(&buffer[..count]);
                        }
                    }
                }
            }
        });
        let writer = pair.master.take_writer().context("take pty writer")?;

        Ok(Self {
            master: pair.master,
            writer,
            child,
            terminal,
            size: (columns, rows),
        })
    }

    /// Current screen contents.
    pub fn screen(&self) -> Screen {
        let terminal = self.terminal.lock().expect("terminal mutex");
        Screen::from_parser(&terminal.parser)
    }

    /// Every byte the child has written.
    pub fn ansi_stream(&self) -> Vec<u8> {
        self.terminal.lock().expect("terminal mutex").raw.clone()
    }

    /// True once the child's output stream has ended.
    pub fn output_closed(&self) -> bool {
        self.terminal.lock().expect("terminal mutex").closed
    }

    pub fn size(&self) -> (u16, u16) {
        self.size
    }

    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer.write_all(bytes).context("write to pty")?;
        self.writer.flush().context("flush pty")
    }

    /// Resize the terminal, delivering SIGWINCH exactly as a window resize does.
    pub fn resize(&mut self, columns: u16, rows: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols: columns,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("resize pseudo-terminal")?;
        {
            let mut terminal = self.terminal.lock().expect("terminal mutex");
            terminal.parser.set_size(rows, columns);
        }
        self.size = (columns, rows);
        Ok(())
    }

    /// Whether the child has exited, without blocking.
    pub fn exited(&mut self) -> Result<Option<u32>> {
        Ok(self
            .child
            .try_wait()
            .context("poll child")?
            .map(|status| status.exit_code()))
    }

    /// Wait for the child to exit, killing it if it outlives the timeout.
    pub fn wait_for_exit(&mut self, timeout: Duration) -> Result<Option<u32>> {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if let Some(code) = self.exited()? {
                return Ok(Some(code));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        Ok(None)
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        // Never leave a workbench process behind: an orphan holds the PTY and
        // the fake daemon's port, which would break the next test.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spawn_shell(script: &str, columns: u16, rows: u16) -> PtySession {
        PtySession::spawn(
            &PathBuf::from("/bin/sh"),
            &["-c".to_owned(), script.to_owned()],
            &[],
            &PathBuf::from("/"),
            columns,
            rows,
        )
        .expect("spawn test child")
    }

    fn wait_for(session: &PtySession, needle: &str) -> Screen {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let screen = session.screen();
            if screen.contains(needle) {
                return screen;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "never saw {needle:?}; screen was:\n{}",
                screen.text()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn child_output_reaches_the_parsed_screen() {
        let session = spawn_shell("printf 'hello workbench\\n'; sleep 5", 40, 6);
        let screen = wait_for(&session, "hello workbench");
        assert_eq!(screen.row(0), "hello workbench");
    }

    #[test]
    fn input_written_to_the_pty_reaches_the_child() {
        let mut session = spawn_shell("read line; printf 'got:%s\\n' \"$line\"; sleep 5", 40, 6);
        session.write(b"typed\r").expect("write");
        let screen = wait_for(&session, "got:typed");
        assert!(screen.contains("got:typed"));
    }

    #[test]
    fn the_child_sees_the_requested_terminal_size() {
        let session = spawn_shell("printf 'cols=%s\\n' \"$COLUMNS\"; sleep 5", 73, 9);
        wait_for(&session, "cols=73");
        assert_eq!(session.size(), (73, 9));
    }

    #[test]
    fn the_raw_ansi_stream_is_captured_for_artifacts() {
        let session = spawn_shell("printf '\\033[1mbold\\033[0m\\n'; sleep 5", 40, 4);
        wait_for(&session, "bold");
        let stream = String::from_utf8_lossy(&session.ansi_stream()).to_string();
        assert!(
            stream.contains("\x1b[1m"),
            "the artifact must keep the escape codes, not only the text"
        );
    }

    #[test]
    fn resizing_updates_both_the_pty_and_the_screen_model() {
        let mut session = spawn_shell("sleep 5", 40, 6);
        session.resize(100, 30).expect("resize");
        assert_eq!(session.size(), (100, 30));
        let screen = session.screen();
        assert_eq!(screen.width(), 100);
        assert_eq!(screen.height(), 30);
    }

    #[test]
    fn a_finished_child_reports_its_exit_code_and_closes_output() {
        let mut session = spawn_shell("printf 'done\\n'; exit 3", 40, 4);
        let code = session
            .wait_for_exit(Duration::from_secs(10))
            .expect("wait");
        assert_eq!(code, Some(3));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !session.output_closed() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(session.output_closed());
    }

    #[test]
    fn a_hung_child_is_killed_rather_than_hanging_the_suite() {
        let mut session = spawn_shell("sleep 300", 40, 4);
        let code = session
            .wait_for_exit(Duration::from_millis(300))
            .expect("wait");
        assert_eq!(code, None, "a timeout must report no clean exit");
    }

    #[test]
    fn dropping_a_session_does_not_leave_the_child_running() {
        let mut session = spawn_shell("sleep 300", 40, 4);
        assert_eq!(session.exited().expect("poll"), None);
        drop(session);
        // The Drop impl kills the child; nothing to assert beyond not hanging.
    }
}
