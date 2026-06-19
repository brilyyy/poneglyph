//! User-facing error types for the CLI and MCP boundaries.
//!
//! Internal code keeps using `anyhow::Result`. This module defines a thin
//! wrapper that categorizes errors for presentation (exit codes, MCP error
//! codes) without requiring every function to change its signature.

use std::fmt;

/// Error categories for user-facing presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// User made a mistake (bad input, missing arg, invalid value).
    User,
    /// Something in the environment is wrong (missing file, DB locked, port busy).
    Environment,
    /// Internal bug or unexpected failure.
    Internal,
}

/// A categorized error with a user-friendly message and optional source chain.
pub struct PoneglyphError {
    pub kind: ErrorKind,
    pub message: String,
    pub source: Option<anyhow::Error>,
}

impl PoneglyphError {
    pub fn user(msg: impl Into<String>) -> Self {
        Self { kind: ErrorKind::User, message: msg.into(), source: None }
    }

    pub fn env(msg: impl Into<String>, source: anyhow::Error) -> Self {
        Self { kind: ErrorKind::Environment, message: msg.into(), source: Some(source) }
    }

    pub fn internal(source: anyhow::Error) -> Self {
        Self { kind: ErrorKind::Internal, message: "internal error".into(), source: Some(source) }
    }

    /// Exit code for CLI usage.
    pub fn exit_code(&self) -> i32 {
        match self.kind {
            ErrorKind::User => 1,
            ErrorKind::Environment => 1,
            ErrorKind::Internal => 2,
        }
    }
}

impl fmt::Display for PoneglyphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl fmt::Debug for PoneglyphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(source) = &self.source {
            write!(f, "\n  caused by: {source:#}")?;
        }
        Ok(())
    }
}

/// Convert an `anyhow::Error` into a `PoneglyphError` by guessing the kind
/// from the error message. Ponytail: heuristic, not perfect — good enough
/// for CLI presentation.
impl From<anyhow::Error> for PoneglyphError {
    fn from(e: anyhow::Error) -> Self {
        let msg = format!("{e:#}");
        let kind = if msg.contains("not found") || msg.contains("no such") {
            ErrorKind::User
        } else if msg.contains("permission denied") || msg.contains("locked")
            || msg.contains("already in use") || msg.contains("failed to")
        {
            ErrorKind::Environment
        } else {
            ErrorKind::Internal
        };
        Self { kind, message: msg, source: Some(e) }
    }
}
