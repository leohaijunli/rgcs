//! Inspector session model (Signal Inspector plan §4/§5, ADR-015/016).
//!
//! The Rust-side counter of the frontend: every plotted trace binds a
//! [`SignalId`] to a DSP filter chain and an optional analyzer, and the session
//! applies them to the live sample stream. The frontend only renders; the
//! filtering runs here so a headless server or QC phase can reuse it (ADR-001).

pub mod session;

pub use session::{
    AlgoConfig, AnalyzerSource, Plot, Session, SessionError, Trace, TraceId, TraceSample,
    GAP_FACTOR,
};
