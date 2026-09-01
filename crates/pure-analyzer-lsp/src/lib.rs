#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! A deliberately thin Language Server Protocol front end for `pure-analyzer`.
//!
//! Protocol values, document state, workspace configuration, and cancellation
//! remain in this crate. The analysis-engine crates therefore remain transport
//! and protocol independent.

mod cancellation;
mod dispatch;
mod document;
mod frame;
mod response;
mod server;
mod state;
mod workspace;

pub use cancellation::{CancellationRegistry, RequestId};
pub use document::{DocumentSnapshot, DocumentStore};
pub use server::{Server, ServerExit, serve_stdio};
pub use workspace::WorkspaceConfiguration;
