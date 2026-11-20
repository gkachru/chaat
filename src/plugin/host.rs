//! Runs one external plugin process: starts it as `<plugin> serve`, sends
//! `hello`, forwards host messages, reads its lines (capped, versioned) and
//! restarts it with backoff if it exits. Knows nothing about what the plugin
//! is for; callers get `HostEvent`s.

use std::time::Duration;

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, watch};

use super::protocol::{HostMessage, MAX_LINE, PluginMessage};

/// What the host reports about its plugin.
#[derive(Debug)]
pub enum HostEvent {
    /// A version-1 message from the plugin.
    Message(PluginMessage),
    /// The plugin exited or could not start; it is restarted with backoff.
    /// `notify` is true for the first failure in a run of failures only.
    Stopped { error: String, notify: bool },
    /// The plugin's `ready` offered no capability Chaat knows: it is stopped
    /// and not restarted.
    Unsupported,
}

/// How to reach a running plugin host.
pub struct Plugin {
    stop: watch::Sender<bool>,
    /// The `hello` sent at each (re)start.
    hello: watch::Sender<HostMessage>,
    outgoing: mpsc::UnboundedSender<HostMessage>,
    task: tokio::task::JoinHandle<()>,
}

impl Plugin {
    /// Starts the plugin at `path`. Events go to `sink`, on the runtime. A
    /// `ready` offering none of `known` stops it as unsupported.
    pub fn start(
        path: std::path::PathBuf,
        hello: HostMessage,
        known: &'static [&'static str],
        sink: impl Fn(HostEvent) + Send + Sync + 'static,
    ) -> Self {
        let (stop, stopped) = watch::channel(false);
        let (hello, hello_rx) = watch::channel(hello);
        let (outgoing, outgoing_rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(run(
            path,
            known,
            Box::new(sink),
            stopped,
            hello_rx,
            outgoing_rx,
        ));
        Self {
            stop,
            hello,
            outgoing,
            task,
        }
    }

    /// Sends a message to the running plugin (or the next one, if it is
    /// restarting).
    pub fn send(&self, message: HostMessage) {
        let _ = self.outgoing.send(message);
    }

    /// A handle that sends messages to this plugin from another task.
    pub fn sender(&self) -> mpsc::UnboundedSender<HostMessage> {
        self.outgoing.clone()
    }

    /// Changes the `hello` used at the next restart.
    pub fn set_hello(&self, update: impl FnOnce(&mut HostMessage)) {
        self.hello.send_modify(update);
    }

    /// Asks the plugin to shut down and waits a little for it.
    pub async fn stop(self) {
        let _ = self.stop.send(true);
        let abort = self.task.abort_handle();
        if tokio::time::timeout(Duration::from_secs(3), self.task)
            .await
            .is_err()
        {
            // Stuck (say, a plugin that stopped reading its input): end the
            // task, which kills the process with it.
            log::info!("plugin: did not stop in time; killing it");
            abort.abort();
        }
    }

    /// Stops the plugin without waiting (it is being replaced).
    pub fn abort(self) {
        let _ = self.stop.send(true);
        self.task.abort();
    }
}

type Sink = Box<dyn Fn(HostEvent) + Send + Sync>;

async fn run(
    path: std::path::PathBuf,
    known: &'static [&'static str],
    sink: Sink,
    mut stop: watch::Receiver<bool>,
    hello: watch::Receiver<HostMessage>,
    mut outgoing: mpsc::UnboundedReceiver<HostMessage>,
) {
    let mut backoff = Duration::from_secs(1);
    // Only the first failure in a row is worth a notice; a plugin that is
    // missing or crashes at once would otherwise repeat it every restart.
    let mut failing = false;
    while !*stop.borrow() {
        let started = std::time::Instant::now();
        let hello = hello.borrow().clone();
        match run_once(&path, known, &sink, &mut stop, hello, &mut outgoing).await {
            Ok(Exit::Stopped) => return,
            Ok(Exit::Unsupported) => {
                sink(HostEvent::Unsupported);
                return;
            }
            Err(error) => {
                // A run that lasted a while was healthy: start over.
                if started.elapsed() > Duration::from_secs(60) {
                    backoff = Duration::from_secs(1);
                    failing = false;
                }
                sink(HostEvent::Stopped {
                    error: error.to_string(),
                    notify: !failing,
                });
                failing = true;
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    // An error means the `Plugin` handle is gone: stop too.
                    changed = stop.changed() => if changed.is_err() {
                        return;
                    },
                }
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
    }
}

enum Exit {
    Stopped,
    Unsupported,
}

/// One run of the plugin process, until it is stopped or exits.
async fn run_once(
    path: &std::path::Path,
    known: &[&str],
    sink: &Sink,
    stop: &mut watch::Receiver<bool>,
    hello: HostMessage,
    outgoing: &mut mpsc::UnboundedReceiver<HostMessage>,
) -> std::io::Result<Exit> {
    let mut command = tokio::process::Command::new(path);
    command
        .arg("serve")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    // Chaat is a windowed app; without this a console plugin opens a console.
    #[cfg(target_os = "windows")]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::other("plugin stdin unavailable"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("plugin stdout unavailable"))?;
    let _ = stdin.write_all(hello.to_line().as_bytes()).await;
    let _ = stdin.flush().await;

    let mut lines = LineReader::new(tokio::io::BufReader::new(stdout));
    loop {
        tokio::select! {
            // Messages first: a `forget` sent just before a stop still goes out.
            biased;
            Some(message) = outgoing.recv() => {
                let _ = stdin.write_all(message.to_line().as_bytes()).await;
                let _ = stdin.flush().await;
            }
            // An error means the `Plugin` handle is gone: stop too.
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    while let Ok(message) = outgoing.try_recv() {
                        let _ = stdin.write_all(message.to_line().as_bytes()).await;
                    }
                    let _ = stdin.write_all(HostMessage::Shutdown.to_line().as_bytes()).await;
                    let _ = stdin.flush().await;
                    let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
                    let _ = child.start_kill();
                    return Ok(Exit::Stopped);
                }
            }
            line = lines.next() => match line? {
                None => return Err(std::io::Error::other("plugin closed its output")),
                Some(Line::TooLong) => log::info!("plugin: dropped an over-long line"),
                Some(Line::Text(line)) => match PluginMessage::parse(&line) {
                    Some(PluginMessage::Ready { capabilities, .. })
                        if !capabilities.iter().any(|c| known.contains(&c.as_str())) =>
                    {
                        let _ = child.start_kill();
                        return Ok(Exit::Unsupported);
                    }
                    Some(message) => sink(HostEvent::Message(message)),
                    None => {}
                },
            },
        }
    }
}

enum Line {
    Text(String),
    TooLong,
}

/// Reads newline-ended lines of at most `MAX_LINE` bytes. Cancel-safe: a line
/// read halfway resumes on the next call.
struct LineReader<R> {
    inner: R,
    buffer: Vec<u8>,
    too_long: bool,
}

impl<R: AsyncBufRead + Unpin> LineReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            buffer: Vec::new(),
            too_long: false,
        }
    }

    /// The next line; `None` at the end of the output.
    async fn next(&mut self) -> std::io::Result<Option<Line>> {
        loop {
            let available = self.inner.fill_buf().await?;
            if available.is_empty() {
                return Ok(None);
            }
            let (chunk, ended) = match available.iter().position(|&b| b == b'\n') {
                Some(end) => (&available[..end], true),
                None => (available, false),
            };
            if self.buffer.len() + chunk.len() > MAX_LINE {
                self.too_long = true;
                self.buffer.clear();
            } else if !self.too_long {
                self.buffer.extend_from_slice(chunk);
            }
            let used = chunk.len() + usize::from(ended);
            self.inner.consume(used);
            if ended {
                let too_long = std::mem::take(&mut self.too_long);
                let bytes = std::mem::take(&mut self.buffer);
                return Ok(Some(if too_long {
                    Line::TooLong
                } else {
                    Line::Text(String::from_utf8_lossy(&bytes).into_owned())
                }));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lines_over_the_limit_are_dropped_and_the_next_one_reads() {
        let long = "x".repeat(MAX_LINE + 10);
        let input = format!("short\n{long}\nafter\nno newline at the end");
        let mut reader =
            LineReader::new(tokio::io::BufReader::with_capacity(1024, input.as_bytes()));
        assert!(matches!(reader.next().await.unwrap(), Some(Line::Text(t)) if t == "short"));
        assert!(matches!(reader.next().await.unwrap(), Some(Line::TooLong)));
        assert!(matches!(reader.next().await.unwrap(), Some(Line::Text(t)) if t == "after"));
        // A last line without its newline never completes.
        assert!(reader.next().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_line_exactly_at_the_limit_is_kept() {
        let exact = "y".repeat(MAX_LINE);
        let input = format!("{exact}\n");
        let mut reader = LineReader::new(tokio::io::BufReader::new(input.as_bytes()));
        assert!(matches!(reader.next().await.unwrap(), Some(Line::Text(t)) if t.len() == MAX_LINE));
    }
}
