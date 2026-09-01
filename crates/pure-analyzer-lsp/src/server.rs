use std::io::{self, BufRead, Write};

use crate::{
    CancellationRegistry, DocumentStore, WorkspaceConfiguration, dispatch, frame::read_frame,
};

/// The terminal result of an LSP server session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServerExit {
    /// The client sent `shutdown` before `exit`.
    Clean,
    /// The client closed the stream or exited before shutdown.
    Unclean,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Lifecycle {
    New,
    Running,
    ShuttingDown,
}

/// A synchronous stdio JSON-RPC server with explicit front-end boundaries.
#[derive(Debug)]
pub struct Server {
    pub(crate) cancellation: CancellationRegistry,
    pub(crate) configuration: WorkspaceConfiguration,
    pub(crate) documents: DocumentStore,
    pub(crate) lifecycle: Lifecycle,
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

impl Server {
    /// Construct a server before its `initialize` request.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cancellation: CancellationRegistry::default(),
            configuration: WorkspaceConfiguration::default(),
            documents: DocumentStore::default(),
            lifecycle: Lifecycle::New,
        }
    }

    /// Return the front-end document store.
    #[must_use]
    pub const fn documents(&self) -> &DocumentStore {
        &self.documents
    }

    /// Return the front-end workspace configuration boundary.
    #[must_use]
    pub const fn configuration(&self) -> &WorkspaceConfiguration {
        &self.configuration
    }

    /// Return the front-end cancellation boundary.
    #[must_use]
    pub const fn cancellation(&self) -> &CancellationRegistry {
        &self.cancellation
    }

    /// Serve framed JSON-RPC messages until the client exits or closes input.
    ///
    /// The caller owns transport lifetime and receives an I/O error for invalid
    /// framing or JSON. Valid but unsupported protocol requests get standard
    /// JSON-RPC error responses instead.
    pub fn serve<R: BufRead, W: Write>(
        &mut self,
        reader: &mut R,
        writer: &mut W,
    ) -> io::Result<ServerExit> {
        while let Some(message) = read_frame(reader)? {
            if let Some(exit) = dispatch::handle(self, message, writer)? {
                return Ok(exit);
            }
        }
        Ok(ServerExit::Unclean)
    }
}

/// Serve the Language Server Protocol over this process's standard streams.
pub fn serve_stdio() -> io::Result<ServerExit> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    Server::new().serve(&mut stdin.lock(), &mut stdout.lock())
}
