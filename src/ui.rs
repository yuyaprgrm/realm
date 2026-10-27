//! Terminal feedback for the sandbox preparation phase.
//!
//! Preparing a sandbox (Nix evaluation, closure realisation, bwrap setup) can
//! take a while. The spinner only tells the user that Realm is still working;
//! it never claims a progress percentage, because the preparation time does
//! not translate into one.
//!
//! The spinner draws on stderr, so the sandboxed process keeps a clean
//! stdout, and it is stopped before that process starts.

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const INTERVAL: Duration = Duration::from_millis(100);
const MESSAGE: &str = "Preparing sandbox...";
static STATUS: Mutex<String> = Mutex::new(String::new());
static PROGRESS: Mutex<Option<(usize, usize)>> = Mutex::new(None);

/// Serializes the terminal writes so the spinner and Realm's own messages
/// never interleave.
static TERMINAL: Mutex<()> = Mutex::new(());
/// Whether a spinner line is currently on the screen.
static ACTIVE: AtomicBool = AtomicBool::new(false);

fn terminal() -> MutexGuard<'static, ()> {
    TERMINAL.lock().unwrap_or_else(|error| error.into_inner())
}

/// A progress spinner for the sandbox preparation phase.
///
/// The spinner is inert when stderr is not a terminal: piped, redirected or
/// CI output must not receive control characters.
pub struct Spinner {
    stop: Option<Sender<()>>,
    handle: Option<JoinHandle<()>>,
}

impl Spinner {
    /// Start the spinner on stderr.
    pub fn start() -> Spinner {
        if !std::io::stderr().is_terminal() || std::env::var_os("NO_COLOR").is_some() {
            return Spinner {
                stop: None,
                handle: None,
            };
        }
        *STATUS.lock().unwrap_or_else(|e| e.into_inner()) = String::from(MESSAGE);
        *PROGRESS.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let (stop, receiver) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || spin(receiver));
        Spinner {
            stop: Some(stop),
            handle: Some(handle),
        }
    }

    /// Stop the spinner and erase its line.
    pub fn stop(&mut self) {
        // Dropping the sender disconnects the channel and wakes the thread.
        self.stop = None;
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Draw frames until the channel is disconnected, then erase the line.
fn spin(receiver: Receiver<()>) {
    let mut frame = 0;
    draw(FRAMES[frame]);
    frame += 1;
    loop {
        match receiver.recv_timeout(INTERVAL) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                draw(FRAMES[frame % FRAMES.len()]);
                frame += 1;
            }
        }
    }
    erase();
}

/// Write one spinner frame.
fn draw(frame: &str) {
    let _guard = terminal();
    let mut stderr = std::io::stderr().lock();
    let _ = write_frame(&mut stderr, frame);
    let _ = stderr.flush();
    ACTIVE.store(true, Ordering::Relaxed);
}

/// Erase the spinner line. Does nothing when no frame was ever drawn.
fn erase() {
    let _guard = terminal();
    if !ACTIVE.swap(false, Ordering::Relaxed) {
        return;
    }
    let mut stderr = std::io::stderr().lock();
    let _ = erase_line(&mut stderr);
    let _ = stderr.flush();
}

/// Print a message to stderr without leaving spinner residue.
///
/// The message replaces the spinner line; the spinner continues on the next
/// line.
pub fn notice(message: &str) {
    let _guard = terminal();
    let mut stderr = std::io::stderr().lock();
    if ACTIVE.load(Ordering::Relaxed) {
        let _ = erase_line(&mut stderr);
    }
    let _ = writeln!(stderr, "{message}");
    let _ = stderr.flush();
}

/// Update the spinner's status line without printing a new line.
pub fn status(message: &str) {
    *STATUS.lock().unwrap_or_else(|error| error.into_inner()) = message.to_string();
    *PROGRESS.lock().unwrap_or_else(|error| error.into_inner()) = None;
    if !std::io::stderr().is_terminal() || std::env::var_os("NO_COLOR").is_some() {
        let _guard = terminal();
        let mut stderr = std::io::stderr().lock();
        let _ = writeln!(stderr, "{message}");
        let _ = stderr.flush();
    }
}

/// Update the spinner's status line with a progress fraction.
pub fn progress(current: usize, total: usize, message: &str) {
    *STATUS.lock().unwrap_or_else(|error| error.into_inner()) = message.to_string();
    *PROGRESS.lock().unwrap_or_else(|error| error.into_inner()) = Some((current, total));
    if !std::io::stderr().is_terminal() || std::env::var_os("NO_COLOR").is_some() {
        let _guard = terminal();
        let mut stderr = std::io::stderr().lock();
        let _ = writeln!(stderr, "{message} ({current}/{total})");
        let _ = stderr.flush();
    }
}

/// The bytes of one spinner frame: the frame, the message, no newline.
fn write_frame(writer: &mut impl Write, frame: &str) -> std::io::Result<()> {
    let status = STATUS.lock().unwrap_or_else(|error| error.into_inner());
    let status = if status.is_empty() { MESSAGE } else { status.as_str() };
    let progress = *PROGRESS.lock().unwrap_or_else(|error| error.into_inner());
    if let Some((current, total)) = progress {
        write!(writer, "\r{frame} {status} ({current}/{total})")
    } else {
        write!(writer, "\r{frame} {status}")
    }
}


/// The bytes that erase the current line.
fn erase_line(writer: &mut impl Write) -> std::io::Result<()> {
    write!(writer, "\r\x1b[2K")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_one_line_that_is_erased_again() {
        let mut out = Vec::new();
        write_frame(&mut out, FRAMES[0]).unwrap();
        assert_eq!(
            String::from_utf8(out.clone()).unwrap(),
            "\r⠋ Preparing sandbox..."
        );
        erase_line(&mut out).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "\r⠋ Preparing sandbox...\r\x1b[2K"
        );
    }
}
