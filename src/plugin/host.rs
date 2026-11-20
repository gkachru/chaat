//! Runs one external plugin process: starts it as `<plugin> serve`, sends
//! `hello`, forwards host messages, reads its lines (capped, versioned,
//! rate-limited) and restarts it with backoff if it exits. Knows nothing about
//! what the plugin is for; callers get `HostEvent`s.
//!
//! What the host guarantees about the process it starts:
//! - the file is hashed before every start and must match the pin (or, with
//!   no pin yet, is reported once so the caller can pin it). The hash and the
//!   start are two steps, so a file swapped in between runs; accepted;
//! - it runs in its own folder, with no `CHAAT_*` variables;
//! - everything it starts dies with it: a Job Object on Windows, a process
//!   group on Unix, ended when the run ends;
//! - it never stalls the host: writes go through their own task and queue and
//!   a write stuck for `WRITE_TIMEOUT` ends the run; reading is never behind
//!   writing; lines are rate-limited.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::{Mutex, mpsc, oneshot, watch};

use super::protocol::{HostMessage, MAX_LINE, PluginMessage};

/// What the host reports about its plugin.
#[derive(Debug)]
pub enum HostEvent {
    /// A version-1 message from the plugin.
    Message(PluginMessage),
    /// The plugin exited or could not start. It is restarted with backoff,
    /// except when it cannot start at all (missing, not allowed, not a
    /// program, a relative path): then the host ends. `notify` is true for
    /// the first failure in a run of failures only, and at most once every
    /// `NOTICE_EVERY` per host.
    Stopped { error: String, notify: bool },
    /// The plugin's `ready` offered no capability Chaat knows: it is stopped
    /// and not restarted.
    Unsupported,
    /// There was no pin: this is the SHA-256 (lowercase hex) of the file about
    /// to run. Sent once per host; later restarts must match it.
    Fingerprint { sha256: String },
    /// The file does not match its pin. It was not run and is not retried
    /// (the host ends). `sha256` is the hash of the file as it is now.
    FileChanged { sha256: String },
    /// The plugin sent lines faster than `LINES_PER_SECOND` (bursts of
    /// `LINE_BURST`); the excess is dropped. Sent once per run.
    Flooding,
}

/// Lines a plugin may send per second, on average.
const LINES_PER_SECOND: f64 = 20.0;
/// Lines a plugin may send at once before the rate applies.
const LINE_BURST: f64 = 50.0;
/// Messages waiting for a plugin's stdin; more are dropped.
const QUEUE: usize = 256;
/// A write to the plugin that takes this long ends the run.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// Crash notices per host are at most this far apart.
const NOTICE_EVERY: Duration = Duration::from_secs(10 * 60);

/// A handle that queues messages for a plugin from another task.
#[derive(Clone)]
pub struct PluginSender(mpsc::Sender<HostMessage>);

impl PluginSender {
    /// Queues a message without waiting. False if it was dropped: the queue
    /// is full (the plugin is not reading) or the host has ended.
    pub fn send(&self, message: HostMessage) -> bool {
        match self.0.try_send(message) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                log::info!("plugin: its input queue is full; dropped a message");
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }
}

type Queue = Arc<Mutex<mpsc::Receiver<HostMessage>>>;

/// How to reach a running plugin host.
pub struct Plugin {
    stop: watch::Sender<bool>,
    /// The `hello` sent at each (re)start.
    hello: watch::Sender<HostMessage>,
    outgoing: PluginSender,
    task: tokio::task::JoinHandle<()>,
}

impl Plugin {
    /// Starts the plugin at `path` (an absolute path; a relative one ends the
    /// host with a notice). Events go to `sink`, on the runtime. A `ready`
    /// offering none of `known` stops it as unsupported. `expected_sha256` is
    /// the file's pin, if it has one (see `HostEvent::Fingerprint` and
    /// `FileChanged`).
    pub fn start(
        path: std::path::PathBuf,
        hello: HostMessage,
        known: &'static [&'static str],
        expected_sha256: Option<String>,
        sink: impl Fn(HostEvent) + Send + Sync + 'static,
    ) -> Self {
        let (stop, stopped) = watch::channel(false);
        let (hello, hello_rx) = watch::channel(hello);
        let (outgoing, queue) = mpsc::channel(QUEUE);
        let task = tokio::spawn(run(
            path,
            known,
            expected_sha256,
            Box::new(sink),
            stopped,
            hello_rx,
            Arc::new(Mutex::new(queue)),
        ));
        Self {
            stop,
            hello,
            outgoing: PluginSender(outgoing),
            task,
        }
    }

    /// Queues a message for the running plugin. It is dropped, with a log
    /// line, if the queue is full; it is also dropped if the plugin is not
    /// running when the next run starts (each run starts with an empty queue,
    /// and its `hello` carries the current state).
    pub fn send(&self, message: HostMessage) {
        self.outgoing.send(message);
    }

    /// A handle that sends messages to this plugin from another task.
    pub fn sender(&self) -> PluginSender {
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
            // task, which kills the process tree with it.
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
    mut pin: Option<String>,
    sink: Sink,
    mut stop: watch::Receiver<bool>,
    hello: watch::Receiver<HostMessage>,
    queue: Queue,
) {
    // A program found by a relative path would depend on Chaat's own folder.
    if !path.is_absolute() {
        sink(HostEvent::Stopped {
            error: "the plugin's path is not absolute".into(),
            notify: true,
        });
        return;
    }
    let mut backoff = Duration::from_secs(1);
    // Only the first failure in a row is worth a notice; a plugin that is
    // missing or crashes at once would otherwise repeat it every restart. And
    // a plugin that runs a while and then crashes, again and again, is held
    // to one notice per `NOTICE_EVERY`.
    let mut failing = false;
    let mut last_notice: Option<Instant> = None;
    while !*stop.borrow() {
        let started = Instant::now();
        match attempt(&path, known, &sink, &mut stop, &hello, &queue, &mut pin).await {
            Ok(Exit::Stopped) => return,
            Ok(Exit::Unsupported) => {
                sink(HostEvent::Unsupported);
                return;
            }
            Ok(Exit::Changed(sha256)) => {
                sink(HostEvent::FileChanged { sha256 });
                return;
            }
            Err(Failure::Fatal(error)) => {
                sink(HostEvent::Stopped {
                    error,
                    notify: true,
                });
                return;
            }
            Err(Failure::Retry(error)) => {
                // A run that lasted a while was healthy: start over.
                if started.elapsed() > Duration::from_secs(60) {
                    backoff = Duration::from_secs(1);
                    failing = false;
                }
                let notify = !failing && last_notice.is_none_or(|at| at.elapsed() >= NOTICE_EVERY);
                if notify {
                    last_notice = Some(Instant::now());
                }
                sink(HostEvent::Stopped {
                    error: error.to_string(),
                    notify,
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
    /// The file no longer matches its pin; this is its hash.
    Changed(String),
}

enum Failure {
    /// Retrying cannot help: the host ends.
    Fatal(String),
    Retry(std::io::Error),
}

impl From<std::io::Error> for Failure {
    fn from(error: std::io::Error) -> Self {
        Failure::Retry(error)
    }
}

/// Sorts an error from reading or starting the file: some never get better.
fn classify(error: std::io::Error) -> Failure {
    use std::io::ErrorKind::{NotFound, PermissionDenied};
    // ERROR_BAD_EXE_FORMAT on Windows, ENOEXEC elsewhere.
    #[cfg(windows)]
    const NOT_EXECUTABLE: i32 = 193;
    #[cfg(not(windows))]
    const NOT_EXECUTABLE: i32 = 8;
    if matches!(error.kind(), NotFound | PermissionDenied)
        || error.raw_os_error() == Some(NOT_EXECUTABLE)
    {
        Failure::Fatal(error.to_string())
    } else {
        Failure::Retry(error)
    }
}

/// The SHA-256 of the file, as lowercase hex, read off the runtime's threads.
async fn fingerprint(path: &std::path::Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        let mut file = std::fs::File::open(path)?;
        let mut hash = Sha256::new();
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hash.update(&buffer[..read]);
        }
        Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
    })
    .await
    .map_err(std::io::Error::other)?
}

/// Names an environment variable that is Chaat's own (`CHAAT_*`): the
/// plugin does not inherit those. Case-insensitive on Windows, where
/// variable names are.
fn is_chaat_variable(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    let prefix = "CHAAT_";
    match name.get(..prefix.len()) {
        Some(start) if cfg!(windows) => start.eq_ignore_ascii_case(prefix),
        Some(start) => start == prefix,
        None => false,
    }
}

/// Checks the file against its pin, then runs it once.
async fn attempt(
    path: &std::path::Path,
    known: &[&str],
    sink: &Sink,
    stop: &mut watch::Receiver<bool>,
    hello: &watch::Receiver<HostMessage>,
    queue: &Queue,
    pin: &mut Option<String>,
) -> Result<Exit, Failure> {
    let sha256 = fingerprint(path).await.map_err(classify)?;
    match pin {
        None => {
            sink(HostEvent::Fingerprint {
                sha256: sha256.clone(),
            });
            *pin = Some(sha256);
        }
        Some(pinned) if pinned.eq_ignore_ascii_case(&sha256) => {}
        Some(_) => return Ok(Exit::Changed(sha256)),
    }
    let hello = hello.borrow().clone();
    run_once(path, known, sink, stop, hello, queue).await
}

/// Ends a task when dropped, so a run that ends (or is aborted) takes its
/// writer with it.
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Why the writer ended.
enum WriteEnd {
    /// It was told to finish and sent `shutdown`.
    Finished,
    /// A write took longer than `WRITE_TIMEOUT`: the plugin is not reading.
    TimedOut,
    /// The plugin's input is closed.
    Failed,
}

async fn write_line(
    stdin: &mut tokio::process::ChildStdin,
    message: &HostMessage,
) -> Result<(), WriteEnd> {
    let line = message.to_line();
    let write = async {
        stdin.write_all(line.as_bytes()).await?;
        stdin.flush().await
    };
    match tokio::time::timeout(WRITE_TIMEOUT, write).await {
        Err(_) => Err(WriteEnd::TimedOut),
        Ok(Err(_)) => Err(WriteEnd::Failed),
        Ok(Ok(())) => Ok(()),
    }
}

/// The only task that writes to the plugin: `hello`, then queued messages in
/// order, until told to finish (then what is still queued, and `shutdown`).
async fn write_loop(
    mut stdin: tokio::process::ChildStdin,
    queue: Queue,
    hello: HostMessage,
    mut finish: oneshot::Receiver<()>,
) -> WriteEnd {
    async fn go(
        stdin: &mut tokio::process::ChildStdin,
        queue: &mut mpsc::Receiver<HostMessage>,
        hello: HostMessage,
        finish: &mut oneshot::Receiver<()>,
    ) -> Result<(), WriteEnd> {
        // Messages left over from before this run are about a process that
        // is gone; `hello` carries the current state.
        while queue.try_recv().is_ok() {}
        write_line(stdin, &hello).await?;
        loop {
            tokio::select! {
                // Messages first: a `forget` sent just before a stop still
                // goes out.
                biased;
                Some(message) = queue.recv() => write_line(stdin, &message).await?,
                _ = &mut *finish => {
                    while let Ok(message) = queue.try_recv() {
                        write_line(stdin, &message).await?;
                    }
                    return write_line(stdin, &HostMessage::Shutdown).await;
                }
            }
        }
    }
    // Held for the whole run, so the next run's writer waits for this one.
    let mut queue = queue.lock().await;
    match go(&mut stdin, &mut queue, hello, &mut finish).await {
        Ok(()) => WriteEnd::Finished,
        Err(end) => end,
    }
}

/// A token bucket for the plugin's lines.
struct Bucket {
    tokens: f64,
    at: Instant,
}

impl Bucket {
    fn new(now: Instant) -> Self {
        Self {
            tokens: LINE_BURST,
            at: now,
        }
    }

    /// Takes a token for one line; false if there is none.
    fn take(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.at).as_secs_f64();
        self.at = now;
        self.tokens = (self.tokens + elapsed * LINES_PER_SECOND).min(LINE_BURST);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// One run of the plugin process, until it is stopped or exits.
async fn run_once(
    path: &std::path::Path,
    known: &[&str],
    sink: &Sink,
    stop: &mut watch::Receiver<bool>,
    hello: HostMessage,
    queue: &Queue,
) -> Result<Exit, Failure> {
    let mut command = tokio::process::Command::new(path);
    command
        .arg("serve")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    if let Some(folder) = path.parent() {
        command.current_dir(folder);
    }
    for (name, _) in std::env::vars_os() {
        if is_chaat_variable(&name) {
            command.env_remove(name);
        }
    }
    // Chaat is a windowed app; without this a console plugin opens a console.
    #[cfg(target_os = "windows")]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    // Its own process group, so the whole group can be killed.
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().map_err(classify)?;
    // Dropped (killing the tree) however this function ends, also when the
    // task is aborted.
    let _tree = tree::Tree::new(&child);
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::other("plugin stdin unavailable"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("plugin stdout unavailable"))?;
    let (finish, finish_rx) = oneshot::channel();
    let mut finish = Some(finish);
    let mut writer = AbortOnDrop(tokio::spawn(write_loop(
        stdin,
        Arc::clone(queue),
        hello,
        finish_rx,
    )));

    let mut lines = LineReader::new(tokio::io::BufReader::new(stdout));
    let mut bucket = Bucket::new(Instant::now());
    let mut flooding = false;
    let mut long_line_logged = false;
    loop {
        tokio::select! {
            biased;
            // The writer ended by itself: the plugin stopped reading, or
            // closed its input.
            end = &mut writer.0 => {
                let reason = match end {
                    Ok(WriteEnd::TimedOut) => "plugin stopped reading its input",
                    _ => "plugin closed its input",
                };
                return Err(std::io::Error::other(reason).into());
            }
            // An error means the `Plugin` handle is gone: stop too.
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    if let Some(finish) = finish.take() {
                        let _ = finish.send(());
                    }
                    // The writer sends what is queued and `shutdown`; a
                    // plugin that does not read gets a second, then the kill.
                    let _ = tokio::time::timeout(Duration::from_secs(1), &mut writer.0).await;
                    let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
                    let _ = child.start_kill();
                    return Ok(Exit::Stopped);
                }
            }
            line = lines.next() => {
                let Some(line) = line? else {
                    return Err(std::io::Error::other("plugin closed its output").into());
                };
                if !bucket.take(Instant::now()) {
                    if !std::mem::replace(&mut flooding, true) {
                        log::info!("plugin: sending lines too fast; dropping the excess");
                        sink(HostEvent::Flooding);
                    }
                    continue;
                }
                match line {
                    Line::TooLong => {
                        if !std::mem::replace(&mut long_line_logged, true) {
                            log::info!("plugin: dropped an over-long line");
                        }
                    }
                    Line::Text(line) => match PluginMessage::parse(&line) {
                        Some(PluginMessage::Ready { capabilities, .. })
                            if !capabilities.iter().any(|c| known.contains(&c.as_str())) =>
                        {
                            let _ = child.start_kill();
                            return Ok(Exit::Unsupported);
                        }
                        Some(message) => sink(HostEvent::Message(message)),
                        None => {}
                    },
                }
            }
        }
    }
}

/// Everything a plugin starts, so that ending its run ends all of it.
mod tree {
    /// Kills the plugin's process tree when dropped.
    #[cfg(windows)]
    pub struct Tree(windows_sys::Win32::Foundation::HANDLE);

    // SAFETY: a job handle is only closed, and may be closed from any thread.
    #[cfg(windows)]
    unsafe impl Send for Tree {}

    #[cfg(windows)]
    impl Tree {
        /// Puts `child` in a new Job Object that kills its processes when the
        /// last handle closes. Without one (the system refused), the plugin
        /// still runs and only the plugin process itself is killed. A process
        /// the plugin starts before it is in the job is not covered; the gap
        /// is a few microseconds after the start.
        pub fn new(child: &tokio::process::Child) -> Option<Self> {
            use windows_sys::Win32::Foundation::CloseHandle;
            use windows_sys::Win32::System::JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject,
            };
            let process = child.raw_handle()?;
            // SAFETY: plain Win32 calls with valid arguments. `process` is the
            // child's own handle, open while `child` lives. `info` is a fully
            // initialised struct whose size is passed. The job handle is
            // closed on every failure path and otherwise by `Drop`.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    log::info!("plugin: could not create a job object");
                    return None;
                }
                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let limited = SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    std::ptr::from_ref(&info).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if limited == 0 || AssignProcessToJobObject(job, process.cast()) == 0 {
                    log::info!("plugin: could not put the plugin in a job object");
                    CloseHandle(job);
                    return None;
                }
                Some(Self(job))
            }
        }
    }

    #[cfg(windows)]
    impl Drop for Tree {
        fn drop(&mut self) {
            // SAFETY: the handle is ours and closed once; closing it kills
            // every process in the job (KILL_ON_JOB_CLOSE).
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.0);
            }
        }
    }

    /// Kills the plugin's process group when dropped.
    #[cfg(unix)]
    pub struct Tree(i32);

    #[cfg(unix)]
    impl Tree {
        /// The child leads its own group (`process_group(0)`), so the group id
        /// is its process id.
        pub fn new(child: &tokio::process::Child) -> Option<Self> {
            // Never 0 or 1: `kill(-1, ..)` would signal every process we may.
            let pid = i32::try_from(child.id()?).ok().filter(|pid| *pid > 1)?;
            Some(Self(pid))
        }
    }

    #[cfg(unix)]
    impl Drop for Tree {
        fn drop(&mut self) {
            // SAFETY: kill(2) with a negative pid signals the group; the
            // value is the id of a group this run created. If every member is
            // already gone it fails with ESRCH, which is ignored.
            unsafe {
                libc::kill(-self.0, libc::SIGKILL);
            }
        }
    }

    #[cfg(not(any(windows, unix)))]
    pub struct Tree;

    #[cfg(not(any(windows, unix)))]
    impl Tree {
        pub fn new(_child: &tokio::process::Child) -> Option<Self> {
            None
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

    #[test]
    fn the_bucket_allows_a_burst_then_the_rate() {
        let start = Instant::now();
        let mut bucket = Bucket::new(start);
        let burst = (0..200).filter(|_| bucket.take(start)).count();
        assert_eq!(burst, 50);
        // Half a second later: ten more lines.
        let later = start + Duration::from_millis(500);
        let more = (0..200).filter(|_| bucket.take(later)).count();
        assert_eq!(more, 10);
        // A long quiet spell refills the burst, no further.
        let much_later = later + Duration::from_secs(3600);
        let refilled = (0..200).filter(|_| bucket.take(much_later)).count();
        assert_eq!(refilled, 50);
    }

    #[test]
    fn chaat_variables_are_recognised_by_prefix() {
        use std::ffi::OsStr;
        assert!(is_chaat_variable(OsStr::new("CHAAT_TOKEN")));
        assert!(!is_chaat_variable(OsStr::new("CHAAT")));
        assert!(!is_chaat_variable(OsStr::new("MY_CHAAT_X")));
        assert!(!is_chaat_variable(OsStr::new("PATH")));
        assert_eq!(is_chaat_variable(OsStr::new("chaat_x")), cfg!(windows));
    }

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
