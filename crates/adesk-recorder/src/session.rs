//! An asynchronous, thread-owned [`RecordingSession`].
//!
//! `adesk-server` runs on a tokio runtime; encoding a frame can be slow. A
//! [`RecordingSession`] owns a [`Recorder`] on a dedicated OS thread and accepts
//! frames over a channel, so the async caller never blocks on the encoder.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use adesk_core::ImageBuffer;

use crate::error::{RecorderError, Result};
use crate::recorder::{Recorder, RecorderConfig, RecordingSummary};

/// A command sent to the recording thread.
enum Command {
    Frame(ImageBuffer, u64),
    Stop,
}

/// How the recording thread finished.
enum Outcome {
    Done(RecordingSummary),
    Failed(RecorderError),
}

/// State shared between the session handle and the recording thread.
struct Shared {
    frames: AtomicU64,
    running: AtomicBool,
    encoder: String,
    outcome: Mutex<Option<Outcome>>,
}

/// A snapshot of a [`RecordingSession`]'s progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingStats {
    /// Frames accepted by the encoder so far.
    pub frames: u64,
    /// The resolved encoder backend name.
    pub encoder: String,
    /// Whether the recording thread is still running.
    pub running: bool,
}

/// A [`Recorder`] running on its own thread, driven over a channel.
///
/// Frames are handed to the thread with [`RecordingSession::push`]; the final
/// [`RecordingSession::stop`] joins the thread and returns the
/// [`RecordingSummary`]. Dropping the session without calling `stop` also
/// finalizes the recording (best-effort).
pub struct RecordingSession {
    cmd_tx: Sender<Command>,
    shared: Arc<Shared>,
    handle: Option<JoinHandle<()>>,
    summary: Option<RecordingSummary>,
}

impl RecordingSession {
    /// Creates the recorder (so an unwritable path fails here) and starts the
    /// recording thread.
    pub fn start(config: RecorderConfig) -> Result<RecordingSession> {
        let recorder = Recorder::create(config)?;
        let encoder = recorder.encoder_name().to_string();
        let shared = Arc::new(Shared {
            frames: AtomicU64::new(0),
            running: AtomicBool::new(true),
            encoder,
            outcome: Mutex::new(None),
        });
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let worker_shared = Arc::clone(&shared);
        let handle = thread::Builder::new()
            .name("adesk-recorder".into())
            .spawn(move || run(recorder, cmd_rx, worker_shared))
            .map_err(|err| {
                RecorderError::Backend(format!("failed to start recording thread: {err}"))
            })?;
        Ok(RecordingSession {
            cmd_tx,
            shared,
            handle: Some(handle),
            summary: None,
        })
    }

    /// Queues one RGBA8 frame (honouring `stride`) stamped with `ts_ms`.
    ///
    /// Returns as soon as the frame is queued; the encode happens on the
    /// recording thread. Fails once the session has stopped, or if the worker
    /// died — the precise failure is reported by [`RecordingSession::stop`].
    pub fn push(&self, frame: ImageBuffer, ts_ms: u64) -> Result<()> {
        match self.cmd_tx.send(Command::Frame(frame, ts_ms)) {
            Ok(()) => Ok(()),
            Err(_) => {
                let guard = self.shared.outcome.lock().expect("poisoned");
                match guard.as_ref() {
                    Some(Outcome::Failed(err)) => {
                        Err(RecorderError::Backend(format!("recording stopped: {err}")))
                    }
                    _ => Err(RecorderError::Backend(
                        "recording session is no longer running".into(),
                    )),
                }
            }
        }
    }

    /// A snapshot of the recording's progress.
    pub fn stats(&self) -> RecordingStats {
        RecordingStats {
            frames: self.shared.frames.load(Ordering::SeqCst),
            encoder: self.shared.encoder.clone(),
            running: self.shared.running.load(Ordering::SeqCst),
        }
    }

    /// The resolved encoder backend name.
    pub fn encoder_name(&self) -> &str {
        &self.shared.encoder
    }

    /// Finalizes the recording and returns its summary.
    ///
    /// Flushes all queued frames, joins the recording thread and finishes the
    /// output. Idempotent: a second call returns the same summary.
    pub fn stop(&mut self) -> Result<RecordingSummary> {
        if let Some(summary) = &self.summary {
            return Ok(summary.clone());
        }
        let _ = self.cmd_tx.send(Command::Stop);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        self.shared.running.store(false, Ordering::SeqCst);

        let outcome = self.shared.outcome.lock().expect("poisoned").take();
        match outcome {
            Some(Outcome::Done(summary)) => {
                self.summary = Some(summary.clone());
                Ok(summary)
            }
            Some(Outcome::Failed(err)) => Err(err),
            None => Err(RecorderError::Backend(
                "recording thread terminated unexpectedly".into(),
            )),
        }
    }
}

impl Drop for RecordingSession {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = self.cmd_tx.send(Command::Stop);
            let _ = handle.join();
        }
        self.shared.running.store(false, Ordering::SeqCst);
    }
}

impl std::fmt::Debug for RecordingSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordingSession")
            .field("encoder", &self.shared.encoder)
            .field("frames", &self.shared.frames.load(Ordering::Relaxed))
            .field("running", &self.shared.running.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

/// The recording thread's entry point; always records an [`Outcome`].
fn run(recorder: Recorder, cmd_rx: Receiver<Command>, shared: Arc<Shared>) {
    let outcome = match drive(recorder, cmd_rx, &shared) {
        Ok(summary) => Outcome::Done(summary),
        Err(err) => Outcome::Failed(err),
    };
    shared.running.store(false, Ordering::SeqCst);
    *shared.outcome.lock().expect("poisoned") = Some(outcome);
}

/// Drains commands into the recorder until a stop or a failure.
fn drive(
    mut recorder: Recorder,
    cmd_rx: Receiver<Command>,
    shared: &Shared,
) -> Result<RecordingSummary> {
    while let Ok(command) = cmd_rx.recv() {
        match command {
            Command::Frame(frame, ts_ms) => {
                recorder.push_frame(&frame, ts_ms)?;
                shared.frames.store(recorder.frames(), Ordering::SeqCst);
            }
            Command::Stop => break,
        }
    }
    let summary = recorder.finish()?;
    shared.frames.store(summary.frames, Ordering::SeqCst);
    Ok(summary)
}
