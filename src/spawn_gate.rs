//! Cooperative coordination for Panelink-owned descriptor creation and exec.
//!
//! This gate only protects application paths that explicitly acquire it. It
//! cannot serialize unrelated native or injected spawn calls.

use std::io;
use std::sync::{Mutex, MutexGuard, TryLockError};
use std::thread;
use std::time::{Duration, Instant};

const RETRY_INTERVAL: Duration = Duration::from_millis(1);

static SPAWN_GATE: Gate = Gate::new();

#[cfg(test)]
static TEST_GATE: Gate = Gate::new();

/// Acquire the process gate for a synchronous spawn or startup bind path.
///
/// A poisoned gate fails closed. Callers must keep the guard in a narrow
/// lexical scope and must not re-enter this module while holding it.
pub(crate) fn acquire() -> io::Result<MutexGuard<'static, ()>> {
    SPAWN_GATE.acquire()
}

/// Acquire the process gate without exceeding an existing absolute deadline.
///
/// This is for bounded accept/connect descriptor creation. It has no side
/// effects when the deadline expires.
pub(crate) fn acquire_until(deadline: Instant) -> io::Result<MutexGuard<'static, ()>> {
    SPAWN_GATE.acquire_until(deadline)
}

#[cfg(test)]
pub(crate) fn test_acquire_until(deadline: Instant) -> io::Result<MutexGuard<'static, ()>> {
    TEST_GATE.acquire_until(deadline)
}

#[cfg(test)]
pub(crate) fn test_try_acquire() -> io::Result<Option<MutexGuard<'static, ()>>> {
    match TEST_GATE.lock.try_lock() {
        Ok(guard) => Ok(Some(guard)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Poisoned(_)) => Err(poisoned_error()),
    }
}

struct Gate {
    lock: Mutex<()>,
}

impl Gate {
    const fn new() -> Self {
        Self {
            lock: Mutex::new(()),
        }
    }

    fn acquire(&self) -> io::Result<MutexGuard<'_, ()>> {
        self.lock.lock().map_err(|_| poisoned_error())
    }

    fn acquire_until(&self, deadline: Instant) -> io::Result<MutexGuard<'_, ()>> {
        loop {
            if Instant::now() >= deadline {
                return Err(timeout_error());
            }

            match self.lock.try_lock() {
                Ok(guard) => {
                    if Instant::now() >= deadline {
                        drop(guard);
                        return Err(timeout_error());
                    }
                    return Ok(guard);
                }
                Err(TryLockError::Poisoned(_)) => return Err(poisoned_error()),
                Err(TryLockError::WouldBlock) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(timeout_error());
                    }
                    thread::sleep(RETRY_INTERVAL.min(remaining));
                }
            }
        }
    }
}

fn poisoned_error() -> io::Error {
    io::Error::other("spawn_gate_poisoned")
}

fn timeout_error() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "spawn_gate_timeout")
}

#[cfg(test)]
mod tests {
    use super::Gate;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    #[test]
    fn concurrent_acquire_waits_for_the_current_creation_section() {
        let gate = Arc::new(Gate::new());
        let held = gate.acquire().expect("first gate acquisition");
        let (attempting_tx, attempting_rx) = mpsc::sync_channel(0);
        let (entered_tx, entered_rx) = mpsc::sync_channel(0);
        let contender_gate = Arc::clone(&gate);
        let contender = thread::spawn(move || {
            attempting_tx.send(()).expect("signal acquisition attempt");
            let _guard = contender_gate.acquire().expect("gate after release");
            entered_tx.send(()).expect("signal gate entry");
        });

        attempting_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("contender reached acquisition");
        assert!(
            entered_rx.try_recv().is_err(),
            "contender entered while held"
        );
        assert!(
            gate.lock.try_lock().is_err(),
            "held gate must exclude peers"
        );
        drop(held);
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("contender enters after release");
        contender.join().expect("contender thread");
    }

    #[test]
    fn expired_deadline_has_no_gate_side_effect() {
        let gate = Gate::new();
        let effect = std::sync::atomic::AtomicBool::new(false);
        let deadline = Instant::now()
            .checked_sub(Duration::from_millis(1))
            .expect("monotonic instant supports a past deadline");

        let result = gate.acquire_until(deadline).map(|_guard| {
            effect.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        assert_eq!(
            result.expect_err("expired deadline must fail").kind(),
            std::io::ErrorKind::TimedOut
        );
        assert!(!effect.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn held_gate_times_out_without_running_the_protected_operation() {
        let gate = Arc::new(Gate::new());
        let held = gate.acquire().expect("hold gate");
        let (done_tx, done_rx) = mpsc::sync_channel(0);
        let timed_gate = Arc::clone(&gate);
        let effect = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let timed_effect = Arc::clone(&effect);
        let waiter = thread::spawn(move || {
            let result = timed_gate.acquire_until(Instant::now() + Duration::from_millis(80));
            if let Ok(_guard) = result {
                timed_effect.store(true, std::sync::atomic::Ordering::SeqCst);
                done_tx.send(std::io::ErrorKind::Interrupted).unwrap();
            } else {
                done_tx.send(result.unwrap_err().kind()).unwrap();
            }
        });

        assert_eq!(
            done_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("bounded waiter result"),
            std::io::ErrorKind::TimedOut
        );
        assert!(!effect.load(std::sync::atomic::Ordering::SeqCst));
        drop(held);
        waiter.join().expect("deadline waiter");
        assert!(gate.acquire().is_ok(), "timeout must not poison the gate");
    }

    #[test]
    fn poisoned_gate_rejects_blocking_and_deadline_acquisition() {
        let gate = Gate::new();
        let panic = catch_unwind(AssertUnwindSafe(|| {
            let _guard = gate.acquire().expect("initial gate acquisition");
            panic!("test-only poison");
        }));
        assert!(panic.is_err());

        let blocking = gate.acquire().expect_err("poison must fail closed");
        let timed = gate
            .acquire_until(Instant::now() + Duration::from_secs(1))
            .expect_err("timed acquisition must fail closed");
        assert_eq!(blocking.to_string(), "spawn_gate_poisoned");
        assert_eq!(timed.to_string(), "spawn_gate_poisoned");
    }
}
