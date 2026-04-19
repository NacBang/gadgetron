//! Per-tenant token-bucket rate limiter (ISSUE 11 TASK 11.2).
//!
//! In-memory, lock-sharded via `DashMap` so contention stays
//! bounded as tenant count grows. Each tenant gets one bucket;
//! `consume()` attempts to atomically withdraw one token, returning
//! an error with the seconds until the next refill if the bucket
//! was empty.
//!
//! Behavior notes:
//! - Refill rate is `requests_per_minute / 60` tokens per second.
//! - Burst size caps the bucket capacity.
//! - Bucket lazily refills at first `consume()` call; no background
//!   task.
//! - Clock source is `Instant` so clock skew / wall-clock jumps
//!   can't double-spend tokens.

use dashmap::DashMap;
use std::time::Instant;
use uuid::Uuid;

/// Single-tenant token bucket.
#[derive(Debug)]
struct Bucket {
    /// Available tokens (fractional so tiny refill increments
    /// aren't rounded to zero).
    tokens: f64,
    /// Last refill observation. Monotonic; wall-clock safe.
    last_refill: Instant,
}

/// Per-tenant token-bucket rate limiter.
///
/// Wire-compatible with `QuotaEnforcer::check_pre` — the
/// `RateLimitedQuotaEnforcer` wrapper calls `consume()` first and
/// converts `RateLimited` errors into `GadgetronError::QuotaExceeded`
/// (today) or a future dedicated variant with structured
/// `retry_after_seconds`.
#[derive(Debug)]
pub struct TokenBucketRateLimiter {
    /// Max tokens the bucket can hold. First-request burst allowance.
    burst: u32,
    /// Refill rate per second (derived from requests-per-minute).
    refill_per_sec: f64,
    /// Per-tenant buckets. `DashMap` shards internally so concurrent
    /// tenant traffic doesn't serialize on one mutex.
    buckets: DashMap<Uuid, Bucket>,
}

/// Outcome of `consume` when the bucket is empty. The caller maps
/// this to HTTP 429 with `retry_after_seconds`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateLimitedError {
    /// Seconds (rounded up) until one more token is available.
    pub retry_after_seconds: u32,
}

impl TokenBucketRateLimiter {
    /// Build a limiter with the given requests-per-minute and burst
    /// size. `requests_per_minute == 0` disables the limiter (every
    /// `consume()` succeeds) — use this when the operator wants
    /// rate limiting off entirely without wrapping the call sites
    /// in `Option`.
    pub fn new(requests_per_minute: u32, burst: u32) -> Self {
        Self {
            burst,
            refill_per_sec: f64::from(requests_per_minute) / 60.0,
            buckets: DashMap::new(),
        }
    }

    /// `true` when `requests_per_minute == 0` (limiter is a no-op).
    pub fn is_disabled(&self) -> bool {
        self.refill_per_sec == 0.0
    }

    /// Atomically refill + try to consume one token for `tenant`.
    ///
    /// First call for a tenant starts them at `burst` tokens (full
    /// bucket) so the initial burst matches the operator's
    /// configured burst ceiling, not zero.
    pub fn consume(&self, tenant: Uuid) -> Result<(), RateLimitedError> {
        if self.is_disabled() {
            return Ok(());
        }
        let now = Instant::now();
        let mut entry = self.buckets.entry(tenant).or_insert_with(|| Bucket {
            tokens: f64::from(self.burst),
            last_refill: now,
        });
        // Refill: tokens += elapsed * rate, capped at burst.
        let elapsed_secs = now.duration_since(entry.last_refill).as_secs_f64();
        let refill = elapsed_secs * self.refill_per_sec;
        entry.tokens = (entry.tokens + refill).min(f64::from(self.burst));
        entry.last_refill = now;
        if entry.tokens >= 1.0 {
            entry.tokens -= 1.0;
            Ok(())
        } else {
            // Seconds until next full token. tokens ∈ [0, 1) here,
            // so we need (1.0 - tokens) / refill_per_sec more
            // seconds. `.ceil()` rounds up so the header hint is
            // never optimistically early.
            let wait_secs = ((1.0 - entry.tokens) / self.refill_per_sec).ceil();
            Err(RateLimitedError {
                retry_after_seconds: wait_secs as u32,
            })
        }
    }

    /// Current token count for a tenant (testing / observability).
    /// Returns `burst` for tenants that haven't been seen yet.
    #[cfg(test)]
    pub fn tokens_for(&self, tenant: Uuid) -> f64 {
        self.buckets
            .get(&tenant)
            .map(|b| b.tokens)
            .unwrap_or(f64::from(self.burst))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;
    use std::time::Duration;

    #[test]
    fn within_burst_accepts() {
        let rl = TokenBucketRateLimiter::new(60, 5);
        let t = Uuid::new_v4();
        for _ in 0..5 {
            rl.consume(t).expect("burst tokens should be available");
        }
    }

    #[test]
    fn exceeds_burst_rejects_with_retry_hint() {
        let rl = TokenBucketRateLimiter::new(60, 3);
        let t = Uuid::new_v4();
        rl.consume(t).unwrap();
        rl.consume(t).unwrap();
        rl.consume(t).unwrap();
        let err = rl
            .consume(t)
            .expect_err("4th call over burst=3 must reject");
        assert!(
            err.retry_after_seconds >= 1 && err.retry_after_seconds <= 2,
            "retry_after should reflect 1s refill at 60/min; got {}",
            err.retry_after_seconds
        );
    }

    #[test]
    fn refills_after_wait() {
        // 600 rpm = 10 tokens/sec; burst=2. Drain, wait ~150ms → >= 1 token.
        let rl = TokenBucketRateLimiter::new(600, 2);
        let t = Uuid::new_v4();
        rl.consume(t).unwrap();
        rl.consume(t).unwrap();
        assert!(rl.consume(t).is_err(), "drained bucket must reject");
        sleep(Duration::from_millis(150));
        rl.consume(t)
            .expect("should have refilled at 10 tok/sec after 150ms");
    }

    #[test]
    fn disabled_limiter_always_accepts() {
        let rl = TokenBucketRateLimiter::new(0, 1);
        let t = Uuid::new_v4();
        for _ in 0..100 {
            rl.consume(t).unwrap();
        }
    }

    #[test]
    fn per_tenant_buckets_are_independent() {
        let rl = TokenBucketRateLimiter::new(60, 2);
        let t1 = Uuid::new_v4();
        let t2 = Uuid::new_v4();
        rl.consume(t1).unwrap();
        rl.consume(t1).unwrap();
        assert!(rl.consume(t1).is_err(), "t1 drained");
        // t2's bucket is independent — still full.
        rl.consume(t2).unwrap();
        rl.consume(t2).unwrap();
    }
}
