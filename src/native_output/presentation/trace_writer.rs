use super::trace::{
    PresentationTransactionTraceRing, TraceExportRecord, append_trace_record_jsonl,
};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const TRACE_BATCH_EVENT_CAPACITY: usize = 128;
const TRACE_QUEUE_BATCH_CAPACITY: usize = 4;
const TRACE_BATCH_POOL_CAPACITY: usize = TRACE_QUEUE_BATCH_CAPACITY + 1;
const TRACE_BATCH_FLUSH_INTERVAL: Duration = Duration::from_millis(20);
const MAX_TRACE_LINE_BYTES: u64 = 4096;
const DEFAULT_TRACE_MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MIN_TRACE_MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_TRACE_MAX_FILE_BYTES: u64 = 1024 * 1024 * 1024;
const DEFAULT_TRACE_MAX_FILES: usize = 4;
const MAX_TRACE_FILES: usize = 16;
const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(50);
const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(1);
const RETENTION_EXPIRED_JSONL: &str = "{\"schema_version\":1,\"record\":\"retention_expired\",\"scope\":\"trace_file_set\",\"reason\":\"file_retention\"}\n";

#[derive(Debug, Clone, Copy)]
struct TraceWriterConfig {
    max_file_bytes: u64,
    max_files: usize,
    startup_timeout: Duration,
    shutdown_timeout: Duration,
}

impl TraceWriterConfig {
    fn from_env() -> Self {
        let max_file_bytes = std::env::var("OBLIVION_ONE_PRESENTATION_TRACE_MAX_BYTES")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_TRACE_MAX_FILE_BYTES)
            .clamp(MIN_TRACE_MAX_FILE_BYTES, MAX_TRACE_MAX_FILE_BYTES);
        let max_files = std::env::var("OBLIVION_ONE_PRESENTATION_TRACE_MAX_FILES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(DEFAULT_TRACE_MAX_FILES)
            .clamp(1, MAX_TRACE_FILES);
        Self {
            max_file_bytes,
            max_files,
            startup_timeout: DEFAULT_STARTUP_TIMEOUT,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
        }
    }
}

#[derive(Debug, Default)]
struct TraceWriterStats {
    events_enqueued: AtomicU64,
    events_written: AtomicU64,
    batches_written: AtomicU64,
    records_written: AtomicU64,
    bytes_written: AtomicU64,
    write_errors: AtomicU64,
    retries: AtomicU64,
    consumer_stalls: AtomicU64,
    worker_disconnects: AtomicU64,
    buffer_return_failures: AtomicU64,
    shutdown_unqueued_events: AtomicU64,
    shutdown_timeouts: AtomicU64,
    startup_errors: AtomicU64,
    startup_timeouts: AtomicU64,
    startup_cancellations: AtomicU64,
    writer_panics: AtomicU64,
}

impl TraceWriterStats {
    fn report_if_needed(&self) {
        let write_errors = self.write_errors.load(Ordering::Relaxed);
        let stalls = self.consumer_stalls.load(Ordering::Relaxed);
        let disconnects = self.worker_disconnects.load(Ordering::Relaxed);
        let return_failures = self.buffer_return_failures.load(Ordering::Relaxed);
        let shutdown_unqueued = self.shutdown_unqueued_events.load(Ordering::Relaxed);
        let shutdown_timeouts = self.shutdown_timeouts.load(Ordering::Relaxed);
        let startup_errors = self.startup_errors.load(Ordering::Relaxed);
        let startup_timeouts = self.startup_timeouts.load(Ordering::Relaxed);
        let startup_cancellations = self.startup_cancellations.load(Ordering::Relaxed);
        let panics = self.writer_panics.load(Ordering::Relaxed);
        if write_errors == 0
            && stalls == 0
            && disconnects == 0
            && return_failures == 0
            && shutdown_unqueued == 0
            && shutdown_timeouts == 0
            && startup_errors == 0
            && startup_timeouts == 0
            && startup_cancellations == 0
            && panics == 0
        {
            return;
        }
        eprintln!(
            "typhon presentation trace writer: write_errors={write_errors} retries={} consumer_stalls={stalls} worker_disconnects={disconnects} buffer_return_failures={return_failures} events_enqueued={} events_written={} batches_written={} records_written={} bytes_written={} shutdown_unqueued_events={shutdown_unqueued} shutdown_timeouts={shutdown_timeouts} startup_errors={startup_errors} startup_timeouts={startup_timeouts} startup_cancellations={startup_cancellations} panics={panics}",
            self.retries.load(Ordering::Relaxed),
            self.events_enqueued.load(Ordering::Relaxed),
            self.events_written.load(Ordering::Relaxed),
            self.batches_written.load(Ordering::Relaxed),
            self.records_written.load(Ordering::Relaxed),
            self.bytes_written.load(Ordering::Relaxed),
        );
    }
}

struct TraceBatch {
    records: Vec<TraceExportRecord>,
    last_sequence: Option<u128>,
}

impl TraceBatch {
    fn new() -> Self {
        Self {
            records: Vec::with_capacity(TRACE_BATCH_EVENT_CAPACITY + 1),
            last_sequence: None,
        }
    }

    fn clear(&mut self) {
        self.records.clear();
        self.last_sequence = None;
    }

    fn event_count(&self) -> u64 {
        self.records
            .iter()
            .filter(|record| matches!(record, TraceExportRecord::Event(_)))
            .count() as u64
    }
}

struct TraceBatchProducer {
    sender: Option<SyncSender<TraceBatch>>,
    reusable_batches: Receiver<TraceBatch>,
    scratch: Option<TraceBatch>,
    scratch_started_at: Option<Instant>,
    last_enqueued_sequence: Option<u128>,
    stats: Arc<TraceWriterStats>,
    consumer_stalled: bool,
    worker_disconnected: bool,
}

impl TraceBatchProducer {
    fn new(
        sender: SyncSender<TraceBatch>,
        reusable_batches: Receiver<TraceBatch>,
        stats: Arc<TraceWriterStats>,
        last_enqueued_sequence: Option<u128>,
    ) -> Self {
        Self {
            sender: Some(sender),
            reusable_batches,
            scratch: None,
            scratch_started_at: None,
            last_enqueued_sequence,
            stats,
            consumer_stalled: false,
            worker_disconnected: false,
        }
    }

    fn submit(&mut self, trace: &PresentationTransactionTraceRing, force: bool) {
        let buffered_sequence = self
            .scratch
            .as_ref()
            .and_then(|batch| batch.last_sequence)
            .or(self.last_enqueued_sequence);
        let has_new_events = trace
            .latest_sequence()
            .is_some_and(|latest| buffered_sequence.is_none_or(|buffered| latest > buffered));
        if self.scratch.is_none() && !has_new_events {
            return;
        }
        if self.scratch.is_none() {
            match self.reusable_batches.try_recv() {
                Ok(batch) => self.scratch = Some(batch),
                Err(TryRecvError::Empty) => {
                    self.note_consumer_stall();
                    return;
                }
                Err(TryRecvError::Disconnected) => {
                    self.note_worker_disconnect();
                    return;
                }
            }
        }

        let Some(batch) = self.scratch.as_mut() else {
            return;
        };
        if has_new_events {
            let previous = batch.last_sequence.or(self.last_enqueued_sequence);
            if batch.records.is_empty() {
                self.scratch_started_at = Some(Instant::now());
            }
            if let Some(sequence) = trace.append_delta_records(previous, &mut batch.records) {
                batch.last_sequence = Some(sequence);
            }
        }
        if batch.records.is_empty() {
            return;
        }

        let flush_due = force
            || batch.records.len() >= batch.records.capacity()
            || self
                .scratch_started_at
                .is_some_and(|started_at| started_at.elapsed() >= TRACE_BATCH_FLUSH_INTERVAL);
        if !flush_due {
            return;
        }

        let Some(sender) = self.sender.as_ref() else {
            return;
        };
        let batch = self.scratch.take().expect("trace batch acquired above");
        let next_sequence = batch.last_sequence;
        let event_count = batch.event_count();
        match sender.try_send(batch) {
            Ok(()) => {
                self.last_enqueued_sequence = next_sequence;
                self.scratch_started_at = None;
                self.stats
                    .events_enqueued
                    .fetch_add(event_count, Ordering::Relaxed);
                self.consumer_stalled = false;
            }
            Err(TrySendError::Full(batch)) => {
                self.scratch = Some(batch);
                self.note_consumer_stall();
            }
            Err(TrySendError::Disconnected(batch)) => {
                self.scratch = Some(batch);
                self.note_worker_disconnect();
            }
        }
    }

    fn note_consumer_stall(&mut self) {
        if !self.consumer_stalled {
            self.stats.consumer_stalls.fetch_add(1, Ordering::Relaxed);
            self.consumer_stalled = true;
        }
    }

    fn note_worker_disconnect(&mut self) {
        if !self.worker_disconnected {
            self.stats
                .worker_disconnects
                .fetch_add(1, Ordering::Relaxed);
            self.worker_disconnected = true;
        }
    }

    fn wait_for_writer_progress(&mut self, timeout: Duration) {
        if self.scratch.is_some() {
            thread::park_timeout(timeout);
            return;
        }
        match self.reusable_batches.recv_timeout(timeout) {
            Ok(batch) if self.scratch.is_none() => self.scratch = Some(batch),
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => self.note_worker_disconnect(),
        }
    }

    fn pending_event_count(&self, latest_sequence: Option<u128>) -> u64 {
        let Some(latest) = latest_sequence else {
            return 0;
        };
        let first_pending = self
            .last_enqueued_sequence
            .and_then(|sequence| sequence.checked_add(1))
            .unwrap_or(1);
        if latest < first_pending {
            return 0;
        }
        latest
            .saturating_sub(first_pending)
            .saturating_add(1)
            .min(u64::MAX as u128) as u64
    }

    fn close(&mut self) {
        self.sender.take();
    }
}

pub(crate) struct PresentationTraceWriter {
    producer: Option<TraceBatchProducer>,
    lifecycle: WriterLifecycle,
    worker: Option<JoinHandle<()>>,
    completion: Receiver<()>,
    stop_requested: Arc<AtomicBool>,
    stats: Arc<TraceWriterStats>,
    config: TraceWriterConfig,
    closed: bool,
}

enum WriterLifecycle {
    Initializing {
        ready: Receiver<io::Result<Option<u128>>>,
        activate: SyncSender<()>,
        deadline: Instant,
    },
    Active,
    Disabled,
}

enum StartupPoll {
    Pending,
    Ready(Option<u128>),
    Failed,
    TimedOut,
}

#[cfg(test)]
#[derive(Clone, Default)]
struct TraceWriterTestHooks {
    before_recovery: Option<Arc<dyn Fn() + Send + Sync>>,
    after_recovery: Option<Arc<dyn Fn() + Send + Sync>>,
    after_ready_sent: Option<Arc<dyn Fn() + Send + Sync>>,
    before_write: Option<Arc<dyn Fn() + Send + Sync>>,
}

#[cfg(not(test))]
#[derive(Clone, Default)]
struct TraceWriterTestHooks;

impl PresentationTraceWriter {
    pub(crate) fn start(
        path: PathBuf,
        trace: &PresentationTransactionTraceRing,
    ) -> io::Result<Self> {
        Self::start_with_config(path, TraceWriterConfig::from_env(), trace)
    }

    fn start_with_config(
        path: PathBuf,
        config: TraceWriterConfig,
        trace: &PresentationTransactionTraceRing,
    ) -> io::Result<Self> {
        Self::start_with_hooks(path, config, trace, TraceWriterTestHooks::default())
    }

    fn start_with_hooks(
        path: PathBuf,
        config: TraceWriterConfig,
        trace: &PresentationTransactionTraceRing,
        test_hooks: TraceWriterTestHooks,
    ) -> io::Result<Self> {
        let source_identity = trace.source_identity();
        let (sender, receiver) = mpsc::sync_channel(TRACE_QUEUE_BATCH_CAPACITY);
        let (reusable_sender, reusable_batches) = mpsc::sync_channel(TRACE_BATCH_POOL_CAPACITY);
        for _ in 0..TRACE_BATCH_POOL_CAPACITY {
            reusable_sender
                .send(TraceBatch::new())
                .map_err(|_| io::Error::other("presentation trace buffer pool closed"))?;
        }

        let stats = Arc::new(TraceWriterStats::default());
        let worker_stats = Arc::clone(&stats);
        let stop_requested = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop_requested);
        let (completion_sender, completion) = mpsc::sync_channel(1);
        let (ready_sender, ready) = mpsc::sync_channel(1);
        let (activate_sender, activate_receiver) = mpsc::sync_channel(1);
        let deadline = Instant::now() + config.startup_timeout;
        let worker = thread::Builder::new()
            .name("typhon-trace-writer".to_owned())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    #[cfg(test)]
                    if let Some(hook) = test_hooks.before_recovery.as_ref() {
                        hook();
                    }
                    let file_writer = TraceFileWriter::new(path, config);
                    match file_writer.resume_sequence(source_identity) {
                        Ok(last_sequence) => {
                            if worker_stop.load(Ordering::Acquire) {
                                return;
                            }
                            #[cfg(test)]
                            if let Some(hook) = test_hooks.after_recovery.as_ref() {
                                hook();
                            }
                            if ready_sender.send(Ok(last_sequence)).is_ok() {
                                #[cfg(test)]
                                if let Some(hook) = test_hooks.after_ready_sent.as_ref() {
                                    hook();
                                }
                                if activate_receiver.recv().is_ok() {
                                    run_writer(
                                        file_writer,
                                        receiver,
                                        reusable_sender,
                                        worker_stats.as_ref(),
                                        &worker_stop,
                                        test_hooks,
                                    );
                                }
                            }
                        }
                        Err(error) => {
                            let _ = ready_sender.send(Err(error));
                        }
                    }
                }));
                if result.is_err() {
                    worker_stats.writer_panics.fetch_add(1, Ordering::Relaxed);
                }
                let _ = completion_sender.try_send(());
            })?;

        Ok(Self {
            producer: Some(TraceBatchProducer::new(
                sender,
                reusable_batches,
                Arc::clone(&stats),
                None,
            )),
            lifecycle: WriterLifecycle::Initializing {
                ready,
                activate: activate_sender,
                deadline,
            },
            worker: Some(worker),
            completion,
            stop_requested,
            stats,
            config,
            closed: false,
        })
    }

    pub(crate) fn submit(&mut self, trace: &PresentationTransactionTraceRing) {
        self.poll_startup();
        if let Some(producer) = self.producer.as_mut()
            && matches!(&self.lifecycle, WriterLifecycle::Active)
        {
            producer.submit(trace, false);
        }
    }

    fn poll_startup(&mut self) {
        let poll = match &self.lifecycle {
            WriterLifecycle::Initializing {
                ready, deadline, ..
            } => {
                if Instant::now() >= *deadline {
                    StartupPoll::TimedOut
                } else {
                    match ready.try_recv() {
                        Ok(Ok(sequence)) if Instant::now() < *deadline => {
                            StartupPoll::Ready(sequence)
                        }
                        Ok(Ok(_)) | Err(TryRecvError::Empty) if Instant::now() >= *deadline => {
                            StartupPoll::TimedOut
                        }
                        Ok(Ok(sequence)) => StartupPoll::Ready(sequence),
                        Ok(Err(_)) | Err(TryRecvError::Disconnected) => StartupPoll::Failed,
                        Err(TryRecvError::Empty) => StartupPoll::Pending,
                    }
                }
            }
            WriterLifecycle::Active | WriterLifecycle::Disabled => StartupPoll::Pending,
        };

        match poll {
            StartupPoll::Pending => {}
            StartupPoll::Ready(last_sequence) => {
                let activated = match &self.lifecycle {
                    WriterLifecycle::Initializing { activate, .. } => activate.try_send(()).is_ok(),
                    WriterLifecycle::Active | WriterLifecycle::Disabled => false,
                };
                if activated {
                    if let Some(producer) = self.producer.as_mut() {
                        producer.last_enqueued_sequence = last_sequence;
                    }
                    self.lifecycle = WriterLifecycle::Active;
                } else {
                    self.stats.startup_errors.fetch_add(1, Ordering::Relaxed);
                    self.abandon_initializing_writer();
                }
            }
            StartupPoll::Failed => {
                self.stats.startup_errors.fetch_add(1, Ordering::Relaxed);
                self.abandon_initializing_writer();
            }
            StartupPoll::TimedOut => {
                self.stats.startup_timeouts.fetch_add(1, Ordering::Relaxed);
                self.abandon_initializing_writer();
            }
        }
    }

    fn abandon_initializing_writer(&mut self) {
        self.stop_requested.store(true, Ordering::Release);
        if let Some(producer) = self.producer.as_mut() {
            producer.close();
        }
        self.lifecycle = WriterLifecycle::Disabled;
        // Recovery may be inside an uninterruptible filesystem call. The
        // worker owns no runtime state; dropping its handle detaches it.
        self.worker.take();
    }

    fn abandon_initializing_writer_for_shutdown(&mut self) {
        if matches!(&self.lifecycle, WriterLifecycle::Initializing { .. }) {
            self.stats
                .startup_cancellations
                .fetch_add(1, Ordering::Relaxed);
            self.abandon_initializing_writer();
        }
    }

    pub(crate) fn shutdown(&mut self, trace: &PresentationTransactionTraceRing) {
        if self.closed {
            return;
        }
        self.poll_startup();
        self.abandon_initializing_writer_for_shutdown();
        if !matches!(&self.lifecycle, WriterLifecycle::Active) {
            if let Some(producer) = self.producer.as_mut() {
                self.stats.shutdown_unqueued_events.fetch_add(
                    producer.pending_event_count(trace.latest_sequence()),
                    Ordering::Relaxed,
                );
                producer.close();
            }
            self.stop_requested.store(true, Ordering::Release);
            self.closed = true;
            self.stats.report_if_needed();
            return;
        }
        let deadline = Instant::now() + self.config.shutdown_timeout;
        let latest_sequence = trace.latest_sequence();
        let producer = self
            .producer
            .as_mut()
            .expect("active trace writer has its producer");
        while producer.pending_event_count(latest_sequence) != 0 && Instant::now() < deadline {
            producer.submit(trace, true);
            if producer.pending_event_count(latest_sequence) != 0 {
                let remaining = deadline.saturating_duration_since(Instant::now());
                producer.wait_for_writer_progress(remaining.min(Duration::from_millis(1)));
            }
        }
        self.stats.shutdown_unqueued_events.fetch_add(
            producer.pending_event_count(latest_sequence),
            Ordering::Relaxed,
        );

        self.stop_requested.store(true, Ordering::Release);
        producer.close();
        if let Some(worker) = self.worker.as_ref() {
            worker.thread().unpark();
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        match self.completion.recv_timeout(remaining) {
            Ok(()) => {
                self.join_worker_if_finished();
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.stats.shutdown_timeouts.fetch_add(1, Ordering::Relaxed);
                self.worker.take();
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.worker.take();
            }
        }
        self.closed = true;
        self.stats.report_if_needed();
    }

    fn join_worker_if_finished(&mut self) {
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished)
            && let Some(worker) = self.worker.take()
        {
            let _ = worker.join();
        } else {
            self.worker.take();
        }
    }
}

impl Drop for PresentationTraceWriter {
    fn drop(&mut self) {
        if self.closed {
            return;
        }
        self.abandon_initializing_writer_for_shutdown();
        self.stop_requested.store(true, Ordering::Release);
        if let Some(producer) = self.producer.as_mut() {
            producer.close();
        }
        if !matches!(&self.lifecycle, WriterLifecycle::Active) {
            self.worker.take();
            self.stats.report_if_needed();
            return;
        }
        if let Some(worker) = self.worker.as_ref() {
            worker.thread().unpark();
        }
        if self
            .completion
            .recv_timeout(self.config.shutdown_timeout)
            .is_ok()
        {
            self.join_worker_if_finished();
        }
        self.worker.take();
        self.stats.report_if_needed();
    }
}

fn run_writer(
    mut file_writer: TraceFileWriter,
    receiver: Receiver<TraceBatch>,
    reusable_sender: SyncSender<TraceBatch>,
    stats: &TraceWriterStats,
    stop_requested: &AtomicBool,
    test_hooks: TraceWriterTestHooks,
) {
    #[cfg(not(test))]
    let _ = test_hooks;
    let mut output = String::with_capacity(TRACE_BATCH_EVENT_CAPACITY * 384);
    let mut retry_delay = Duration::from_millis(10);
    while let Ok(mut batch) = receiver.recv() {
        let mut written = false;
        loop {
            #[cfg(test)]
            if let Some(hook) = test_hooks.before_write.as_ref() {
                hook();
            }
            match file_writer.write_batch(&batch, &mut output) {
                Ok(bytes) => {
                    stats
                        .events_written
                        .fetch_add(batch.event_count(), Ordering::Relaxed);
                    stats.batches_written.fetch_add(1, Ordering::Relaxed);
                    stats
                        .records_written
                        .fetch_add(batch.records.len() as u64, Ordering::Relaxed);
                    stats.bytes_written.fetch_add(bytes, Ordering::Relaxed);
                    written = true;
                    retry_delay = Duration::from_millis(10);
                    break;
                }
                Err(BatchWriteError::Retry(_error)) => {
                    stats.write_errors.fetch_add(1, Ordering::Relaxed);
                    if stop_requested.load(Ordering::Acquire) {
                        break;
                    }
                    stats.retries.fetch_add(1, Ordering::Relaxed);
                    thread::park_timeout(retry_delay);
                    retry_delay = retry_delay
                        .saturating_mul(2)
                        .min(Duration::from_millis(500));
                }
                Err(BatchWriteError::Fatal(_error)) => {
                    stats.write_errors.fetch_add(1, Ordering::Relaxed);
                    break;
                }
            }
        }
        if !written {
            stats
                .shutdown_unqueued_events
                .fetch_add(batch.event_count(), Ordering::Relaxed);
            while let Ok(queued) = receiver.try_recv() {
                stats
                    .shutdown_unqueued_events
                    .fetch_add(queued.event_count(), Ordering::Relaxed);
            }
            break;
        }
        batch.clear();
        if reusable_sender.try_send(batch).is_err() {
            stats.buffer_return_failures.fetch_add(1, Ordering::Relaxed);
        }
    }
    if let Err(_error) = file_writer.sync_data() {
        stats.write_errors.fetch_add(1, Ordering::Relaxed);
    }
}

trait TraceFileIo: Read + Write + Seek {
    fn trace_len(&mut self) -> io::Result<u64>;
    fn trace_set_len(&mut self, len: u64) -> io::Result<()>;
}

impl TraceFileIo for File {
    fn trace_len(&mut self) -> io::Result<u64> {
        self.metadata().map(|metadata| metadata.len())
    }

    fn trace_set_len(&mut self, len: u64) -> io::Result<()> {
        self.set_len(len)
    }
}

#[derive(Debug)]
enum BatchWriteError {
    Retry(io::Error),
    Fatal(io::Error),
}

fn write_batch_bytes(file: &mut impl TraceFileIo, bytes: &[u8]) -> Result<u64, BatchWriteError> {
    let start = file.trace_len().map_err(BatchWriteError::Retry)?;
    file.seek(SeekFrom::Start(start))
        .map_err(BatchWriteError::Retry)?;
    if let Err(error) = file.write_all(bytes) {
        let rollback = file
            .trace_set_len(start)
            .and_then(|()| file.seek(SeekFrom::Start(start)).map(|_| ()));
        if let Err(rollback_error) = rollback {
            return Err(BatchWriteError::Fatal(io::Error::other(format!(
                "trace write failed ({error}); rollback failed ({rollback_error})"
            ))));
        }
        return Err(BatchWriteError::Retry(error));
    }
    Ok(start.saturating_add(bytes.len() as u64))
}

struct TraceFileWriter {
    path: PathBuf,
    file: Option<File>,
    current_bytes: u64,
    retention_record_pending: bool,
    config: TraceWriterConfig,
    #[cfg(test)]
    fail_next_rename: bool,
}

impl TraceFileWriter {
    fn new(path: PathBuf, config: TraceWriterConfig) -> Self {
        Self {
            path,
            file: None,
            current_bytes: 0,
            retention_record_pending: false,
            config,
            #[cfg(test)]
            fail_next_rename: false,
        }
    }

    fn write_batch(
        &mut self,
        batch: &TraceBatch,
        output: &mut String,
    ) -> Result<u64, BatchWriteError> {
        output.clear();
        for record in batch.records.iter().copied() {
            append_trace_record_jsonl(output, record);
        }
        self.ensure_open().map_err(BatchWriteError::Retry)?;
        if self.current_bytes != 0
            && self.current_bytes.saturating_add(output.len() as u64) > self.config.max_file_bytes
        {
            self.file = None;
            self.rotate_paths().map_err(BatchWriteError::Retry)?;
            self.open_new().map_err(BatchWriteError::Retry)?;
        }
        if self.retention_record_pending {
            output.insert_str(0, RETENTION_EXPIRED_JSONL);
        }

        let file = self.file.as_mut().expect("trace file opened above");
        let bytes = match write_batch_bytes(file, output.as_bytes()) {
            Ok(bytes) => bytes,
            Err(error) => {
                if matches!(error, BatchWriteError::Fatal(_)) {
                    self.file = None;
                }
                return Err(error);
            }
        };
        self.current_bytes = bytes;
        self.retention_record_pending = false;
        Ok(output.len() as u64)
    }

    fn resume_sequence(&self, source_identity: (u32, u128)) -> io::Result<Option<u128>> {
        let mut latest_sequence = None;
        for index in 0..self.config.max_files {
            let path = if index == 0 {
                self.path.clone()
            } else {
                rotated_path(&self.path, index)
            };
            let file = match OpenOptions::new().read(true).write(index == 0).open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            let mut file = file;
            let Some(line) = last_complete_json_line(&mut file, index == 0)? else {
                continue;
            };
            let record: TraceResumeRecord = serde_json::from_slice(&line).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "invalid final presentation trace record in {}: {error}",
                        path.display()
                    ),
                )
            })?;
            if record.process_id == Some(source_identity.0)
                && record.source_id == Some(source_identity.1)
                && let Some(sequence) = record.sequence
            {
                latest_sequence =
                    Some(latest_sequence.map_or(sequence, |latest: u128| latest.max(sequence)));
            }
        }
        Ok(latest_sequence)
    }

    fn ensure_open(&mut self) -> io::Result<()> {
        if self.file.is_some() {
            return Ok(());
        }
        if let Some(parent) = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        self.prune_old_rotations()?;
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&self.path)?;
        let len = file.metadata()?.len();
        if len > self.config.max_file_bytes {
            drop(file);
            let path = self.path.clone();
            self.remove_retained_file(&path)?;
            file = self.open_file()?;
            self.current_bytes = 0;
        } else if self.retention_record_pending && len > 0 {
            drop(file);
            self.rotate_paths()?;
            file = self.open_file()?;
            self.current_bytes = file.metadata()?.len();
        } else {
            self.current_bytes = len;
        }
        file.seek(SeekFrom::Start(self.current_bytes))?;
        self.file = Some(file);
        Ok(())
    }

    fn open_new(&mut self) -> io::Result<()> {
        let file = self.open_file()?;
        self.current_bytes = file.metadata()?.len();
        self.file = Some(file);
        Ok(())
    }

    fn open_file(&self) -> io::Result<File> {
        OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&self.path)
    }

    fn rotate_paths(&mut self) -> io::Result<()> {
        if self.config.max_files <= 1 {
            let path = self.path.clone();
            return self.remove_retained_file(&path);
        }
        let max_rotations = self.config.max_files - 1;
        self.remove_retained_file(&rotated_path(&self.path, max_rotations))?;
        for index in (1..max_rotations).rev() {
            let source = rotated_path(&self.path, index);
            let destination = rotated_path(&self.path, index + 1);
            if path_exists(&source) {
                self.remove_retained_file(&destination)?;
                #[cfg(test)]
                if std::mem::take(&mut self.fail_next_rename) {
                    return Err(io::Error::other("injected trace rotation rename failure"));
                }
                fs::rename(source, destination)?;
            }
        }
        if path_exists(&self.path) {
            let first_rotation = rotated_path(&self.path, 1);
            self.remove_retained_file(&first_rotation)?;
            #[cfg(test)]
            if std::mem::take(&mut self.fail_next_rename) {
                return Err(io::Error::other("injected trace rotation rename failure"));
            }
            fs::rename(&self.path, first_rotation)?;
        }
        Ok(())
    }

    fn prune_old_rotations(&mut self) -> io::Result<()> {
        for index in self.config.max_files..=MAX_TRACE_FILES {
            self.remove_retained_file(&rotated_path(&self.path, index))?;
        }
        for index in 1..self.config.max_files {
            let path = rotated_path(&self.path, index);
            if fs::metadata(&path).is_ok_and(|metadata| metadata.len() > self.config.max_file_bytes)
            {
                self.remove_retained_file(&path)?;
            }
        }
        Ok(())
    }

    fn remove_retained_file(&mut self, path: &Path) -> io::Result<()> {
        let contained_history = fs::metadata(path).is_ok_and(|metadata| metadata.len() > 0);
        remove_if_present(path)?;
        self.retention_record_pending |= contained_history;
        Ok(())
    }

    fn sync_data(&mut self) -> io::Result<()> {
        if let Some(file) = self.file.as_mut() {
            file.sync_data()?;
        }
        Ok(())
    }
}

#[derive(serde::Deserialize)]
struct TraceResumeRecord {
    process_id: Option<u32>,
    source_id: Option<u128>,
    #[cfg(test)]
    #[serde(default, rename = "record")]
    kind: Option<String>,
    #[cfg(test)]
    #[serde(default)]
    scope: Option<String>,
    #[cfg(test)]
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    sequence: Option<u128>,
    #[cfg(test)]
    #[serde(default)]
    from_sequence: Option<u128>,
    #[cfg(test)]
    #[serde(default)]
    to_sequence: Option<u128>,
}

fn last_complete_json_line(
    file: &mut impl TraceFileIo,
    repair_partial_tail: bool,
) -> io::Result<Option<Vec<u8>>> {
    let file_len = file.trace_len()?;
    if file_len == 0 {
        return Ok(None);
    }

    file.seek(SeekFrom::End(-1))?;
    let mut final_byte = [0; 1];
    file.read_exact(&mut final_byte)?;
    let complete_len = if final_byte[0] == b'\n' {
        file_len
    } else if repair_partial_tail {
        let complete_len = find_last_newline_before(file, file_len)?
            .map(|position| position + 1)
            .unwrap_or(0);
        file.trace_set_len(complete_len)?;
        complete_len
    } else {
        return Ok(None);
    };
    if complete_len == 0 {
        return Ok(None);
    }

    let content_end = complete_len - 1;
    let line_start = find_last_newline_before(file, content_end)?
        .map(|position| position + 1)
        .unwrap_or(0);
    let line_len = content_end - line_start;
    if line_len > MAX_TRACE_LINE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "final presentation trace record exceeds the supported line size",
        ));
    }
    let mut line = vec![0; line_len as usize];
    file.seek(SeekFrom::Start(line_start))?;
    file.read_exact(&mut line)?;
    Ok(Some(line))
}

#[cfg(test)]
fn audit_trace_output(
    path: &Path,
    max_files: usize,
    expected_source: (u32, u128),
    expected_events: u64,
) -> (u64, u64, u64) {
    let mut next_sequence = 1u128;
    let mut events = 0u64;
    let mut loss_ranges = 0u64;
    let mut lost_events = 0u64;
    for index in (0..max_files).rev() {
        let candidate = if index == 0 {
            path.to_path_buf()
        } else {
            rotated_path(path, index)
        };
        let Ok(contents) = fs::read_to_string(candidate) else {
            continue;
        };
        for line in contents.lines() {
            let record: TraceResumeRecord =
                serde_json::from_str(line).expect("writer output is valid JSONL");
            if record.kind.as_deref() == Some("retention_expired") {
                assert_eq!(record.scope.as_deref(), Some("trace_file_set"));
                assert!(record.process_id.is_none());
                assert!(record.source_id.is_none());
                assert!(record.sequence.is_none());
                continue;
            }
            assert_eq!(record.process_id, Some(expected_source.0));
            assert_eq!(record.source_id, Some(expected_source.1));
            if let Some(sequence) = record.sequence {
                assert_eq!(
                    sequence, next_sequence,
                    "event sequence has an unexplained gap"
                );
                next_sequence += 1;
                events += 1;
            } else {
                assert_eq!(record.kind.as_deref(), Some("loss"));
                assert_eq!(record.reason.as_deref(), Some("retention_overwrite"));
                let from_sequence = record.from_sequence.expect("loss has a start sequence");
                let to_sequence = record.to_sequence.expect("loss has an end sequence");
                assert_eq!(from_sequence, next_sequence, "loss range is out of order");
                assert!(to_sequence >= from_sequence);
                lost_events += (to_sequence - from_sequence + 1) as u64;
                next_sequence = to_sequence + 1;
                loss_ranges += 1;
            }
        }
    }
    assert_eq!(next_sequence, u128::from(expected_events) + 1);
    (events, loss_ranges, lost_events)
}

fn find_last_newline_before(
    file: &mut (impl Read + Seek),
    mut end: u64,
) -> io::Result<Option<u64>> {
    let mut chunk = [0; 512];
    while end > 0 {
        let start = end.saturating_sub(chunk.len() as u64);
        let chunk_len = (end - start) as usize;
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut chunk[..chunk_len])?;
        if let Some(index) = chunk[..chunk_len].iter().rposition(|byte| *byte == b'\n') {
            return Ok(Some(start + index as u64));
        }
        end = start;
    }
    Ok(None)
}

fn rotated_path(path: &Path, index: usize) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{index}"));
    path.with_file_name(name)
}

fn path_exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_output::presentation::trace::{
        PresentationTransactionEvent, PresentationTransactionTraceRecord, TraceExportRecord,
    };
    use crate::native_output::presentation::transaction::OutputTransactionId;
    use std::num::NonZeroU64;
    use std::{io::Cursor, ops::Deref};

    fn event(timestamp_ns: u64) -> PresentationTransactionEvent {
        PresentationTransactionEvent::ContentObserved {
            transaction_id: OutputTransactionId::new(
                NonZeroU64::new(timestamp_ns.max(1)).expect("nonzero output transaction id"),
            ),
            timestamp_ns,
        }
    }

    fn test_config(max_file_bytes: u64, max_files: usize) -> TraceWriterConfig {
        TraceWriterConfig {
            max_file_bytes,
            max_files,
            startup_timeout: Duration::from_secs(1),
            shutdown_timeout: Duration::from_secs(2),
        }
    }

    fn wait_until_active(writer: &mut PresentationTraceWriter) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            writer.poll_startup();
            if matches!(&writer.lifecycle, WriterLifecycle::Active) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "trace writer did not become active"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn blocked_hook(
        entered: Arc<std::sync::Barrier>,
        release: Arc<std::sync::Barrier>,
    ) -> Arc<dyn Fn() + Send + Sync> {
        Arc::new(move || {
            entered.wait();
            release.wait();
        })
    }

    fn set_startup_deadline(writer: &mut PresentationTraceWriter, deadline: Instant) {
        let WriterLifecycle::Initializing {
            deadline: writer_deadline,
            ..
        } = &mut writer.lifecycle
        else {
            panic!("writer is not initializing");
        };
        *writer_deadline = deadline;
    }

    struct TestTraceDirectory(PathBuf);

    impl Deref for TestTraceDirectory {
        type Target = Path;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl AsRef<Path> for TestTraceDirectory {
        fn as_ref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestTraceDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn unique_path(label: &str) -> TestTraceDirectory {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        for _ in 0..128 {
            let nonce = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "typhon-presentation-trace-{label}-{}-{timestamp}-{nonce}",
                std::process::id(),
            ));
            match fs::create_dir(&path) {
                Ok(()) => return TestTraceDirectory(path),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("failed to create trace test directory: {error}"),
            }
        }
        panic!("could not allocate a unique trace test directory");
    }

    fn report_latency(label: &str, capacity: usize, samples: &[u64]) {
        let mut ordered = samples.to_vec();
        ordered.sort_unstable();
        let percentile = |percent: usize| {
            let rank = ordered.len().saturating_mul(percent).div_ceil(100);
            ordered[rank.saturating_sub(1)]
        };
        println!(
            "benchmark={label} capacity={capacity} samples={} p50_ns={} p95_ns={} p99_ns={}",
            ordered.len(),
            percentile(50),
            percentile(95),
            percentile(99),
        );
    }

    fn report_phases(label: &str, capacity: usize, samples: &[u64]) {
        let warm_end = capacity.min(samples.len());
        report_latency(
            &format!("{label}_before_saturation"),
            capacity,
            &samples[..warm_end],
        );
        if samples.len() > warm_end {
            report_latency(
                &format!("{label}_first_overwrite"),
                capacity,
                &samples[warm_end..warm_end + 1],
            );
        }
        if samples.len() > warm_end + 1 {
            report_latency(
                &format!("{label}_after_ten_wraps"),
                capacity,
                &samples[warm_end + 1..],
            );
        }
    }

    #[test]
    fn writer_serializes_batches_off_the_producer_and_flushes_on_shutdown() {
        let root = unique_path("writer");
        let path = root.join("trace.jsonl");
        let mut ring = PresentationTransactionTraceRing::new(8);
        ring.push(event(1));
        ring.push(event(2));
        let mut writer = PresentationTraceWriter::start_with_config(
            path.clone(),
            test_config(1024 * 1024, 2),
            &ring,
        )
        .expect("writer thread starts");
        wait_until_active(&mut writer);
        writer.submit(&ring);
        assert_eq!(writer.stats.events_enqueued.load(Ordering::Relaxed), 0);
        writer.shutdown(&ring);

        let contents = fs::read_to_string(&path).expect("trace file was flushed");
        assert_eq!(contents.lines().count(), 2);
        let first = contents.find("\"sequence\":1").unwrap();
        let second = contents.find("\"sequence\":2").unwrap();
        assert!(first < second);
        assert!(contents.contains("\"transaction_id\":1"));
    }

    #[test]
    fn startup_timeout_and_shutdown_do_not_wait_for_recovery() {
        let root = unique_path("blocked-recovery");
        let path = root.join("trace.jsonl");
        let ring = PresentationTransactionTraceRing::new(8);
        let entered = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        let started_at = Instant::now();
        let mut writer = PresentationTraceWriter::start_with_hooks(
            path,
            TraceWriterConfig {
                startup_timeout: Duration::from_millis(20),
                shutdown_timeout: Duration::from_millis(30),
                ..test_config(1024 * 1024, 2)
            },
            &ring,
            TraceWriterTestHooks {
                before_recovery: Some(blocked_hook(Arc::clone(&entered), Arc::clone(&release))),
                ..TraceWriterTestHooks::default()
            },
        )
        .expect("spawning the optional writer is enough to start");
        assert!(started_at.elapsed() < Duration::from_millis(100));
        entered.wait();
        set_startup_deadline(&mut writer, Instant::now() - Duration::from_millis(1));
        writer.poll_startup();
        assert!(matches!(&writer.lifecycle, WriterLifecycle::Disabled));

        let shutdown_started = Instant::now();
        writer.shutdown(&ring);
        writer.shutdown(&ring);
        assert!(shutdown_started.elapsed() < Duration::from_millis(100));

        release.wait();
        assert!(
            writer
                .completion
                .recv_timeout(Duration::from_secs(1))
                .is_ok()
        );
    }

    #[test]
    fn readiness_queued_at_deadline_is_rejected_without_activation() {
        let root = unique_path("ready-boundary");
        let path = root.join("trace.jsonl");
        let ring = PresentationTransactionTraceRing::new(8);
        let entered = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        let mut writer = PresentationTraceWriter::start_with_hooks(
            path,
            TraceWriterConfig {
                startup_timeout: Duration::from_millis(50),
                ..test_config(1024 * 1024, 2)
            },
            &ring,
            TraceWriterTestHooks {
                after_ready_sent: Some(blocked_hook(Arc::clone(&entered), Arc::clone(&release))),
                ..TraceWriterTestHooks::default()
            },
        )
        .expect("writer thread starts");
        entered.wait();
        set_startup_deadline(&mut writer, Instant::now() - Duration::from_nanos(1));
        release.wait();
        writer.poll_startup();
        assert!(matches!(&writer.lifecycle, WriterLifecycle::Disabled));
        assert_eq!(writer.stats.startup_timeouts.load(Ordering::Relaxed), 1);
        assert!(
            writer
                .completion
                .recv_timeout(Duration::from_secs(1))
                .is_ok()
        );
        writer.shutdown(&ring);
    }

    #[test]
    fn recovery_result_after_timeout_cannot_reactivate_or_start_export() {
        let root = unique_path("late-recovery");
        let path = root.join("trace.jsonl");
        let ring = PresentationTransactionTraceRing::new(8);
        let entered = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        let mut writer = PresentationTraceWriter::start_with_hooks(
            path,
            test_config(1024 * 1024, 2),
            &ring,
            TraceWriterTestHooks {
                after_recovery: Some(blocked_hook(Arc::clone(&entered), Arc::clone(&release))),
                ..TraceWriterTestHooks::default()
            },
        )
        .expect("writer thread starts");
        entered.wait();
        set_startup_deadline(&mut writer, Instant::now() - Duration::from_millis(1));
        writer.poll_startup();
        assert!(matches!(&writer.lifecycle, WriterLifecycle::Disabled));
        release.wait();
        assert!(
            writer
                .completion
                .recv_timeout(Duration::from_secs(1))
                .is_ok()
        );
        writer.poll_startup();
        assert!(matches!(&writer.lifecycle, WriterLifecycle::Disabled));
        assert_eq!(writer.stats.startup_timeouts.load(Ordering::Relaxed), 1);
        writer.shutdown(&ring);
    }

    #[test]
    fn worker_panic_before_readiness_disables_file_export() {
        let root = unique_path("startup-panic");
        let path = root.join("trace.jsonl");
        let ring = PresentationTransactionTraceRing::new(8);
        let (entered_sender, entered_receiver) = mpsc::sync_channel(1);
        let mut writer = PresentationTraceWriter::start_with_hooks(
            path,
            test_config(1024 * 1024, 2),
            &ring,
            TraceWriterTestHooks {
                before_recovery: Some(Arc::new(move || {
                    let _ = entered_sender.send(());
                    panic!("injected trace recovery panic");
                })),
                ..TraceWriterTestHooks::default()
            },
        )
        .expect("writer thread starts before recovery");
        entered_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("worker reached recovery");
        assert!(
            writer
                .completion
                .recv_timeout(Duration::from_secs(1))
                .is_ok()
        );
        writer.poll_startup();
        assert!(matches!(&writer.lifecycle, WriterLifecycle::Disabled));
        assert_eq!(writer.stats.writer_panics.load(Ordering::Relaxed), 1);
        assert_eq!(writer.stats.startup_errors.load(Ordering::Relaxed), 1);
        writer.shutdown(&ring);
    }

    #[test]
    fn recovery_error_is_reported_as_disabled_optional_export() {
        let root = unique_path("recovery-error");
        let parent_file = root.join("parent-file");
        fs::write(&parent_file, b"the trace parent is not a directory").unwrap();
        let path = parent_file.join("trace.jsonl");
        let ring = PresentationTransactionTraceRing::new(8);
        let mut writer =
            PresentationTraceWriter::start_with_config(path, test_config(1024 * 1024, 2), &ring)
                .expect("filesystem recovery is asynchronous");
        assert!(
            writer
                .completion
                .recv_timeout(Duration::from_secs(1))
                .is_ok()
        );
        writer.poll_startup();
        assert!(matches!(&writer.lifecycle, WriterLifecycle::Disabled));
        assert_eq!(writer.stats.startup_errors.load(Ordering::Relaxed), 1);
        writer.shutdown(&ring);
    }

    #[test]
    fn shutdown_does_not_wait_for_a_writer_blocked_in_file_work() {
        let root = unique_path("blocked-write");
        let path = root.join("trace.jsonl");
        let mut ring = PresentationTransactionTraceRing::new(8);
        ring.push(event(1));
        let entered = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        let mut writer = PresentationTraceWriter::start_with_hooks(
            path,
            TraceWriterConfig {
                shutdown_timeout: Duration::from_millis(30),
                ..test_config(1024 * 1024, 2)
            },
            &ring,
            TraceWriterTestHooks {
                before_write: Some(blocked_hook(Arc::clone(&entered), Arc::clone(&release))),
                ..TraceWriterTestHooks::default()
            },
        )
        .expect("writer thread starts");
        wait_until_active(&mut writer);
        writer
            .producer
            .as_mut()
            .expect("active writer has a producer")
            .submit(&ring, true);
        entered.wait();
        let shutdown_started = Instant::now();
        writer.shutdown(&ring);
        assert!(shutdown_started.elapsed() < Duration::from_millis(100));
        assert_eq!(writer.stats.shutdown_timeouts.load(Ordering::Relaxed), 1);
        release.wait();
        assert!(
            writer
                .completion
                .recv_timeout(Duration::from_secs(1))
                .is_ok()
        );
    }

    #[test]
    fn writer_restart_resumes_persisted_sequence_and_repairs_partial_tail() {
        let root = unique_path("restart");
        let path = root.join("trace.jsonl");
        let mut ring = PresentationTransactionTraceRing::new(8);
        ring.push(event(1));
        ring.push(event(2));
        let mut first_writer = PresentationTraceWriter::start_with_config(
            path.clone(),
            test_config(1024 * 1024, 2),
            &ring,
        )
        .expect("first writer starts");
        wait_until_active(&mut first_writer);
        first_writer.submit(&ring);
        first_writer.shutdown(&ring);

        OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("trace file can be reopened")
            .write_all(b"{partial")
            .expect("simulate an interrupted final JSONL record");
        ring.push(event(3));
        let mut restarted_writer = PresentationTraceWriter::start_with_config(
            path.clone(),
            test_config(1024 * 1024, 2),
            &ring,
        )
        .expect("writer restarts and truncates the incomplete tail");
        wait_until_active(&mut restarted_writer);
        restarted_writer.submit(&ring);
        restarted_writer.shutdown(&ring);

        let contents = fs::read_to_string(&path).expect("trace file remains readable");
        let sequences = contents
            .lines()
            .map(|line| {
                let record: TraceResumeRecord =
                    serde_json::from_str(line).expect("each JSONL event is independently valid");
                record.sequence.expect("event record has a sequence")
            })
            .collect::<Vec<_>>();
        assert_eq!(sequences, [1, 2, 3]);
        assert!(!contents.contains("partial"));
    }

    #[test]
    fn paused_consumer_does_not_block_and_reports_retention_loss_on_resume() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let (reusable_sender, reusable_batches) = mpsc::sync_channel(2);
        reusable_sender
            .send(TraceBatch {
                records: Vec::with_capacity(2),
                last_sequence: None,
            })
            .unwrap();
        reusable_sender
            .send(TraceBatch {
                records: Vec::with_capacity(2),
                last_sequence: None,
            })
            .unwrap();
        let stats = Arc::new(TraceWriterStats::default());
        let mut producer =
            TraceBatchProducer::new(sender, reusable_batches, Arc::clone(&stats), None);
        let mut ring = PresentationTransactionTraceRing::new(1);

        ring.push(event(1));
        producer.submit(&ring, true);
        ring.push(event(2));
        producer.submit(&ring, false);
        for timestamp in 3..=8 {
            ring.push(event(timestamp));
            producer.submit(&ring, false);
        }
        assert_eq!(producer.last_enqueued_sequence, Some(1));
        assert_eq!(stats.consumer_stalls.load(Ordering::Relaxed), 1);

        let first_batch = receiver.try_recv().expect("first batch queued");
        reusable_sender
            .try_send(TraceBatch {
                records: Vec::with_capacity(2),
                last_sequence: None,
            })
            .expect("return a test batch buffer");
        producer.submit(&ring, false);
        assert_eq!(producer.last_enqueued_sequence, Some(3));
        let second_batch = receiver.try_recv().expect("second batch queued");
        assert!(matches!(
            second_batch.records.as_slice(),
            [TraceExportRecord::Event(first), TraceExportRecord::Event(second)]
                if first.sequence == 2 && second.sequence == 3
        ));
        drop((first_batch, second_batch));

        reusable_sender
            .try_send(TraceBatch {
                records: Vec::with_capacity(2),
                last_sequence: None,
            })
            .expect("return the second test batch buffer");
        producer.submit(&ring, false);
        let resumed = receiver.try_recv().expect("resumed batch queued");
        assert!(matches!(
            resumed.records.as_slice(),
            [
                TraceExportRecord::LostRange {
                    from_sequence: 4,
                    to_sequence: 7,
                    ..
                },
                TraceExportRecord::Event(record)
            ] if record.sequence == 8
        ));
    }

    #[test]
    fn rotation_keeps_each_file_and_total_storage_bounded() {
        let root = unique_path("rotation");
        let path = root.join("trace.jsonl");
        let mut ring = PresentationTransactionTraceRing::new(2048);
        for timestamp in 1..=1024 {
            ring.push(event(timestamp));
        }
        let mut writer = PresentationTraceWriter::start_with_config(
            path.clone(),
            test_config(64 * 1024, 2),
            &ring,
        )
        .expect("writer thread starts");
        wait_until_active(&mut writer);
        writer.shutdown(&ring);

        let mut total = 0;
        for candidate in [&path, &rotated_path(&path, 1)] {
            if let Ok(metadata) = fs::metadata(candidate) {
                assert!(metadata.len() <= 64 * 1024);
                total += metadata.len();
            }
        }
        assert!(total <= 128 * 1024);
    }

    #[test]
    fn repeated_rotation_marks_expired_history_separately_from_event_loss() {
        let root = unique_path("many-rotations");
        let path = root.join("trace.jsonl");
        let ring = PresentationTransactionTraceRing::new(8);
        let source = ring.source_identity();
        let mut file_writer = TraceFileWriter::new(
            path.clone(),
            TraceWriterConfig {
                max_file_bytes: 1024,
                max_files: 3,
                ..test_config(1024, 3)
            },
        );
        let mut output = String::with_capacity(4096);
        let attempted_events = 240u64;
        let mut expiry_markers_written = 0;
        for sequence in 1..=attempted_events {
            let record = PresentationTransactionTraceRecord {
                sequence: u128::from(sequence),
                process_id: source.0,
                source_id: source.1,
                event: event(sequence),
            };
            let batch = TraceBatch {
                records: vec![TraceExportRecord::Event(record)],
                last_sequence: Some(u128::from(sequence)),
            };
            file_writer
                .write_batch(&batch, &mut output)
                .expect("single-event batch fits in a trace file");
            expiry_markers_written += u64::from(output.starts_with(RETENTION_EXPIRED_JSONL));
        }
        file_writer.sync_data().expect("trace files sync");
        assert!(
            expiry_markers_written > 10,
            "test did not cross ten rotations"
        );

        let mut retained_event_sequences = Vec::new();
        let mut retained_expiry_markers = 0;
        let mut retained_loss_records = 0;
        let mut total_bytes = 0;
        let mut file_count = 0;
        for index in (0..3).rev() {
            let candidate = if index == 0 {
                path.clone()
            } else {
                rotated_path(&path, index)
            };
            let Ok(contents) = fs::read_to_string(&candidate) else {
                continue;
            };
            let bytes = fs::metadata(&candidate).unwrap().len();
            assert!(bytes <= 1024, "retained file exceeded its byte limit");
            total_bytes += bytes;
            file_count += 1;
            for line in contents.lines() {
                let record: TraceResumeRecord =
                    serde_json::from_str(line).expect("every retained line is JSONL");
                if record.kind.as_deref() == Some("retention_expired") {
                    assert_eq!(record.scope.as_deref(), Some("trace_file_set"));
                    assert!(record.process_id.is_none());
                    assert!(record.source_id.is_none());
                    assert!(record.sequence.is_none());
                    retained_expiry_markers += 1;
                } else if let Some(sequence) = record.sequence {
                    assert_eq!(record.process_id, Some(source.0));
                    assert_eq!(record.source_id, Some(source.1));
                    retained_event_sequences.push(sequence);
                } else {
                    assert_eq!(record.kind.as_deref(), Some("loss"));
                    retained_loss_records += 1;
                }
            }
        }

        assert!(file_count <= 3);
        assert!(total_bytes <= 3 * 1024);
        assert!(retained_expiry_markers > 0);
        assert_eq!(retained_loss_records, 0);
        assert!(!retained_event_sequences.is_empty());
        assert!(
            retained_event_sequences[0] > 1,
            "older event history expired"
        );
        assert!(retained_event_sequences.len() < attempted_events as usize);
        for pair in retained_event_sequences.windows(2) {
            assert_eq!(
                pair[1],
                pair[0] + 1,
                "retained events have a gap or duplicate"
            );
        }

        drop(file_writer);
        OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("active trace file reopens")
            .write_all(b"{partial")
            .expect("simulate an interrupted final line");
        let restarted = TraceFileWriter::new(path, test_config(1024, 3));
        assert_eq!(
            restarted.resume_sequence(source).unwrap(),
            Some(u128::from(attempted_events)),
            "restart skips storage metadata and repairs the partial tail"
        );
    }

    #[test]
    fn reopening_an_oversized_active_file_expires_it_before_writing() {
        let root = unique_path("oversized-active");
        let path = root.join("trace.jsonl");
        fs::write(&path, vec![b'x'; 1025]).unwrap();
        fs::write(rotated_path(&path, 1), vec![b'y'; 900]).unwrap();
        let ring = PresentationTransactionTraceRing::new(8);
        let source = ring.source_identity();
        let batch = TraceBatch {
            records: vec![TraceExportRecord::Event(
                PresentationTransactionTraceRecord {
                    sequence: 1,
                    process_id: source.0,
                    source_id: source.1,
                    event: event(1),
                },
            )],
            last_sequence: Some(1),
        };
        let mut file_writer = TraceFileWriter::new(path.clone(), test_config(1024, 2));
        let mut output = String::with_capacity(1024);
        file_writer
            .write_batch(&batch, &mut output)
            .expect("writer replaces an oversized active file");
        file_writer.sync_data().unwrap();

        let active = fs::read_to_string(&path).unwrap();
        let marker = active.lines().next().unwrap();
        let marker: TraceResumeRecord = serde_json::from_str(marker).unwrap();
        assert_eq!(marker.kind.as_deref(), Some("retention_expired"));
        assert_eq!(marker.scope.as_deref(), Some("trace_file_set"));
        assert!(active.len() <= 1024);
        assert!(fs::metadata(rotated_path(&path, 1)).unwrap().len() <= 1024);
    }

    #[test]
    fn overloaded_consumer_keeps_transport_loss_distinct_from_rotated_history() {
        let root = unique_path("overload-and-rotation");
        let path = root.join("trace.jsonl");
        let mut ring = PresentationTransactionTraceRing::new(16);
        let entered = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        let block_next_write = Arc::new(AtomicBool::new(false));
        let mut writer = PresentationTraceWriter::start_with_hooks(
            path.clone(),
            test_config(8192, 3),
            &ring,
            TraceWriterTestHooks {
                before_write: Some({
                    let entered = Arc::clone(&entered);
                    let release = Arc::clone(&release);
                    let block_next_write = Arc::clone(&block_next_write);
                    Arc::new(move || {
                        if block_next_write.swap(false, Ordering::AcqRel) {
                            entered.wait();
                            release.wait();
                        }
                    })
                }),
                ..TraceWriterTestHooks::default()
            },
        )
        .expect("writer starts");
        wait_until_active(&mut writer);

        for sequence in 1..=700 {
            ring.push(event(sequence));
            writer.producer.as_mut().unwrap().submit(&ring, true);
            let deadline = Instant::now() + Duration::from_secs(2);
            while writer.stats.events_written.load(Ordering::Relaxed) < sequence {
                assert!(
                    Instant::now() < deadline,
                    "writer stopped before sequence {sequence}"
                );
                thread::yield_now();
            }
        }

        block_next_write.store(true, Ordering::Release);
        ring.push(event(701));
        writer.producer.as_mut().unwrap().submit(&ring, true);
        entered.wait();
        for sequence in 702..=1000 {
            ring.push(event(sequence));
            writer.producer.as_mut().unwrap().submit(&ring, true);
        }
        assert!(writer.stats.consumer_stalls.load(Ordering::Relaxed) > 0);
        release.wait();

        let drained_deadline = Instant::now() + Duration::from_secs(2);
        while writer.stats.events_written.load(Ordering::Relaxed) < 705 {
            assert!(
                Instant::now() < drained_deadline,
                "queued batches did not drain"
            );
            thread::yield_now();
        }
        writer.producer.as_mut().unwrap().submit(&ring, true);
        writer.shutdown(&ring);
        assert_eq!(writer.stats.events_written.load(Ordering::Relaxed), 721);

        let source = ring.source_identity();
        let mut next_sequence = None;
        let mut saw_loss = false;
        let mut saw_expired_history = false;
        let mut retained_files = 0;
        let mut total_bytes = 0;
        for index in (0..3).rev() {
            let candidate = if index == 0 {
                path.clone()
            } else {
                rotated_path(&path, index)
            };
            let Ok(contents) = fs::read_to_string(&candidate) else {
                continue;
            };
            let bytes = fs::metadata(&candidate).unwrap().len();
            assert!(bytes <= 8192);
            retained_files += 1;
            total_bytes += bytes;
            for line in contents.lines() {
                let record: TraceResumeRecord = serde_json::from_str(line).unwrap();
                if record.kind.as_deref() == Some("retention_expired") {
                    assert_eq!(record.scope.as_deref(), Some("trace_file_set"));
                    assert!(record.process_id.is_none());
                    assert!(record.source_id.is_none());
                    assert!(record.sequence.is_none());
                    saw_expired_history = true;
                } else {
                    assert_eq!(record.process_id, Some(source.0));
                    assert_eq!(record.source_id, Some(source.1));
                    if let Some(sequence) = record.sequence {
                        if let Some(expected) = next_sequence {
                            assert_eq!(sequence, expected, "retained events are ordered");
                        }
                        next_sequence = Some(sequence + 1);
                    } else {
                        assert_eq!(record.kind.as_deref(), Some("loss"));
                        assert_eq!(record.reason.as_deref(), Some("retention_overwrite"));
                        let from = record.from_sequence.unwrap();
                        let to = record.to_sequence.unwrap();
                        if let Some(expected) = next_sequence {
                            assert_eq!(from, expected, "transport loss follows prior events");
                        }
                        assert!(to >= from);
                        next_sequence = Some(to + 1);
                        saw_loss = true;
                    }
                }
            }
        }
        assert!(retained_files <= 3);
        assert!(total_bytes <= 3 * 8192);
        assert!(saw_loss, "queue backpressure loss must remain explicit");
        assert!(
            saw_expired_history,
            "rotated history expiry must remain explicit"
        );
    }

    struct PartialFailureFile {
        cursor: Cursor<Vec<u8>>,
        fail_after_bytes: Option<usize>,
        current_write_bytes: usize,
        fail_set_len: bool,
    }

    impl Read for PartialFailureFile {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            self.cursor.read(bytes)
        }
    }

    impl Write for PartialFailureFile {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self
                .fail_after_bytes
                .is_some_and(|limit| self.current_write_bytes >= limit)
            {
                return Err(io::Error::from(io::ErrorKind::StorageFull));
            }
            let accepted = self.fail_after_bytes.map_or(bytes.len(), |limit| {
                bytes.len().min(limit - self.current_write_bytes)
            });
            let written = self.cursor.write(&bytes[..accepted])?;
            self.current_write_bytes += written;
            Ok(written)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Seek for PartialFailureFile {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            if matches!(position, SeekFrom::End(_)) {
                self.current_write_bytes = 0;
            }
            self.cursor.seek(position)
        }
    }

    impl TraceFileIo for PartialFailureFile {
        fn trace_len(&mut self) -> io::Result<u64> {
            Ok(self.cursor.get_ref().len() as u64)
        }

        fn trace_set_len(&mut self, len: u64) -> io::Result<()> {
            if self.fail_set_len {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected rollback failure",
                ));
            }
            self.cursor.get_mut().truncate(len as usize);
            self.cursor.set_position(len);
            Ok(())
        }
    }

    #[test]
    fn partial_write_is_rolled_back_before_retry() {
        let mut file = PartialFailureFile {
            cursor: Cursor::new(b"prior\n".to_vec()),
            fail_after_bytes: Some(3),
            current_write_bytes: 0,
            fail_set_len: false,
        };
        assert!(matches!(
            write_batch_bytes(&mut file, b"record\n"),
            Err(BatchWriteError::Retry(_))
        ));
        assert_eq!(file.cursor.get_ref(), b"prior\n");

        file.fail_after_bytes = None;
        write_batch_bytes(&mut file, b"record\n").expect("batch retry succeeds");
        assert_eq!(file.cursor.get_ref(), b"prior\nrecord\n");
    }

    #[test]
    fn failed_partial_write_rollback_is_fatal_to_only_the_trace_writer() {
        let mut file = PartialFailureFile {
            cursor: Cursor::new(b"prior\n".to_vec()),
            fail_after_bytes: Some(3),
            current_write_bytes: 0,
            fail_set_len: true,
        };
        assert!(matches!(
            write_batch_bytes(&mut file, b"record\n"),
            Err(BatchWriteError::Fatal(_))
        ));
    }

    #[test]
    fn recovery_truncation_failure_preserves_the_partial_tail() {
        let original = b"complete\n{partial".to_vec();
        let mut file = PartialFailureFile {
            cursor: Cursor::new(original.clone()),
            fail_after_bytes: None,
            current_write_bytes: 0,
            fail_set_len: true,
        };
        assert_eq!(
            last_complete_json_line(&mut file, true).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(file.cursor.get_ref(), &original);
    }

    #[test]
    fn rotation_error_preserves_current_file_and_stays_retryable() {
        let root = unique_path("rotation-error");
        let path = root.join("trace.jsonl");
        fs::write(&path, vec![b'x'; 1023]).unwrap();
        fs::create_dir(rotated_path(&path, 1)).unwrap();
        let mut file_writer = TraceFileWriter::new(path.clone(), test_config(1024, 2));
        let ring = PresentationTransactionTraceRing::new(8);
        let source = ring.source_identity();
        let batch = TraceBatch {
            records: vec![TraceExportRecord::Event(
                PresentationTransactionTraceRecord {
                    sequence: 1,
                    process_id: source.0,
                    source_id: source.1,
                    event: event(1),
                },
            )],
            last_sequence: Some(1),
        };
        let mut output = String::with_capacity(1024);
        assert!(matches!(
            file_writer.write_batch(&batch, &mut output),
            Err(BatchWriteError::Retry(_))
        ));
        assert_eq!(fs::metadata(&path).unwrap().len(), 1023);
        assert!(rotated_path(&path, 1).is_dir());
    }

    #[test]
    fn rotation_rename_error_preserves_the_active_history() {
        let root = unique_path("rename-error");
        let path = root.join("trace.jsonl");
        fs::write(&path, vec![b'x'; 1023]).unwrap();
        let mut file_writer = TraceFileWriter::new(path.clone(), test_config(1024, 2));
        file_writer.fail_next_rename = true;
        let ring = PresentationTransactionTraceRing::new(8);
        let source = ring.source_identity();
        let batch = TraceBatch {
            records: vec![TraceExportRecord::Event(
                PresentationTransactionTraceRecord {
                    sequence: 1,
                    process_id: source.0,
                    source_id: source.1,
                    event: event(1),
                },
            )],
            last_sequence: Some(1),
        };
        let mut output = String::with_capacity(1024);
        assert!(matches!(
            file_writer.write_batch(&batch, &mut output),
            Err(BatchWriteError::Retry(_))
        ));
        assert_eq!(fs::metadata(&path).unwrap().len(), 1023);
        assert!(!rotated_path(&path, 1).exists());
    }

    #[test]
    fn writer_io_failure_stays_diagnostic_only() {
        let root = unique_path("permission");
        let replaceable_parent = root.join("parent");
        fs::create_dir(&replaceable_parent).unwrap();
        let path = replaceable_parent.join("trace.jsonl");
        let mut ring = PresentationTransactionTraceRing::new(8);
        ring.push(event(1));
        let mut writer = PresentationTraceWriter::start_with_config(
            path,
            TraceWriterConfig {
                max_file_bytes: 64 * 1024,
                max_files: 2,
                startup_timeout: Duration::from_secs(1),
                shutdown_timeout: Duration::from_millis(100),
            },
            &ring,
        )
        .expect("writer thread starts even when the path will fail");
        wait_until_active(&mut writer);
        fs::remove_dir_all(&replaceable_parent).unwrap();
        fs::write(&replaceable_parent, b"parent is now a regular file").unwrap();
        writer.submit(&ring);
        writer.shutdown(&ring);
        assert!(writer.stats.write_errors.load(Ordering::Relaxed) > 0);
    }

    #[test]
    fn full_device_write_failure_does_not_escape_the_diagnostic_writer() {
        let path = PathBuf::from("/dev/full");
        if !path.exists() {
            return;
        }
        let mut ring = PresentationTransactionTraceRing::new(8);
        ring.push(event(1));
        let mut writer =
            PresentationTraceWriter::start_with_config(path, test_config(64 * 1024, 1), &ring)
                .expect("writer setup should not perform the failing write");
        wait_until_active(&mut writer);
        writer.submit(&ring);
        writer.shutdown(&ring);
        assert!(writer.stats.write_errors.load(Ordering::Relaxed) > 0);
        assert!(
            writer
                .stats
                .shutdown_unqueued_events
                .load(Ordering::Relaxed)
                > 0
        );
    }

    #[test]
    #[ignore = "manual opt-in trace-producer benchmark; writes temporary files"]
    fn benchmark_trace_producer_modes_at_required_capacities() {
        for capacity in [64, 4_096, 65_536] {
            let iterations = capacity * 12;
            let sample = event(1);
            let mut disabled = PresentationTransactionTraceRing::disabled(capacity);
            let mut disabled_samples = Vec::with_capacity(iterations);
            for _ in 0..iterations {
                let started_at = Instant::now();
                disabled.push(sample);
                disabled_samples.push(started_at.elapsed().as_nanos() as u64);
            }
            report_phases("disabled_push", capacity, &disabled_samples);

            let mut retained = PresentationTransactionTraceRing::new(capacity);
            let mut retained_samples = Vec::with_capacity(iterations);
            for _ in 0..iterations {
                let started_at = Instant::now();
                retained.push(sample);
                retained_samples.push(started_at.elapsed().as_nanos() as u64);
            }
            report_phases("retention_push", capacity, &retained_samples);
            let mut first_overwrite = PresentationTransactionTraceRing::new(capacity);
            for _ in 0..=capacity {
                first_overwrite.push(sample);
            }
            let mut first_overwrite_export_samples = Vec::with_capacity(5);
            let mut serialized_bytes = 0;
            for _ in 0..5 {
                let started_at = Instant::now();
                serialized_bytes = first_overwrite.export_jsonl().len();
                first_overwrite_export_samples.push(started_at.elapsed().as_nanos() as u64);
            }
            report_latency(
                "legacy_full_history_replace_at_first_overwrite",
                capacity,
                &first_overwrite_export_samples,
            );
            let mut legacy_after_wrap_samples = Vec::with_capacity(5);
            for _ in 0..5 {
                let started_at = Instant::now();
                serialized_bytes = retained.export_jsonl().len();
                legacy_after_wrap_samples.push(started_at.elapsed().as_nanos() as u64);
            }
            report_latency(
                "legacy_full_history_replace_after_ten_wraps",
                capacity,
                &legacy_after_wrap_samples,
            );
            println!(
                "benchmark=legacy_full_history_replace_bytes capacity={capacity} bytes={serialized_bytes}"
            );
            println!(
                "benchmark=retention_memory capacity={capacity} event_slots={} record_bytes={} writer_batch_pool_bytes={}",
                retained.retained_event_capacity(),
                retained.retained_event_capacity() * std::mem::size_of::<
                    crate::native_output::presentation::trace::PresentationTransactionTraceRecord,
                >(),
                TRACE_BATCH_POOL_CAPACITY
                    * (TRACE_BATCH_EVENT_CAPACITY + 1)
                    * std::mem::size_of::<TraceExportRecord>(),
            );

            let root = unique_path(&format!("bench-{capacity}"));
            let path = root.join("trace.jsonl");
            let mut streaming_ring = PresentationTransactionTraceRing::new(capacity);
            let mut writer = PresentationTraceWriter::start_with_config(
                path.clone(),
                TraceWriterConfig {
                    max_file_bytes: 16 * 1024 * 1024,
                    max_files: 4,
                    startup_timeout: Duration::from_secs(1),
                    shutdown_timeout: Duration::from_secs(10),
                },
                &streaming_ring,
            )
            .expect("streaming writer starts");
            wait_until_active(&mut writer);
            let mut streaming_samples = Vec::with_capacity(iterations);
            for _ in 0..iterations {
                let started_at = Instant::now();
                streaming_ring.push(sample);
                writer.submit(&streaming_ring);
                streaming_samples.push(started_at.elapsed().as_nanos() as u64);
            }
            writer.shutdown(&streaming_ring);
            report_phases("streaming_push_and_handoff", capacity, &streaming_samples);
            let (audited_events, loss_range_records, unpersisted_events) = audit_trace_output(
                &path,
                4,
                streaming_ring.source_identity(),
                iterations as u64,
            );
            assert_eq!(
                audited_events,
                writer.stats.events_written.load(Ordering::Relaxed)
            );
            assert_eq!(
                loss_range_records,
                writer
                    .stats
                    .records_written
                    .load(Ordering::Relaxed)
                    .saturating_sub(audited_events)
            );
            let retained_file_bytes = (0..4)
                .filter_map(|index| {
                    let candidate = if index == 0 {
                        path.clone()
                    } else {
                        rotated_path(&path, index)
                    };
                    fs::metadata(candidate).ok().map(|metadata| metadata.len())
                })
                .sum::<u64>();
            println!(
                "benchmark=streaming_results capacity={capacity} attempted={} enqueued={} written={} batches={} records={} loss_range_records={} unpersisted_events={} bytes={} consumer_stalls={} retention_drops={} file_bytes={} file_bytes_max={}",
                iterations,
                writer.stats.events_enqueued.load(Ordering::Relaxed),
                writer.stats.events_written.load(Ordering::Relaxed),
                writer.stats.batches_written.load(Ordering::Relaxed),
                writer.stats.records_written.load(Ordering::Relaxed),
                loss_range_records,
                unpersisted_events,
                writer.stats.bytes_written.load(Ordering::Relaxed),
                writer.stats.consumer_stalls.load(Ordering::Relaxed),
                streaming_ring.dropped(),
                retained_file_bytes,
                4 * 16 * 1024 * 1024,
            );
        }
    }
}
