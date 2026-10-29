//! RPi is a Rust-first, protocol-oriented agent runtime.
//!
//! Shared async orchestration for TUI, local Web GUI and embedded applications.

pub mod agent;
pub mod budget;
pub mod config;
pub mod context;
pub mod events;
pub mod facet;
pub mod mcp;
pub mod model;
pub mod object;
pub mod projects;
pub mod provider;
pub mod session;
pub mod skills;
/// Utilities used by protocol fixtures. Test servers are constrained to the
/// documented loopback range 300-400 so integration runs never bind arbitrary
/// ephemeral ports. The helper keeps the listener open while the caller starts
/// its server, which also prevents races between concurrently running tests.
pub mod test_support;
pub mod tools;

pub use agent::{AgentRuntime, RuntimeBuilder, TaskProgress, TaskRequest, TaskResult};
pub use config::RuntimeConfig;
pub use events::{EventLog, TraceEvent, TraceId};
pub use model::{ModelRouter, ModelTier, TaskProfile};
pub use object::{Object, ObjectEnvelope, ObjectId, ObjectKind, ObjectRegistry};
pub use projects::{Project, ProjectStore};
