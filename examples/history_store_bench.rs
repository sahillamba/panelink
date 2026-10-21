//! Bounded synthetic measurements for Panelink's durable history store.
//!
//! This example creates only private temporary fixtures and prints one JSON
//! object. It does not accept paths or read user history.

use panelink::control_protocol::TargetRef;
use panelink::history_rows;
use panelink::history_store::HistoryStore;
use panelink::screen_checkpoint::{self, CheckpointMetadata, ScreenBuffer};
use panelink::terminal::{ScreenSnapshot, TerminalSize};
use std::ffi::OsStr;
use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MEASURED_OPERATIONS: usize = 24;
const SEED_ROWS: usize = 1_024;
const MAX_RETAINED_ROWS: usize = 1_024;
const MAX_SEGMENT_BYTES: u64 = 8 * 1024 * 1024;
const SYNTHETIC_CAPTURE_BASE_MS: u64 = 1_800_000_000_000;
const TEMP_DIRECTORY_ATTEMPTS: usize = 64;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

struct PrivateFixture {
    path: PathBuf,
    armed: bool,
}

impl PrivateFixture {
    fn create() -> io::Result<Self> {
        let base = fs::canonicalize(std::env::temp_dir())?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for _ in 0..TEMP_DIRECTORY_ATTEMPTS {
            let sequence = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let candidate = base.join(format!(
                "panelink-history-bench-{}-{timestamp:x}-{sequence:x}",
                std::process::id()
            ));
            let mut builder = DirBuilder::new();
            builder.mode(0o700);
            match builder.create(&candidate) {
                Ok(()) => {
                    let mut fixture = Self {
                        path: candidate,
                        armed: true,
                    };
                    fs::set_permissions(&fixture.path, fs::Permissions::from_mode(0o700))?;
                    let path = fs::canonicalize(&fixture.path)?;
                    let metadata = fs::symlink_metadata(&path)?;
                    if path != fixture.path
                        || !metadata.is_dir()
                        || metadata.mode() & 0o7777 != 0o700
                    {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "unsafe benchmark fixture",
                        ));
                    }
                    fixture.path = path;
                    return Ok(fixture);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique benchmark fixture",
        ))
    }

    fn case_directory(&self, name: &str) -> io::Result<PathBuf> {
        let candidate = self.path.join(name);
        let mut builder = DirBuilder::new();
        builder.mode(0o700);
        builder.create(&candidate)?;
        fs::set_permissions(&candidate, fs::Permissions::from_mode(0o700))?;
        let path = fs::canonicalize(&candidate)?;
        let metadata = fs::symlink_metadata(&path)?;
        if path != candidate || metadata.mode() & 0o7777 != 0o700 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "unsafe benchmark case directory",
            ));
        }
        Ok(path)
    }

    fn cleanup(mut self) -> io::Result<()> {
        fs::remove_dir_all(&self.path)?;
        self.armed = false;
        Ok(())
    }
}

impl Drop for PrivateFixture {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

struct Latencies {
    samples_us: Vec<u128>,
    total_ns: u128,
    p50_us: u128,
    p95_us: u128,
    max_us: u128,
    total_us: u128,
}

struct PreparedRows {
    seed: Vec<Vec<u8>>,
    batches: Vec<Vec<Vec<u8>>>,
}

impl Latencies {
    fn from_durations(durations: &[Duration]) -> io::Result<Self> {
        if durations.len() < 16 {
            return Err(invalid_benchmark());
        }
        let total_ns = durations.iter().map(Duration::as_nanos).sum::<u128>();
        if total_ns == 0 {
            return Err(invalid_benchmark());
        }
        let samples_us: Vec<_> = durations.iter().map(Duration::as_micros).collect();
        let mut sorted = samples_us.clone();
        sorted.sort_unstable();
        let p50_us = nearest_rank(&sorted, 50);
        let p95_us = nearest_rank(&sorted, 95);
        let max_us = sorted.last().copied().ok_or_else(invalid_benchmark)?;
        Ok(Self {
            samples_us,
            total_ns,
            p50_us,
            p95_us,
            max_us,
            total_us: total_ns / 1_000,
        })
    }
}

struct HistoryResult {
    name: &'static str,
    width_columns: u16,
    batch_rows: usize,
    seed_rows: usize,
    latencies: Latencies,
    input_rows: usize,
    input_record_bytes: u128,
    rewritten_storage_bytes: u128,
    final_segment_bytes: u64,
    final_retained_rows: usize,
    final_first_ordinal: u64,
    final_last_ordinal: u64,
}

struct CheckpointResult {
    name: &'static str,
    rows: u16,
    columns: u16,
    latencies: Latencies,
    rewritten_storage_bytes: u128,
    final_checkpoint_bytes: u64,
}

fn invalid_benchmark() -> io::Error {
    io::Error::other("history benchmark invariant failed")
}

fn nearest_rank(sorted_samples: &[u128], percentile: usize) -> u128 {
    let rank = percentile * sorted_samples.len();
    let index = rank.div_ceil(100).saturating_sub(1);
    sorted_samples[index.min(sorted_samples.len() - 1)]
}

fn target(case_id: u64) -> TargetRef {
    TargetRef {
        generation: 100 + case_id,
        workspace: 200 + case_id,
        pane: 300 + case_id,
        session: 400 + case_id,
        occupant_revision: 1,
    }
}

fn feed_synthetic_line(parser: &mut vt100::Parser, case_id: u64, line: &mut u64) {
    let text = format!("case{case_id:02}-row{line:06}\r\n");
    parser.process(text.as_bytes());
    *line += 1;
}

fn collect_history_records(
    parser: &vt100::Parser,
    target: TargetRef,
    cursor: vt100::HistoryCursor,
) -> io::Result<Vec<Vec<u8>>> {
    parser
        .screen()
        .history_after(cursor)
        .map_err(|_| invalid_benchmark())?
        .map(|row| history_rows::encode(target, 1, row, SYNTHETIC_CAPTURE_BASE_MS + row.ordinal))
        .collect()
}

fn validate_prepared_records(
    records: &[Vec<u8>],
    target: TargetRef,
    width_columns: u16,
    first_ordinal: u64,
) -> io::Result<()> {
    for (index, record) in records.iter().enumerate() {
        let row = history_rows::decode(record)?;
        if row.target != target
            || row.epoch != 1
            || row.ordinal != first_ordinal + index as u64
            || row.width != width_columns
        {
            return Err(invalid_benchmark());
        }
    }
    Ok(())
}

fn prepare_row_batches(
    target: TargetRef,
    case_id: u64,
    width_columns: u16,
    seed_rows: usize,
    batch_rows: usize,
) -> io::Result<PreparedRows> {
    let measured_rows = batch_rows
        .checked_mul(MEASURED_OPERATIONS)
        .ok_or_else(invalid_benchmark)?;
    let total_rows = seed_rows
        .checked_add(measured_rows)
        .ok_or_else(invalid_benchmark)?;
    let mut parser = vt100::Parser::new(2, width_columns, MAX_RETAINED_ROWS);
    let mut cursor = parser
        .screen()
        .history_cursor()
        .map_err(|_| invalid_benchmark())?;
    let mut line = 0;

    // A 2-row screen moves one prior line into normal history for each line
    // after the first. Drain at most 1,024 rows before feeding the next chunk,
    // so both engine history and store retention stay bounded. All output and
    // record encoding happens before the storage timing intervals.
    for _ in 0..=seed_rows {
        feed_synthetic_line(&mut parser, case_id, &mut line);
    }
    let seed = collect_history_records(&parser, target, cursor)?;
    if seed.len() != seed_rows {
        return Err(invalid_benchmark());
    }
    validate_prepared_records(&seed, target, width_columns, 1)?;
    cursor = parser
        .screen()
        .history_cursor()
        .map_err(|_| invalid_benchmark())?;

    let mut batches = Vec::with_capacity(MEASURED_OPERATIONS);
    for operation in 0..MEASURED_OPERATIONS {
        for _ in 0..batch_rows {
            feed_synthetic_line(&mut parser, case_id, &mut line);
        }
        let batch = collect_history_records(&parser, target, cursor)?;
        let first_ordinal = (seed_rows + operation * batch_rows + 1) as u64;
        if batch.len() != batch_rows {
            return Err(invalid_benchmark());
        }
        validate_prepared_records(&batch, target, width_columns, first_ordinal)?;
        cursor = parser
            .screen()
            .history_cursor()
            .map_err(|_| invalid_benchmark())?;
        batches.push(batch);
    }
    if batches.len() != MEASURED_OPERATIONS
        || cursor.ordinal != total_rows as u64
        || line != total_rows as u64 + 1
    {
        return Err(invalid_benchmark());
    }
    Ok(PreparedRows { seed, batches })
}

fn single_file_size(directory: &Path, extension: &str) -> io::Result<u64> {
    let mut found = None;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_file() && entry.path().extension() == Some(OsStr::new(extension)) {
            if found.is_some() {
                return Err(invalid_benchmark());
            }
            found = Some(entry.metadata()?.len());
        }
    }
    found.ok_or_else(invalid_benchmark)
}

fn run_history_case(
    fixture: &PrivateFixture,
    name: &'static str,
    case_id: u64,
    width_columns: u16,
    batch_rows: usize,
    require_byte_eviction: bool,
) -> io::Result<HistoryResult> {
    let target = target(case_id);
    let PreparedRows { seed, batches } =
        prepare_row_batches(target, case_id, width_columns, SEED_ROWS, batch_rows)?;
    let directory = fixture.case_directory(name)?;
    let store = HistoryStore::open(&directory)?;
    store.begin_history(target, 1, false)?;
    store.append_rows(target, &seed, false)?;

    let mut durations = Vec::with_capacity(MEASURED_OPERATIONS);
    let mut input_record_bytes = 0_u128;
    let mut rewritten_storage_bytes = 0_u128;
    for batch in &batches {
        input_record_bytes += batch
            .iter()
            .map(|record| record.len() as u128)
            .sum::<u128>();
        let start = Instant::now();
        store.append_rows(target, batch, false)?;
        durations.push(start.elapsed());
        // The append atomically rewrites the complete segment. Read only its
        // metadata outside the timed interval to count the bytes it replaced.
        rewritten_storage_bytes += u128::from(single_file_size(&directory, "rows")?);
    }
    let latencies = Latencies::from_durations(&durations)?;
    let final_segment_bytes = single_file_size(&directory, "rows")?;
    let saved = store.load_rows(target)?.ok_or_else(invalid_benchmark)?;
    if final_segment_bytes > MAX_SEGMENT_BYTES
        || saved.records.is_empty()
        || saved.records.len() > MAX_RETAINED_ROWS
        || (require_byte_eviction && saved.records.len() >= MAX_RETAINED_ROWS)
        || (!require_byte_eviction && saved.records.len() != MAX_RETAINED_ROWS)
    {
        return Err(invalid_benchmark());
    }
    let decoded = saved
        .records
        .iter()
        .map(|record| history_rows::decode(record))
        .collect::<io::Result<Vec<_>>>()?;
    for pair in decoded.windows(2) {
        if pair[1].ordinal != pair[0].ordinal + 1 {
            return Err(invalid_benchmark());
        }
    }
    let first = decoded.first().ok_or_else(invalid_benchmark)?;
    let last = decoded.last().ok_or_else(invalid_benchmark)?;
    if first.target != target
        || last.target != target
        || first.width != width_columns
        || last.width != width_columns
        || last.ordinal != (SEED_ROWS + batch_rows * MEASURED_OPERATIONS) as u64
        || (require_byte_eviction && !saved.gap)
    {
        return Err(invalid_benchmark());
    }

    Ok(HistoryResult {
        name,
        width_columns,
        batch_rows,
        seed_rows: SEED_ROWS,
        latencies,
        input_rows: batch_rows * MEASURED_OPERATIONS,
        input_record_bytes,
        rewritten_storage_bytes,
        final_segment_bytes,
        final_retained_rows: saved.records.len(),
        final_first_ordinal: first.ordinal,
        final_last_ordinal: last.ordinal,
    })
}

fn run_checkpoint_case(fixture: &PrivateFixture) -> io::Result<CheckpointResult> {
    let case_name = "checkpoint_24x80_blank";
    let directory = fixture.case_directory(case_name)?;
    let store = HistoryStore::open(&directory)?;
    let target = target(4);
    let snapshot = ScreenSnapshot::try_new(TerminalSize::new(24, 80).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidInput, "invalid checkpoint dimensions")
    })?)
    .map_err(|_| invalid_benchmark())?;
    let mut durations = Vec::with_capacity(MEASURED_OPERATIONS);
    let mut rewritten_storage_bytes = 0_u128;
    for sequence in 1..=MEASURED_OPERATIONS as u64 {
        let start = Instant::now();
        let metadata = CheckpointMetadata::new(
            target,
            sequence,
            SYNTHETIC_CAPTURE_BASE_MS + sequence,
            sequence,
        )
        .map_err(|_| invalid_benchmark())?;
        let encoded =
            screen_checkpoint::encode(&snapshot, &metadata).map_err(|_| invalid_benchmark())?;
        store.save(target, ScreenBuffer::Normal, &encoded)?;
        durations.push(start.elapsed());
        rewritten_storage_bytes += u128::from(single_file_size(&directory, "screen")?);
    }
    let latencies = Latencies::from_durations(&durations)?;
    let final_checkpoint_bytes = single_file_size(&directory, "screen")?;
    let checkpoint = store
        .load(target, ScreenBuffer::Normal)?
        .ok_or_else(invalid_benchmark)?;
    let decoded = screen_checkpoint::decode(&checkpoint).map_err(|_| invalid_benchmark())?;
    if decoded.metadata().output_sequence() != MEASURED_OPERATIONS as u64
        || decoded.metadata().screen_revision() != MEASURED_OPERATIONS as u64
    {
        return Err(invalid_benchmark());
    }
    Ok(CheckpointResult {
        name: case_name,
        rows: 24,
        columns: 80,
        latencies,
        rewritten_storage_bytes,
        final_checkpoint_bytes,
    })
}

fn samples_json(samples: &[u128]) -> String {
    samples
        .iter()
        .map(u128::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn history_json(result: &HistoryResult) -> String {
    let rows_per_second =
        result.input_rows as f64 * 1_000_000_000.0 / result.latencies.total_ns as f64;
    format!(
        "{{\"case\":\"{}\",\"kind\":\"normal_history_append\",\
         \"row_width_columns\":{},\"input_rows_per_operation\":{},\
         \"measured_operations\":{},\"seed_rows_outside_timing\":{},\
         \"vt100_preparation_ring_rows\":{},\
         \"elapsed_us\":{{\"samples\":[{}],\"p50\":{},\"p95\":{},\
         \"max\":{},\"total\":{}}},\"rows_per_second_ingress\":{:.3},\
         \"new_input_rows\":{},\"new_input_record_bytes\":{},\
         \"rewritten_storage_bytes_total\":{},\"final_segment_bytes\":{},\
         \"final_retained_rows\":{},\"final_first_ordinal\":{},\
         \"final_last_ordinal\":{},\"timed_scope\":\"HistoryStore append_rows, including existing-segment read and validation, complete segment encoding, temp-file write and sync, F_FULLFSYNC, rename, and directory sync; VT100 row generation and input record encoding excluded\"}}",
        result.name,
        result.width_columns,
        result.batch_rows,
        MEASURED_OPERATIONS,
        result.seed_rows,
        MAX_RETAINED_ROWS,
        samples_json(&result.latencies.samples_us),
        result.latencies.p50_us,
        result.latencies.p95_us,
        result.latencies.max_us,
        result.latencies.total_us,
        rows_per_second,
        result.input_rows,
        result.input_record_bytes,
        result.rewritten_storage_bytes,
        result.final_segment_bytes,
        result.final_retained_rows,
        result.final_first_ordinal,
        result.final_last_ordinal,
    )
}

fn checkpoint_json(result: &CheckpointResult) -> String {
    let operations_per_second =
        MEASURED_OPERATIONS as f64 * 1_000_000_000.0 / result.latencies.total_ns as f64;
    format!(
        "{{\"case\":\"{}\",\"kind\":\"screen_checkpoint_save\",\
         \"screen_rows\":{},\"screen_columns\":{},\
         \"screen_snapshot_style\":\"default_blank_cells\",\
         \"measured_operations\":{},\"elapsed_us\":{{\"samples\":[{}],\
         \"p50\":{},\"p95\":{},\"max\":{},\"total\":{}}},\
         \"operations_per_second\":{:.3},\"final_checkpoint_bytes\":{},\
         \"rewritten_storage_bytes_total\":{},\
         \"timed_scope\":\"ScreenSnapshot metadata construction, screen_checkpoint encode, HistoryStore save, checkpoint read and validation, temp-file write and sync, F_FULLFSYNC, rename, and directory sync\"}}",
        result.name,
        result.rows,
        result.columns,
        MEASURED_OPERATIONS,
        samples_json(&result.latencies.samples_us),
        result.latencies.p50_us,
        result.latencies.p95_us,
        result.latencies.max_us,
        result.latencies.total_us,
        operations_per_second,
        result.final_checkpoint_bytes,
        result.rewritten_storage_bytes,
    )
}

fn report_json(history: &[HistoryResult], checkpoint: &CheckpointResult) -> String {
    let cases = history
        .iter()
        .map(history_json)
        .chain(std::iter::once(checkpoint_json(checkpoint)))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"schema_version\":1,\"os\":\"{}\",\"architecture\":\"{}\",\
         \"measurement_type\":\"single_machine_synthetic_history_storage_observation\",\
         \"history_limits\":{{\"retained_rows\":{},\"segment_bytes\":{}}},\
         \"limitations\":[\"not_a_ten_agent_workload\",\
         \"not_a_ui_latency_proof\",\"not_a_release_budget_pass\",\
         \"hardware_representativeness_unverified\",\
         \"operating_system_cache_and_storage_state_not_controlled\",\
         \"history_row_record_preparation_excluded_from_append_timing\",\
         \"complete_bounded_segment_rewrite_and_durability_syncs_included\"],\
         \"cases\":[{}]}}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        MAX_RETAINED_ROWS,
        MAX_SEGMENT_BYTES,
        cases,
    )
}

fn run() -> io::Result<()> {
    let fixture = PrivateFixture::create()?;
    let history = vec![
        run_history_case(&fixture, "normal_80_batch_1", 1, 80, 1, false)?,
        run_history_case(&fixture, "normal_80_batch_32", 2, 80, 32, false)?,
        run_history_case(
            &fixture,
            "wide_240_batch_32_byte_eviction",
            3,
            240,
            32,
            true,
        )?,
    ];
    let checkpoint = run_checkpoint_case(&fixture)?;
    fixture.cleanup()?;
    println!("{}", report_json(&history, &checkpoint));
    Ok(())
}

fn main() {
    if run().is_err() {
        eprintln!("history_store_bench: benchmark failed");
        std::process::exit(1);
    }
}
