//! One global token bucket shared by every connection. `0` means unlimited (no waiting).

#![allow(dead_code)]

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

/// Burst size floor keeps the bucket from oscillating at very low rates.
const MIN_CAPACITY: u64 = 16 * 1024;

/// Bucket size for a given bytes/second rate; about `rate / 4`, never below 16 KiB.
pub fn capacity_for(rate: u64) -> u64 {
    if rate == 0 {
        0
    } else {
        (rate / 4).max(MIN_CAPACITY)
    }
}

fn refill_tokens(tokens: f64, elapsed_secs: f64, rate: u64, capacity: u64) -> f64 {
    if rate == 0 {
        return 0.0;
    }
    (tokens + elapsed_secs * rate as f64).min(capacity as f64)
}

struct Inner {
    tokens: f64,
    last: Instant,
}

/// Async token bucket. Changing the rate applies immediately and wakes current waiters.
pub struct RateLimiter {
    rate: AtomicU64,
    inner: Mutex<Inner>,
    notify: Notify,
}

impl RateLimiter {
    pub fn new(rate: u64) -> RateLimiter {
        let capacity = capacity_for(rate);
        RateLimiter {
            rate: AtomicU64::new(rate),
            inner: Mutex::new(Inner {
                tokens: capacity as f64,
                last: Instant::now(),
            }),
            notify: Notify::new(),
        }
    }

    pub fn rate(&self) -> u64 {
        self.rate.load(Ordering::SeqCst)
    }

    /// Change the limit without recreating the bucket; `0` disables limiting.
    pub fn set_rate(&self, rate: u64) {
        let old = self.rate.swap(rate, Ordering::SeqCst);
        if old == rate {
            return;
        }
        {
            let mut inner = lock(&self.inner);
            let now = Instant::now();
            let elapsed = now.duration_since(inner.last).as_secs_f64();
            if old > 0 {
                inner.tokens = refill_tokens(inner.tokens, elapsed, old, capacity_for(old));
            }
            inner.tokens = inner.tokens.min(capacity_for(rate) as f64);
            inner.last = now;
        }
        self.notify.notify_waiters();
    }

    /// Wait until `bytes` may be sent. Returns immediately when the limit is disabled.
    pub async fn acquire(&self, bytes: u64) {
        if bytes == 0 {
            return;
        }
        let mut left = bytes;

        loop {
            if self.rate() == 0 {
                return;
            }

            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            let wait = {
                let mut inner = lock(&self.inner);
                let rate = self.rate.load(Ordering::SeqCst);
                if rate == 0 {
                    return;
                }
                let now = Instant::now();
                let elapsed = now.duration_since(inner.last).as_secs_f64();
                inner.last = now;

                let capacity = capacity_for(rate);
                inner.tokens = refill_tokens(inner.tokens, elapsed, rate, capacity);

                let take = left.min(capacity);
                if inner.tokens >= take as f64 {
                    inner.tokens -= take as f64;
                    left -= take;
                    None
                } else {
                    let needed = take as f64 - inner.tokens;
                    Some(Duration::from_secs_f64(needed / rate as f64))
                }
            };

            match wait {
                None if left == 0 => return,
                None => continue,
                Some(delay) => {
                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {}
                        _ = notified => {}
                    }
                }
            }
        }
    }
}

fn lock(inner: &Mutex<Inner>) -> std::sync::MutexGuard<'_, Inner> {
    inner
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn capacity_is_a_quarter_of_the_rate_with_a_floor() {
        assert_eq!(capacity_for(0), 0);
        assert_eq!(capacity_for(1), MIN_CAPACITY);
        assert_eq!(capacity_for(4 * MIN_CAPACITY), MIN_CAPACITY);
        assert_eq!(capacity_for(4_000_000), 1_000_000);
    }

    #[test]
    fn refill_is_linear_and_capped() {
        assert!((refill_tokens(0.0, 1.0, 1000, 4096) - 1000.0).abs() < 1e-9);
        assert!((refill_tokens(1000.0, 0.5, 1000, 4096) - 1500.0).abs() < 1e-9);
        assert!((refill_tokens(4000.0, 10.0, 1000, 4096) - 4096.0).abs() < 1e-9);
        assert_eq!(refill_tokens(4000.0, 10.0, 0, 4096), 0.0);
    }

    #[tokio::test]
    async fn unlimited_never_waits() {
        let limiter = RateLimiter::new(0);
        let start = Instant::now();
        limiter.acquire(100 * 1024 * 1024).await;
        assert!(start.elapsed() < Duration::from_millis(100));
    }

    #[tokio::test]
    async fn limited_blocks_for_the_deficit() {
        let limiter = RateLimiter::new(1024 * 1024);
        let start = Instant::now();
        limiter.acquire(512 * 1024).await;
        assert!(start.elapsed() >= Duration::from_millis(150));
    }

    #[tokio::test]
    async fn changing_rate_to_unlimited_wakes_waiters() {
        let limiter = Arc::new(RateLimiter::new(1));
        let waiter = {
            let limiter = Arc::clone(&limiter);
            tokio::spawn(async move { limiter.acquire(8 * 1024 * 1024).await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        limiter.set_rate(0);
        let joined = tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("waiter woke after rate change");
        assert!(joined.is_ok());
    }

    #[tokio::test]
    async fn zero_bytes_returns_immediately() {
        let limiter = RateLimiter::new(1);
        tokio::time::timeout(Duration::from_millis(100), limiter.acquire(0))
            .await
            .expect("no wait");
    }
}
