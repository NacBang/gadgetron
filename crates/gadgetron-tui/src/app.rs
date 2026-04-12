use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

use gadgetron_core::ui::{
    ClusterHealth, GpuMetrics, ModelState, ModelStatus, RequestEntry, WsMessage,
};

use crate::ui;

/// TUI application state.
///
/// Concurrency model: `Arc<RwLock<T>>`.
/// Rationale: TUI has a single render thread; data producers (gateway handler loop) are
/// separate tasks. `RwLock` allows multiple readers and a single writer — no writer holds
/// the lock during rendering (1 Hz updates vs 10 Hz render budget).
/// `std::sync::RwLock` is used instead of `tokio::sync::RwLock` because the TUI render
/// loop is synchronous (crossterm uses `event::poll` for blocking I/O).
pub struct App {
    pub running: bool,
    /// Cluster health summary; updated at 1 Hz.
    pub health: Arc<RwLock<ClusterHealth>>,
    /// Per-node GPU metric list.
    pub gpu_metrics: Arc<RwLock<Vec<GpuMetrics>>>,
    /// Model status list.
    pub model_statuses: Arc<RwLock<Vec<ModelStatus>>>,
    /// Recent request log (max 100 entries). VecDeque for O(1) front eviction.
    pub request_log: Arc<RwLock<VecDeque<RequestEntry>>>,
    /// Update channel receiver. `None` = no data source connected (demo mode).
    update_rx: Option<broadcast::Receiver<WsMessage>>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// Standalone mode (TUI demo/dev), no data source.
    /// Populates with static demo data so the layout renders without a live cluster.
    pub fn new() -> Self {
        use chrono::Utc;

        let gpu_metrics = vec![
            GpuMetrics {
                node_id: "node-1".to_string(),
                gpu_index: 0,
                name: "NVIDIA A100 80G".to_string(),
                temperature_c: 52.0,
                vram_used_mb: 28_000,
                vram_total_mb: 40_960,
                utilization_pct: 68.0,
                power_w: 240.0,
                power_limit_w: 400.0,
                clock_mhz: 1_410,
                timestamp: Utc::now(),
            },
            GpuMetrics {
                node_id: "node-1".to_string(),
                gpu_index: 1,
                name: "NVIDIA A100 80G".to_string(),
                temperature_c: 78.0,
                vram_used_mb: 38_000,
                vram_total_mb: 40_960,
                utilization_pct: 92.0,
                power_w: 380.0,
                power_limit_w: 400.0,
                clock_mhz: 1_410,
                timestamp: Utc::now(),
            },
            GpuMetrics {
                node_id: "node-2".to_string(),
                gpu_index: 0,
                name: "NVIDIA H100 SXM".to_string(),
                temperature_c: 63.0,
                vram_used_mb: 12_000,
                vram_total_mb: 81_920,
                utilization_pct: 30.0,
                power_w: 120.0,
                power_limit_w: 700.0,
                clock_mhz: 1_830,
                timestamp: Utc::now(),
            },
        ];

        let model_statuses = vec![
            ModelStatus {
                model_id: "meta-llama/Llama-3-8B-Instruct".to_string(),
                name: "Llama 3 8B".to_string(),
                state: ModelState::Running,
                provider: "ollama".to_string(),
                node_id: Some("node-1".to_string()),
                vram_mb: Some(16_000),
                loaded_at: Some(Utc::now()),
            },
            ModelStatus {
                model_id: "mistralai/Mistral-7B-v0.3".to_string(),
                name: "Mistral 7B v0.3".to_string(),
                state: ModelState::Loading,
                provider: "vllm".to_string(),
                node_id: Some("node-2".to_string()),
                vram_mb: None,
                loaded_at: None,
            },
            ModelStatus {
                model_id: "gpt-4o".to_string(),
                name: "GPT-4o".to_string(),
                state: ModelState::Running,
                provider: "openai".to_string(),
                node_id: None,
                vram_mb: None,
                loaded_at: Some(Utc::now()),
            },
        ];

        let request_log = VecDeque::from([
            RequestEntry {
                request_id: "req-a1b2c3d4".to_string(),
                tenant_id: "tenant-1".to_string(),
                model: "llama3".to_string(),
                provider: "ollama".to_string(),
                status: 200,
                latency_ms: 312,
                prompt_tokens: 128,
                completion_tokens: 64,
                total_tokens: 192,
                timestamp: Utc::now(),
            },
            RequestEntry {
                request_id: "req-e5f6a7b8".to_string(),
                tenant_id: "tenant-2".to_string(),
                model: "gpt-4o".to_string(),
                provider: "openai".to_string(),
                status: 200,
                latency_ms: 891,
                prompt_tokens: 512,
                completion_tokens: 256,
                total_tokens: 768,
                timestamp: Utc::now(),
            },
            RequestEntry {
                request_id: "req-c9d0e1f2".to_string(),
                tenant_id: "tenant-1".to_string(),
                model: "mistral-7b".to_string(),
                provider: "vllm".to_string(),
                status: 503,
                latency_ms: 50,
                prompt_tokens: 64,
                completion_tokens: 0,
                total_tokens: 64,
                timestamp: Utc::now(),
            },
        ]);

        let health = ClusterHealth {
            total_nodes: 2,
            healthy_nodes: 2,
            total_gpus: 3,
            active_gpus: 3,
            models_loaded: 2,
            requests_per_sec: 14.3,
            error_rate_pct: 2.1,
            updated_at: Utc::now(),
        };

        Self {
            running: true,
            health: Arc::new(RwLock::new(health)),
            gpu_metrics: Arc::new(RwLock::new(gpu_metrics)),
            model_statuses: Arc::new(RwLock::new(model_statuses)),
            request_log: Arc::new(RwLock::new(request_log)),
            update_rx: None,
        }
    }

    /// Create with a broadcast channel receiver for live data.
    pub fn with_channel(rx: broadcast::Receiver<WsMessage>) -> Self {
        let mut app = Self::new();
        app.update_rx = Some(rx);
        app
    }

    /// Consume messages from the broadcast channel and update internal Arc<RwLock<T>>.
    ///
    /// Maximum `REQUEST_LOG_CAPACITY = 100` entries are kept; oldest is dropped when exceeded.
    ///
    /// Call site: `App::run()` loop, before each terminal draw, gated to 1 Hz
    /// to avoid unnecessary RwLock contention.
    pub fn drain_updates(&mut self) {
        const REQUEST_LOG_CAPACITY: usize = 100;
        let Some(rx) = &mut self.update_rx else {
            return;
        };
        loop {
            match rx.try_recv() {
                Ok(WsMessage::NodeUpdate(metrics)) => {
                    *self.gpu_metrics.write().unwrap() = metrics;
                }
                Ok(WsMessage::ModelUpdate(statuses)) => {
                    *self.model_statuses.write().unwrap() = statuses;
                }
                Ok(WsMessage::RequestLog(entry)) => {
                    let mut log = self.request_log.write().unwrap();
                    log.push_back(entry);
                    if log.len() > REQUEST_LOG_CAPACITY {
                        log.pop_front();
                    }
                }
                Ok(WsMessage::HealthUpdate(h)) => {
                    *self.health.write().unwrap() = h;
                }
                // #[non_exhaustive] requires a wildcard arm for WsMessage
                #[allow(unreachable_patterns)]
                Ok(_) => {}
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Closed) => {
                    self.running = false;
                    break;
                }
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
            }
        }
    }

    /// Run the TUI event loop.
    ///
    /// Render loop: 100ms crossterm event poll.
    /// Update gate: `drain_updates()` called at most once per second (1 Hz) to
    /// limit RwLock contention.
    pub async fn run(&mut self) -> anyhow::Result<()> {
        terminal::enable_raw_mode()?;
        let mut stdout = io::stdout();
        crossterm::execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        // Initialise so the first iteration drains immediately.
        let mut last_update = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);

        while self.running {
            // 1 Hz gate: drain only when at least 1 second has elapsed since last drain.
            if last_update.elapsed() >= Duration::from_secs(1) {
                self.drain_updates();
                last_update = Instant::now();
            }

            terminal.draw(|f| ui::draw(f, self))?;

            if event::poll(Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => self.running = false,
                        _ => {}
                    }
                }
            }
        }

        terminal::disable_raw_mode()?;
        crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        terminal.show_cursor()?;

        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use gadgetron_core::ui::{ClusterHealth, GpuMetrics, ModelState, ModelStatus, RequestEntry};
    use tokio::sync::broadcast;

    fn sample_gpu_metrics() -> GpuMetrics {
        GpuMetrics {
            node_id: "test-node".to_string(),
            gpu_index: 0,
            name: "A100".to_string(),
            temperature_c: 50.0,
            vram_used_mb: 10_000,
            vram_total_mb: 40_960,
            utilization_pct: 50.0,
            power_w: 200.0,
            power_limit_w: 400.0,
            clock_mhz: 1_410,
            timestamp: chrono::Utc::now(),
        }
    }

    fn sample_request_entry(id: &str) -> RequestEntry {
        RequestEntry {
            request_id: id.to_string(),
            tenant_id: "t1".to_string(),
            model: "llama3".to_string(),
            provider: "ollama".to_string(),
            status: 200,
            latency_ms: 100,
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            timestamp: chrono::Utc::now(),
        }
    }

    #[test]
    fn app_new_has_demo_data() {
        let app = App::new();
        assert!(app.running);
        // Demo data is pre-populated
        assert!(!app.gpu_metrics.read().unwrap().is_empty());
        assert!(!app.model_statuses.read().unwrap().is_empty());
        assert!(!app.request_log.read().unwrap().is_empty());
    }

    #[test]
    fn drain_updates_node_update_replaces_gpu_metrics() {
        let (tx, rx) = broadcast::channel::<WsMessage>(16);
        let mut app = App::with_channel(rx);

        let metrics = vec![sample_gpu_metrics()];
        tx.send(WsMessage::NodeUpdate(metrics.clone())).unwrap();
        app.drain_updates();

        let stored = app.gpu_metrics.read().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].node_id, "test-node");
    }

    #[test]
    fn drain_updates_model_update_replaces_model_statuses() {
        let (tx, rx) = broadcast::channel::<WsMessage>(16);
        let mut app = App::with_channel(rx);

        let statuses = vec![ModelStatus {
            model_id: "llama3".to_string(),
            name: "Llama 3".to_string(),
            state: ModelState::Running,
            provider: "ollama".to_string(),
            node_id: None,
            vram_mb: None,
            loaded_at: None,
        }];
        tx.send(WsMessage::ModelUpdate(statuses)).unwrap();
        app.drain_updates();

        let stored = app.model_statuses.read().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].model_id, "llama3");
    }

    #[test]
    fn drain_updates_health_update_replaces_health() {
        let (tx, rx) = broadcast::channel::<WsMessage>(16);
        let mut app = App::with_channel(rx);

        let health = ClusterHealth {
            total_nodes: 5,
            healthy_nodes: 4,
            ..ClusterHealth::default()
        };
        tx.send(WsMessage::HealthUpdate(health)).unwrap();
        app.drain_updates();

        let stored = app.health.read().unwrap();
        assert_eq!(stored.total_nodes, 5);
        assert_eq!(stored.healthy_nodes, 4);
    }

    #[test]
    fn drain_updates_request_log_overflow_keeps_100() {
        let (tx, rx) = broadcast::channel::<WsMessage>(256);
        let mut app = App::with_channel(rx);

        // Clear demo data first
        app.request_log.write().unwrap().clear();

        // Send 101 entries
        for i in 0..101u32 {
            tx.send(WsMessage::RequestLog(sample_request_entry(&format!(
                "req-{i:04}"
            ))))
            .unwrap();
        }
        app.drain_updates();

        let log = app.request_log.read().unwrap();
        assert_eq!(log.len(), 100, "log must be capped at 100 entries");
        // The first entry (req-0000) must have been evicted
        assert!(
            !log.iter().any(|e| e.request_id == "req-0000"),
            "oldest entry must be evicted"
        );
        // The last entry (req-0100) must be present
        assert!(
            log.iter().any(|e| e.request_id == "req-0100"),
            "newest entry must be retained"
        );
    }

    #[test]
    fn drain_updates_empty_channel_is_noop() {
        let (_tx, rx) = broadcast::channel::<WsMessage>(16);
        let mut app = App::with_channel(rx);
        // Should not panic or modify state
        app.drain_updates();
        assert!(app.running);
    }

    #[test]
    fn drain_updates_closed_channel_stops_app() {
        let (tx, rx) = broadcast::channel::<WsMessage>(16);
        let mut app = App::with_channel(rx);
        drop(tx); // close the channel
        app.drain_updates();
        assert!(!app.running, "closed channel must set running = false");
    }
}
