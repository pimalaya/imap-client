use imap_next::{
    client::Client as ClientNext,
    imap_types::response::{Response, Status},
    Interrupt, State,
};
use tracing::{debug, warn};

use super::{Scheduler, SchedulerError, SchedulerEvent, Task, TaskHandle};

/// The resolver is a scheduler than manages one task at a time.
pub struct Resolver {
    pub scheduler: Scheduler,
}

impl Resolver {
    /// Create a new resolver.
    pub fn new(client_next: ClientNext) -> Self {
        Self {
            scheduler: Scheduler::new(client_next),
        }
    }

    /// Enqueue a [`Task`] for immediate resolution.
    pub fn resolve<T: Task>(&mut self, task: T) -> ResolvingTask<'_, T> {
        let handle = self.scheduler.enqueue_task(task);

        ResolvingTask {
            resolver: self,
            handle,
        }
    }
}

impl State for Resolver {
    type Event = SchedulerEvent;
    type Error = SchedulerError;

    fn enqueue_input(&mut self, bytes: &[u8]) {
        self.scheduler.enqueue_input(bytes);
    }

    fn next(&mut self) -> Result<Self::Event, Interrupt<Self::Error>> {
        self.scheduler.progress()
    }
}

pub struct ResolvingTask<'a, T: Task> {
    resolver: &'a mut Resolver,
    handle: TaskHandle<T>,
}

impl<T: Task> State for ResolvingTask<'_, T> {
    type Event = T::Output;
    type Error = SchedulerError;

    fn enqueue_input(&mut self, bytes: &[u8]) {
        self.resolver.enqueue_input(bytes);
    }

    fn next(&mut self) -> Result<Self::Event, Interrupt<Self::Error>> {
        loop {
            match self.resolver.next()? {
                SchedulerEvent::GreetingReceived(greeting) => {
                    debug!("received greeting: {greeting:?}");
                }
                SchedulerEvent::TaskFinished(mut token) => {
                    if let Some(output) = self.handle.resolve(&mut token) {
                        break Ok(output);
                    } else {
                        warn!(?token, "received unexpected task token")
                    }
                }
                SchedulerEvent::Unsolicited(unsolicited) => {
                    if let Response::Status(Status::Bye(bye)) = unsolicited {
                        let err = SchedulerError::UnexpectedByeResponse(bye);
                        break Err(Interrupt::Error(err));
                    } else {
                        warn!(?unsolicited, "received unsolicited");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::assert_impl_all;

    use super::Resolver;

    assert_impl_all!(Resolver: Send);
}
