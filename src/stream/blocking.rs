use imap_next::State;

use super::Error;

pub trait StreamExt {
    fn next<F: State>(&mut self, state: F) -> Result<F::Event, Error<F::Error>>;
}
