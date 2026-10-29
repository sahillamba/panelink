#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Opt-in timing for durable context transactions only; not wheel latency.

use panelink::client_context::ContextFile;
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = PathBuf::from(format!(
            "/private/tmp/pl-wheel-cost-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .expect("private fixture directory");
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn percentile(sorted: &[Duration], p: usize) -> f64 {
    sorted[((sorted.len() - 1) * p).div_ceil(100)].as_secs_f64() * 1_000_000.0
}

fn run_case(file: &mut ContextFile, logical_wheels: usize, pairs_per_wheel: usize) {
    let mut reserves = Vec::with_capacity(logical_wheels * pairs_per_wheel);
    let mut acks = Vec::with_capacity(logical_wheels * pairs_per_wheel);
    let mut wheels = Vec::with_capacity(logical_wheels);
    let wall_start = Instant::now();
    for _ in 0..logical_wheels {
        let wheel_start = Instant::now();
        for _ in 0..pairs_per_wheel {
            let start = Instant::now();
            let key = file.reserve().expect("durable reserve");
            reserves.push(start.elapsed());
            let start = Instant::now();
            file.ack(key).expect("durable ack");
            acks.push(start.elapsed());
        }
        wheels.push(wheel_start.elapsed());
    }
    let wall = wall_start.elapsed();
    wheels.sort_unstable();
    reserves.sort_unstable();
    acks.sort_unstable();
    let sum_reserve: Duration = reserves.iter().copied().sum();
    let sum_ack: Duration = acks.iter().copied().sum();
    println!(
        "pairs_per_wheel={pairs_per_wheel} logical_wheels={logical_wheels} wheel_us_p50/p95/max={:.1}/{:.1}/{:.1} reserve_count={} ack_count={} reserve_us_p50/p95/max={:.1}/{:.1}/{:.1} ack_us_p50/p95/max={:.1}/{:.1}/{:.1} summed_reserve_ms={:.3} summed_ack_ms={:.3} case_wall_ms={:.3} context_only_reports_per_sec={:.2}",
        percentile(&wheels, 50), percentile(&wheels, 95), wheels.last().unwrap().as_secs_f64()*1e6,
        reserves.len(), acks.len(), percentile(&reserves, 50), percentile(&reserves, 95), reserves.last().unwrap().as_secs_f64()*1e6,
        percentile(&acks, 50), percentile(&acks, 95), acks.last().unwrap().as_secs_f64()*1e6,
        sum_reserve.as_secs_f64()*1e3, sum_ack.as_secs_f64()*1e3, wall.as_secs_f64()*1e3,
        logical_wheels as f64 / wall.as_secs_f64()
    );
}

#[test]
#[ignore = "opt-in durable context cost measurement"]
fn durable_context_wheel_transaction_cost() {
    let fixture = Fixture::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    println!("measurement=context_reserve_ack_only durable=true; each pair has two full flushes; creation excluded");
    for pairs in [3, 1] {
        for count in [1, 20, 100] {
            let mut file = ContextFile::create(
                &fixture.0.join(format!("context-{pairs}-{count}")),
                1,
                1,
                deadline,
            )
            .expect("create durable context");
            run_case(&mut file, count, pairs);
        }
    }
}
