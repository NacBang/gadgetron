use std::collections::BTreeSet;
use std::sync::Mutex;

use gadgetron_core::error::{GadgetronError, NodeErrorKind, Result};

/// Thread-safe port allocator over a fixed range [base, base + capacity).
///
/// Range defaults to 30000–39999 (10 000 ports).
/// Ports are returned to the pool on release, enabling reuse without restart.
pub struct PortPool {
    available: Mutex<BTreeSet<u16>>,
}

impl PortPool {
    /// Create a pool covering `[base, base + capacity)`.
    ///
    /// # Panics
    /// Panics if `base as u32 + capacity as u32 > 65535`.
    pub fn new(base: u16, capacity: u16) -> Self {
        assert!(
            base as u32 + capacity as u32 <= 65535,
            "port range exceeds u16 max"
        );
        let available = (base..base + capacity).collect::<BTreeSet<_>>();
        Self {
            available: Mutex::new(available),
        }
    }

    /// Allocate the lowest available port.
    ///
    /// Returns `Err(NodeErrorKind::PortAllocationFailed)` when the pool is exhausted.
    pub fn allocate(&self) -> Result<u16> {
        let mut set = self.available.lock().unwrap();
        set.pop_first().ok_or_else(|| GadgetronError::Node {
            kind: NodeErrorKind::PortAllocationFailed,
            message: "port pool exhausted".to_string(),
        })
    }

    /// Return a port to the pool.
    pub fn release(&self, port: u16) {
        self.available.lock().unwrap().insert(port);
    }

    /// Number of ports currently available.
    pub fn available_count(&self) -> usize {
        self.available.lock().unwrap().len()
    }
}

/// Default pool: 30000–39999.
impl Default for PortPool {
    fn default() -> Self {
        Self::new(30_000, 10_000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_pool_allocate_returns_port() {
        let pool = PortPool::new(9000, 10);
        let port = pool.allocate().expect("should allocate");
        assert_eq!(port, 9000); // BTreeSet gives lowest first
    }

    #[test]
    fn port_pool_release_makes_port_available() {
        let pool = PortPool::new(9000, 2); // ports 9000, 9001
        let p0 = pool.allocate().unwrap();
        let p1 = pool.allocate().unwrap();
        assert_eq!(pool.available_count(), 0);

        pool.release(p0);
        assert_eq!(pool.available_count(), 1);

        let reallocated = pool.allocate().unwrap();
        assert_eq!(reallocated, p0);
        let _ = p1; // silence unused warning
    }

    #[test]
    fn port_pool_exhaustion_returns_none() {
        let pool = PortPool::new(9000, 1);
        let _ = pool.allocate().unwrap();
        let result = pool.allocate();
        assert!(result.is_err());
    }
}
