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

#[derive(Debug, Clone, Copy)]
struct TraceWriterConfig {
    max_file_bytes: u64,
    max_files: usize,
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
        let panics = self.writer_panics.load(Ordering::Relaxed);
        if write_errors == 0
            && stalls == 0
            && disconnects == 0
            && return_failures == 0
            && shutdown_unqueued == 0
            && shutdown_timeouts == 0
            && panics == 0
        {
            return;
        }
        eprintln!(
            "typhon presentation trace writer: write_errors={write_errors} retries={} consumer_stalls={stalls} worker_disconnects={disconnects} buffer_return_failures={return_failures} events_enqueued={} events_written={} batches_written={} records_written={} bytes_written={} shutdown_unqueued_events={shutdown_unqueued} shutdown_timeouts={shutdown_timeouts} panics={panics}",
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
    producer: TraceBatchProducer,
    worker: Option<JoinHandle<()>>,
    completion: Receiver<()>,
    stop_requested: Arc<AtomicBool>,
    stats: Arc<TraceWriterStats>,
    config: TraceWriterConfig,
    closed: bool,
}

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
        let worker = thread::Builder::new()
            .name("typhon-trace-writer".to_owned())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let file_writer = TraceFileWriter::new(path, config);
                    match file_writer.resume_sequence(source_identity) {
                        Ok(last_sequence) => {
                            let _ = ready_sender.send(Ok(last_sequence));
                            run_writer(
                                file_writer,
                                receiver,
                                reusable_sender,
                                worker_stats.as_ref(),
                                &worker_stop,
                            );
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

        let last_enqueued_sequence = match ready.recv() {
            Ok(Ok(sequence)) => sequence,
            Ok(Err(error)) => {
                worker.thread().unpark();
                let _ = worker.join();
                return Err(error);
            }
            Err(_) => {
                let _ = worker.join();
                return Err(io::Error::other(
                    "presentation trace writer failed during file recovery",
                ));
            }
        };

        Ok(Self {
            producer: TraceBatchProducer::new(
                sender,
                reusable_batches,
                Arc::clone(&stats),
                last_enqueued_sequence,
            ),
            worker: Some(worker),
            completion,
            stop_requested,
            stats,
            config,
            closed: false,
        })
    }

    pub(crate) fn submit(&mut self, trace: &PresentationTransactionTraceRing) {
        self.producer.submit(trace, false);
    }

    pub(crate) fn shutdown(&mut self, trace: &PresentationTransactionTraceRing) {
        if self.closed {
            return;
        }
        let deadline = Instant::now() + self.config.shutdown_timeout;
        let latest_sequence = trace.latest_sequence();
        while self.producer.pending_event_count(latest_sequence) != 0 && Instant::now() < deadline {
            self.producer.submit(trace, true);
            if self.producer.pending_event_count(latest_sequence) != 0 {
                let remaining = deadline.saturating_duration_since(Instant::now());
                self.producer
                    .wait_for_writer_progress(remaining.min(Duration::from_millis(1)));
            }
        }
        self.stats.shutdown_unqueued_events.fetch_add(
            self.producer.pending_event_count(latest_sequence),
            Ordering::Relaxed,
        );

        self.stop_requested.store(true, Ordering::Release);
        self.producer.close();
        if let Some(worker) = self.worker.as_ref() {
            worker.thread().unpark();
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        match self.completion.recv_timeout(remaining) {
            Ok(()) => {
                if let Some(worker) = self.worker.take() {
                    let _ = worker.join();
                }
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
}

impl Drop for PresentationTraceWriter {
    fn drop(&mut self) {
        if self.closed {
            return;
        }
        self.stop_requested.store(true, Ordering::Release);
        self.producer.close();
        if let Some(worker) = self.worker.as_ref() {
            worker.thread().unpark();
        }
        if self
            .completion
            .recv_timeout(self.config.shutdown_timeout)
            .is_ok()
            && let Some(worker) = self.worker.take()
        {
            let _ = worker.join();
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
) {
    let mut output = String::with_capacity(TRACE_BATCH_EVENT_CAPACITY * 384);
    let mut retry_delay = Duration::from_millis(10);
    while let Ok(mut batch) = receiver.recv() {
        let mut written = false;
        loop {
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

trait TraceFileIo: Write + Seek {
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
    config: TraceWriterConfig,
}

impl TraceFileWriter {
    fn new(path: PathBuf, config: TraceWriterConfig) -> Self {
        Self {
            path,
            file: None,
            current_bytes: 0,
            config,
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

    fn rotate_paths(&self) -> io::Result<()> {
        if self.config.max_files <= 1 {
            return remove_if_present(&self.path);
        }
        let max_rotations = self.config.max_files - 1;
        remove_if_present(&rotated_path(&self.path, max_rotations))?;
        for index in (1..max_rotations).rev() {
            let source = rotated_path(&self.path, index);
            let destination = rotated_path(&self.path, index + 1);
            if path_exists(&source) {
                remove_if_present(&destination)?;
                fs::rename(source, destination)?;
            }
        }
        if path_exists(&self.path) {
            let first_rotation = rotated_path(&self.path, 1);
            remove_if_present(&first_rotation)?;
            fs::rename(&self.path, first_rotation)?;
        }
        Ok(())
    }

    fn prune_old_rotations(&self) -> io::Result<()> {
        for index in self.config.max_files..=MAX_TRACE_FILES {
            remove_if_present(&rotated_path(&self.path, index))?;
        }
        for index in 1..self.config.max_files {
            let path = rotated_path(&self.path, index);
            if fs::metadata(&path).is_ok_and(|metadata| metadata.len() > self.config.max_file_bytes)
            {
                remove_if_present(&path)?;
            }
        }
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
    file: &mut File,
    repair_partial_tail: bool,
) -> io::Result<Option<Vec<u8>>> {
    let file_len = file.metadata()?.len();
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
        file.set_len(complete_len)?;
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

fn find_last_newline_before(file: &mut File, mut end: u64) -> io::Result<Option<u64>> {
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
        PresentationTransactionEvent, TraceExportRecord,
    };
    use crate::native_output::presentation::transaction::OutputTransactionId;
    use std::io::Cursor;
    use std::num::NonZeroU64;

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
            shutdown_timeout: Duration::from_secs(2),
        }
    }

    fn unique_path(label: &str) -> PathBuf {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        PathBuf::from("/mnt/Aether/Desktop/GitHub/Typhon-test-tmp").join(format!(
            "typhon-presentation-trace-{label}-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed),
        ))
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
        writer.submit(&ring);
        assert_eq!(writer.stats.events_enqueued.load(Ordering::Relaxed), 0);
        writer.shutdown(&ring);

        let contents = fs::read_to_string(&path).expect("trace file was flushed");
        assert_eq!(contents.lines().count(), 2);
        let first = contents.find("\"sequence\":1").unwrap();
        let second = contents.find("\"sequence\":2").unwrap();
        assert!(first < second);
        assert!(contents.contains("\"transaction_id\":1"));
        let _ = fs::remove_dir_all(root);
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
        let _ = fs::remove_dir_all(root);
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
        writer.shutdown(&ring);

        let mut total = 0;
        for candidate in [&path, &rotated_path(&path, 1)] {
            if let Ok(metadata) = fs::metadata(candidate) {
                assert!(metadata.len() <= 64 * 1024);
                total += metadata.len();
            }
        }
        assert!(total <= 128 * 1024);
        let _ = fs::remove_dir_all(root);
    }

    struct PartialFailureFile {
        cursor: Cursor<Vec<u8>>,
        fail_after_bytes: Option<usize>,
        current_write_bytes: usize,
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
    fn writer_io_failure_stays_diagnostic_only() {
        let root = unique_path("permission");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("trace.jsonl");
        let mut ring = PresentationTransactionTraceRing::new(8);
        ring.push(event(1));
        let mut writer = PresentationTraceWriter::start_with_config(
            path,
            TraceWriterConfig {
                max_file_bytes: 64 * 1024,
                max_files: 2,
                shutdown_timeout: Duration::from_millis(100),
            },
            &ring,
        )
        .expect("writer thread starts even when the path will fail");
        fs::remove_dir_all(&root).unwrap();
        fs::write(&root, b"parent is now a regular file").unwrap();
        writer.submit(&ring);
        writer.shutdown(&ring);
        assert!(writer.stats.write_errors.load(Ordering::Relaxed) > 0);
        let _ = fs::remove_file(root);
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
    #[ignore = "manual opt-in trace-producer benchmark; writes temporary files under Aether"]
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
                    shutdown_timeout: Duration::from_secs(10),
                },
                &streaming_ring,
            )
            .expect("streaming writer starts");
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
            let _ = fs::remove_dir_all(root);
        }
    }
}
