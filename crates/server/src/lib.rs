//! HTTP transport for the anonymization engine.
//!
//! Tokio and axum live here and nowhere else: the engine crate is sync and
//! free of I/O. This crate owns wire types, job machinery and observability.

#![forbid(unsafe_code)]

pub mod config;
pub mod http;
pub mod job;
pub mod telemetry;
