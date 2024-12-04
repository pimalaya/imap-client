pub mod blocking;
#[cfg(feature = "futures")]
pub mod futures;
#[cfg(feature = "std")]
pub mod std;

use ::std::io;

use imap_next::State;
use thiserror::Error;

pub trait StreamExt {
    async fn next<F: State>(&mut self, state: F) -> Result<F::Event, Error<F::Error>>;
}

pub struct Stream<S> {
    stream: S,
    buf: Vec<u8>,
}

impl<S> Stream<S> {
    pub fn new(stream: S) -> Self {
        Self::with_capacity(1024, stream)
    }

    pub fn with_capacity(capacity: usize, stream: S) -> Self {
        Self {
            stream,
            buf: vec![0; capacity].into(),
        }
    }

    pub fn get_ref(&self) -> &S {
        &self.stream
    }

    pub fn get_mut(&mut self) -> &mut S {
        &mut self.stream
    }

    pub fn into_inner(self) -> S {
        self.stream
    }
}

/// Error during reading into or writing from a stream.
#[derive(Debug, Error)]
pub enum Error<E> {
    /// Operation failed because stream is closed.
    ///
    /// We detect this by checking if the read or written byte count is 0. Whether the stream is
    /// closed indefinitely or temporarily depends on the actual stream implementation.
    #[error("Stream was closed")]
    Closed,
    /// An I/O error occurred in the underlying stream.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// An error occurred while progressing the state.
    #[error(transparent)]
    State(E),
}
