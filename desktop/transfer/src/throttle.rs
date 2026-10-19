//! Bandwidth limit and pause for transfers. Every upload chunk and download read takes its
//! bytes from a shared `Throttle` first: while paused nothing new is sent, and with a limit
//! set the average rate stays under it (a single chunk may go out in one burst).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

#[derive(Debug)]
pub struct Throttle {
    /// Bytes per second; 0 means no limit.
    rate: AtomicU64,
    paused: AtomicBool,
    /// When the bytes already granted will have "drained" at the current rate.
    next_free: Mutex<Option<Instant>>,
}

impl Default for Throttle {
    fn default() -> Self {
        Self::new(0)
    }
}

impl Throttle {
    pub fn new(bytes_per_second: u64) -> Self {
        Self { rate: AtomicU64::new(bytes_per_second), paused: AtomicBool::new(false), next_free: Mutex::new(None) }
    }

    /// Megabits per second, as people usually think of network speed (0 = no limit).
    pub fn from_mbps(mbps: f64) -> Self {
        Self::new(mbps_to_bytes(mbps))
    }

    pub fn set_rate(&self, bytes_per_second: u64) {
        self.rate.store(bytes_per_second, Ordering::Relaxed);
    }

    pub fn rate(&self) -> u64 {
        self.rate.load(Ordering::Relaxed)
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// Wait until `bytes` may be sent: after any pause, and at the rate limit.
    /// `cancelled` is checked while paused so a paused transfer can still be stopped.
    pub async fn take(&self, bytes: u64, cancelled: impl Fn() -> bool) {
        while self.is_paused() && !cancelled() {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let rate = self.rate();
        if rate == 0 || bytes == 0 {
            return;
        }
        let start = {
            let mut next = self.next_free.lock().await;
            let now = Instant::now();
            // Idle time isn't banked beyond the present, so a pause doesn't become a burst.
            let start = next.filter(|t| *t > now).unwrap_or(now);
            *next = Some(start + Duration::from_secs_f64(bytes as f64 / rate as f64));
            start
        };
        tokio::time::sleep_until(start).await;
        // Paused while waiting for a slot: hold the bytes until resumed.
        while self.is_paused() && !cancelled() {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

pub fn mbps_to_bytes(mbps: f64) -> u64 {
    if mbps.is_finite() && mbps > 0.0 {
        (mbps * 1_000_000.0 / 8.0) as u64
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn limits_the_average_rate() {
        let throttle = Throttle::new(1_000_000); // 1 MB/s
        let started = Instant::now();
        for _ in 0..5 {
            throttle.take(1_000_000, || false).await;
        }
        // The first chunk goes immediately; the next four wait a second each.
        let elapsed = started.elapsed().as_secs_f64();
        assert!((3.9..4.2).contains(&elapsed), "{elapsed}");
    }

    #[tokio::test(start_paused = true)]
    async fn unlimited_and_resumable_pause() {
        let throttle = std::sync::Arc::new(Throttle::default());
        let started = Instant::now();
        throttle.take(u64::MAX / 2, || false).await;
        assert!(started.elapsed() < Duration::from_millis(1));

        throttle.set_paused(true);
        let waiter = {
            let t = throttle.clone();
            tokio::spawn(async move { t.take(10, || false).await })
        };
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(!waiter.is_finished(), "paused transfers wait");
        throttle.set_paused(false);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(waiter.is_finished());

        // Cancelling releases a paused transfer.
        throttle.set_paused(true);
        throttle.take(10, || true).await;
    }

    #[test]
    fn converts_megabits() {
        assert_eq!(mbps_to_bytes(8.0), 1_000_000);
        assert_eq!(mbps_to_bytes(0.0), 0);
        assert_eq!(mbps_to_bytes(-1.0), 0);
    }
}
