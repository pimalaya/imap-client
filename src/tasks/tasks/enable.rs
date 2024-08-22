use imap_next::imap_types::{
    command::CommandBody,
    core::Vec1,
    extensions::enable::CapabilityEnable,
    response::{StatusBody, StatusKind},
};

use super::TaskError;
use crate::Task;

#[derive(Clone, Debug)]
pub struct EnableTask {
    capabilities: Vec1<CapabilityEnable<'static>>,
}

impl EnableTask {
    pub fn new(capabilities: Vec1<CapabilityEnable<'static>>) -> Self {
        Self { capabilities }
    }
}

impl Task for EnableTask {
    type Output = Result<(), TaskError>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Enable {
            capabilities: self.capabilities.clone(),
        }
    }

    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => Ok(()),
            StatusKind::No => Err(TaskError::UnexpectedNoResponse(status_body)),
            StatusKind::Bad => Err(TaskError::UnexpectedBadResponse(status_body)),
        }
    }
}
