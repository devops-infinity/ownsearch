use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub struct Heartbeat {
    ticks: AtomicU64,
    stopped: AtomicBool,
}

impl Heartbeat {
    pub fn new() -> Self {
        Self {
            ticks: AtomicU64::new(0),
            stopped: AtomicBool::new(false),
        }
    }

    pub fn beat(&self) {
        self.ticks.fetch_add(1, Ordering::Relaxed);
    }

    pub fn ticks(&self) -> u64 {
        self.ticks.load(Ordering::Relaxed)
    }

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
}

impl Default for Heartbeat {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn is_stalled(last_change: Instant, now: Instant, stale_after: Duration) -> bool {
    now.saturating_duration_since(last_change) > stale_after
}

pub(crate) fn start(
    heartbeat: Arc<Heartbeat>,
    stale_after: Duration,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let poll = (stale_after / 3).clamp(Duration::from_secs(1), Duration::from_secs(10));
        let mut last_ticks = heartbeat.ticks();
        let mut last_change = Instant::now();
        while !heartbeat.is_stopped() {
            std::thread::sleep(poll);
            let ticks = heartbeat.ticks();
            let now = Instant::now();
            if ticks != last_ticks {
                last_ticks = ticks;
                last_change = now;
                continue;
            }
            if is_stalled(last_change, now, stale_after) {
                tracing::error!(
                    stalled_secs = stale_after.as_secs(),
                    "the process stopped making progress; exiting so the supervisor can restart it"
                );
                std::process::exit(EXIT_HANG);
            }
        }
    })
}

const EXIT_HANG: i32 = 75;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stall_is_detected_only_past_the_window() {
        let base = Instant::now();
        assert!(!is_stalled(
            base,
            base + Duration::from_secs(10),
            Duration::from_secs(30)
        ));
        assert!(is_stalled(
            base,
            base + Duration::from_secs(31),
            Duration::from_secs(30)
        ));
    }

    #[test]
    fn a_beat_moves_the_counter_and_stop_ends_the_loop() {
        let heartbeat = Heartbeat::new();
        assert_eq!(heartbeat.ticks(), 0);
        heartbeat.beat();
        assert_eq!(heartbeat.ticks(), 1);
        assert!(!heartbeat.is_stopped());
        heartbeat.stop();
        assert!(heartbeat.is_stopped());
    }
}
