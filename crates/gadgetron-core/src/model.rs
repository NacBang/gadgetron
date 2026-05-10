use serde::{Deserialize, Serialize};
use std::time::Instant;
use uuid::Uuid;

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReason {
    UserRequested,
    NodeShutdown,
    Evicted,
    HealthCheckFailed,
    RollingUpgrade,
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureReason {
    CrashExit(i32),
    LoadTimeout,
    HealthCheckStale,
    VramAllocationFailed,
    PortAllocationFailed,
    KillTimeout,
    ExecutableNotFound,
    GpuDeviceUnavailable,
}

impl std::fmt::Display for FailureReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CrashExit(code) => write!(f, "crash_exit({code})"),
            Self::LoadTimeout => write!(f, "load_timeout"),
            Self::HealthCheckStale => write!(f, "health_check_stale"),
            Self::VramAllocationFailed => write!(f, "vram_allocation_failed"),
            Self::PortAllocationFailed => write!(f, "port_allocation_failed"),
            Self::KillTimeout => write!(f, "kill_timeout"),
            Self::ExecutableNotFound => write!(f, "executable_not_found"),
            Self::GpuDeviceUnavailable => write!(f, "gpu_device_unavailable"),
        }
    }
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum ProcessState {
    NotLoaded,
    Loading {
        started_at: Instant,
        model_id: Uuid,
    },
    Running {
        pid: u32,
        port: u16,
        loaded_at: Instant,
        model_id: Uuid,
    },
    Stopping {
        since: Instant,
        reason: StopReason,
    },
    Stopped {
        reason: StopReason,
        stopped_at: Instant,
    },
    Failed {
        reason: FailureReason,
        failed_at: Instant,
    },
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessEvent {
    LoadRequest,
    LoadComplete { pid: u32, port: u16 },
    LoadFailed,
    StopRequest { reason: StopReason },
    StopComplete,
    CrashDetected { exit_code: i32 },
    HealthCheckFailed,
}

impl ProcessState {
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Running { .. })
    }

    pub fn can_load(&self) -> bool {
        matches!(
            self,
            Self::NotLoaded | Self::Stopped { .. } | Self::Failed { .. }
        )
    }

    pub fn transition(
        self,
        event: ProcessEvent,
        model_id: Uuid,
    ) -> Result<ProcessState, InvalidTransition> {
        match (&self, &event) {
            (
                Self::NotLoaded | Self::Stopped { .. } | Self::Failed { .. },
                ProcessEvent::LoadRequest,
            ) => Ok(Self::Loading {
                started_at: Instant::now(),
                model_id,
            }),
            (Self::Loading { .. }, ProcessEvent::LoadComplete { pid, port }) => Ok(Self::Running {
                pid: *pid,
                port: *port,
                loaded_at: Instant::now(),
                model_id,
            }),
            (Self::Loading { .. }, ProcessEvent::LoadFailed) => Ok(Self::Failed {
                reason: FailureReason::LoadTimeout,
                failed_at: Instant::now(),
            }),
            (Self::Running { .. }, ProcessEvent::StopRequest { reason }) => Ok(Self::Stopping {
                since: Instant::now(),
                reason: reason.clone(),
            }),
            (Self::Running { .. }, ProcessEvent::CrashDetected { exit_code }) => Ok(Self::Failed {
                reason: FailureReason::CrashExit(*exit_code),
                failed_at: Instant::now(),
            }),
            (Self::Running { .. }, ProcessEvent::HealthCheckFailed) => Ok(Self::Failed {
                reason: FailureReason::HealthCheckStale,
                failed_at: Instant::now(),
            }),
            (Self::Stopping { reason, .. }, ProcessEvent::StopComplete) => Ok(Self::Stopped {
                reason: reason.clone(),
                stopped_at: Instant::now(),
            }),
            _ => Err(InvalidTransition {
                from: format!("{:?}", std::mem::discriminant(&self)),
                event: format!("{event:?}"),
            }),
        }
    }
}

/// High-level deployment lifecycle state, distinct from the low-level `ProcessState`
/// finite state machine. Used by the scheduler to track cluster-level deployment status.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelState {
    Loading,
    Running,
    Stopping,
    Stopped,
    Failed,
}

/// A deployed model instance tracked by the scheduler.
///
/// `id` and `assigned_node` use `String` to match `NodeStatus::id` and existing
/// scheduler keying semantics. The v1 spec's `priority: i32` field is included.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDeployment {
    pub id: String,
    pub engine: InferenceEngine,
    pub status: ModelState,
    pub assigned_node: String,
    pub port: u16,
    pub vram_requirement_mb: u64,
    pub priority: i32,
    pub args: Option<Vec<String>>,
    pub last_used: chrono::DateTime<chrono::Utc>,
    pub request_count: u64,
}

impl ModelDeployment {
    pub fn is_available(&self) -> bool {
        self.status == ModelState::Running
    }
}

#[derive(Debug, Clone)]
pub struct InvalidTransition {
    pub from: String,
    pub event: String,
}

impl std::fmt::Display for InvalidTransition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid transition: {} + {}", self.from, self.event)
    }
}

impl std::error::Error for InvalidTransition {}

#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum InferenceEngine {
    Ollama,
    Vllm,
    Sglang,
    LlamaCpp,
    Tgi,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[allow(non_camel_case_types)]
pub enum Quantization {
    Fp16,
    Fp8,
    Q8_0,
    Q5_K_M,
    Q4_K_M,
    Q3_K_M,
    GgufAuto,
}

pub fn estimate_vram_mb(params_billion: f64, quantization: Quantization) -> u64 {
    let gb_per_billion = match quantization {
        Quantization::Fp16 => 2.0,
        Quantization::Fp8 => 1.0,
        Quantization::Q8_0 => 1.1,
        Quantization::Q5_K_M => 0.7,
        Quantization::Q4_K_M => 0.6,
        Quantization::Q3_K_M => 0.45,
        Quantization::GgufAuto => 0.6,
    };
    ((params_billion * gb_per_billion * 1024.0) + 1024.0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_loaded_can_transition_to_loading() {
        let state = ProcessState::NotLoaded;
        let model_id = Uuid::new_v4();
        let next = state
            .transition(ProcessEvent::LoadRequest, model_id)
            .unwrap();
        assert!(matches!(next, ProcessState::Loading { .. }));
    }

    #[test]
    fn loading_completes_to_running() {
        let state = ProcessState::Loading {
            started_at: Instant::now(),
            model_id: Uuid::new_v4(),
        };
        let model_id = Uuid::new_v4();
        let next = state
            .transition(
                ProcessEvent::LoadComplete {
                    pid: 1234,
                    port: 8001,
                },
                model_id,
            )
            .unwrap();
        match next {
            ProcessState::Running { pid, port, .. } => {
                assert_eq!(pid, 1234);
                assert_eq!(port, 8001);
            }
            _ => panic!("expected Running"),
        }
    }

    #[test]
    fn loading_failure_transitions_to_failed() {
        let state = ProcessState::Loading {
            started_at: Instant::now(),
            model_id: Uuid::new_v4(),
        };
        let next = state
            .transition(ProcessEvent::LoadFailed, Uuid::nil())
            .unwrap();
        match next {
            ProcessState::Failed { reason, .. } => {
                assert_eq!(reason, FailureReason::LoadTimeout);
            }
            _ => panic!("expected Failed"),
        }
    }

    #[test]
    fn running_crash_transitions_to_failed() {
        let state = ProcessState::Running {
            pid: 100,
            port: 8001,
            loaded_at: Instant::now(),
            model_id: Uuid::new_v4(),
        };
        let next = state
            .transition(ProcessEvent::CrashDetected { exit_code: 137 }, Uuid::nil())
            .unwrap();
        match next {
            ProcessState::Failed { reason, .. } => {
                assert_eq!(reason, FailureReason::CrashExit(137));
            }
            _ => panic!("expected Failed"),
        }
    }

    #[test]
    fn running_stop_request_transitions_to_stopping() {
        let state = ProcessState::Running {
            pid: 100,
            port: 8001,
            loaded_at: Instant::now(),
            model_id: Uuid::new_v4(),
        };
        let next = state
            .transition(
                ProcessEvent::StopRequest {
                    reason: StopReason::UserRequested,
                },
                Uuid::nil(),
            )
            .unwrap();
        assert!(matches!(next, ProcessState::Stopping { .. }));
    }

    #[test]
    fn stopping_completes_to_stopped() {
        let state = ProcessState::Stopping {
            since: Instant::now(),
            reason: StopReason::Evicted,
        };
        let next = state
            .transition(ProcessEvent::StopComplete, Uuid::nil())
            .unwrap();
        match next {
            ProcessState::Stopped { reason, .. } => {
                assert_eq!(reason, StopReason::Evicted);
            }
            _ => panic!("expected Stopped"),
        }
    }

    #[test]
    fn failed_can_reload() {
        let state = ProcessState::Failed {
            reason: FailureReason::CrashExit(1),
            failed_at: Instant::now(),
        };
        let next = state
            .transition(ProcessEvent::LoadRequest, Uuid::new_v4())
            .unwrap();
        assert!(matches!(next, ProcessState::Loading { .. }));
    }

    #[test]
    fn invalid_transition_returns_error() {
        let state = ProcessState::NotLoaded;
        let result = state.transition(ProcessEvent::StopComplete, Uuid::nil());
        assert!(result.is_err());
    }

    #[test]
    fn stop_reason_has_five_variants() {
        let reasons = [
            StopReason::UserRequested,
            StopReason::NodeShutdown,
            StopReason::Evicted,
            StopReason::HealthCheckFailed,
            StopReason::RollingUpgrade,
        ];
        assert_eq!(reasons.len(), 5);
    }

    #[test]
    fn failure_reason_has_eight_variants() {
        let reasons = [
            FailureReason::CrashExit(1),
            FailureReason::LoadTimeout,
            FailureReason::HealthCheckStale,
            FailureReason::VramAllocationFailed,
            FailureReason::PortAllocationFailed,
            FailureReason::KillTimeout,
            FailureReason::ExecutableNotFound,
            FailureReason::GpuDeviceUnavailable,
        ];
        assert_eq!(reasons.len(), 8);
    }

    #[test]
    fn is_available_only_when_running() {
        assert!(!ProcessState::NotLoaded.is_available());
        assert!(ProcessState::Running {
            pid: 1,
            port: 8001,
            loaded_at: Instant::now(),
            model_id: Uuid::nil(),
        }
        .is_available());
    }

    #[test]
    fn running_health_check_failed_transitions_to_failed() {
        let state = ProcessState::Running {
            pid: 100,
            port: 8001,
            loaded_at: Instant::now(),
            model_id: Uuid::new_v4(),
        };
        let next = state
            .transition(ProcessEvent::HealthCheckFailed, Uuid::nil())
            .unwrap();
        match next {
            ProcessState::Failed { reason, .. } => {
                assert_eq!(reason, FailureReason::HealthCheckStale);
            }
            _ => panic!("expected Failed"),
        }
    }

    #[test]
    fn vram_estimate_fp16() {
        let vram = estimate_vram_mb(7.0, Quantization::Fp16);
        assert_eq!(vram, (7.0 * 2.0 * 1024.0 + 1024.0) as u64);
    }
}
