//! The `pumice` orchestrator's library. The binary in `src/main.rs` is a thin
//! entry point on top of it, so the flow execution seam can be driven
//! directly by integration tests (with the fake AI CLI they get built).

pub mod dispatch;
pub mod orchestrator;
