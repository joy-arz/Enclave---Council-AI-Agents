//! Simple in-memory rate limiter using token bucket algorithm

use std::time::Instant;
use tokio::sync::Mutex;

/// Token bucket rate limiter with interior mutability
pub struct RateLimiter {
    /// Maximum tokens (requests) allowed
    max_tokens: usize,
    /// Current available tokens
    tokens: Mutex<usize>,
    /// Refill rate: tokens added per second
    refill_rate: f64,
    /// Last refill timestamp
    last_refill: Mutex<Instant>,
}

impl RateLimiter {
    /// Create a new rate limiter
    /// - `max_tokens`: Maximum burst size (requests allowed at once)
    /// - `refill_rate`: Tokens added per second
    pub fn new(max_tokens: usize, refill_rate: f64) -> Self {
        Self {
            max_tokens,
            tokens: Mutex::new(max_tokens),
            refill_rate,
            last_refill: Mutex::new(Instant::now()),
        }
    }

    /// Try to acquire a token. Returns true if allowed, false if rate limited.
    pub async fn try_acquire(&self) -> bool {
        // Refill tokens based on elapsed time
        let elapsed = {
            let last_refill = self.last_refill.lock().await;
            last_refill.elapsed().as_secs_f64()
        };

        let tokens_to_add = elapsed * self.refill_rate;

        if tokens_to_add >= 1.0 {
            let mut tokens = self.tokens.lock().await;
            let mut last_refill = self.last_refill.lock().await;
            *tokens = (*tokens).min(self.max_tokens) + tokens_to_add as usize;
            *last_refill = Instant::now();
        }

        let mut tokens = self.tokens.lock().await;
        if *tokens > 0 {
            *tokens -= 1;
            true
        } else {
            false
        }
    }

    /// Get remaining tokens (approximate)
    pub async fn remaining(&self) -> usize {
        *self.tokens.lock().await
    }
}

/// Simple per-IP rate limiter that stores limiters by IP
pub struct IpRateLimiter {
    limiters: std::sync::Arc<Mutex<std::collections::HashMap<String, RateLimiter>>>,
    max_tokens: usize,
    refill_rate: f64,
}

impl IpRateLimiter {
    pub fn new(max_tokens: usize, refill_rate: f64) -> Self {
        Self {
            limiters: std::sync::Arc::new(Mutex::new(std::collections::HashMap::new())),
            max_tokens,
            refill_rate,
        }
    }

    /// Check if request from IP is allowed
    pub async fn try_acquire(&self, ip: &str) -> bool {
        // Get or create limiter for this IP
        {
            let mut limiters = self.limiters.lock().await;
            if !limiters.contains_key(ip) {
                limiters.insert(ip.to_string(), RateLimiter::new(self.max_tokens, self.refill_rate));
            }
        }

        // Now try to acquire
        let limiters = self.limiters.lock().await;
        if let Some(limiter) = limiters.get(ip) {
            limiter.try_acquire().await
        } else {
            true
        }
    }

    /// Get remaining requests for IP
    pub async fn remaining(&self, ip: &str) -> usize {
        let limiters = self.limiters.lock().await;
        if let Some(limiter) = limiters.get(ip) {
            limiter.remaining().await
        } else {
            self.max_tokens
        }
    }
}