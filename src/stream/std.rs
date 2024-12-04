use std::io::{Read, Write};

use imap_next::{Interrupt, Io, State};
use tracing::trace;

use super::{blocking, Error, Stream};

impl<S: Read + Write> blocking::StreamExt for Stream<S> {
    fn next<F: State>(&mut self, mut state: F) -> Result<F::Event, Error<F::Error>> {
        loop {
            // Progress the client/server
            let result = state.next();

            // Return events immediately without doing IO
            let interrupt = match result {
                Err(interrupt) => interrupt,
                Ok(event) => return Ok(event),
            };

            // Return errors immediately without doing IO
            let io = match interrupt {
                Interrupt::Io(io) => io,
                Interrupt::Error(err) => return Err(Error::State(err)),
            };

            // Handle the output bytes from the client/server
            match io {
                Io::Output(ref bytes) => {
                    let count = self.stream.write(bytes)?;
                    trace!("wrote {count}/{} bytes", bytes.len());
                    if count == 0 {
                        return Err(Error::Closed);
                    }
                }
                Io::NeedMoreInput => {
                    trace!("more input needed");
                }
            }

            let count = self.stream.read(&mut self.buf)?;
            trace!("read {count}/{} bytes", self.buf.len());
            if count == 0 {
                return Err(Error::Closed);
            }
        }
    }
}
