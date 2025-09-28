#[cfg(feature = "smol")]
mod smol;
#[cfg(feature = "tokio")]
mod tokio;

use thiserror::Error;

pub struct Stream<S> {
    stream: S,
    buf: Vec<u8>,
}

impl<S> Stream<S> {
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            buf: vec![0; 1024].into(),
        }
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
    Io(#[from] std::io::Error),
    /// An error occurred while progressing the state.
    #[error(transparent)]
    State(E),
}
