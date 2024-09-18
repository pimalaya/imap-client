use imap_next::imap_types::{
    command::CommandBody,
    core::Vec1,
    extensions::enable::CapabilityEnable,
    response::{Data, StatusBody, StatusKind},
};

use super::TaskError;
use crate::Task;

#[derive(Clone, Debug)]
pub struct EnableTask {
    requested_capabilities: Vec1<CapabilityEnable<'static>>,
    enabled_capabilities: Option<Vec<CapabilityEnable<'static>>>,
}

impl EnableTask {
    pub fn new(capabilities: Vec1<CapabilityEnable<'static>>) -> Self {
        Self {
            requested_capabilities: capabilities,
            enabled_capabilities: None,
        }
    }
}

impl Task for EnableTask {
    type Output = Result<Option<Vec<CapabilityEnable<'static>>>, TaskError>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Enable {
            capabilities: self.requested_capabilities.clone(),
        }
    }

    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        if let Data::Enabled { capabilities } = data {
            self.enabled_capabilities = Some(capabilities);
            None
        } else {
            Some(data)
        }
    }

    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => Ok(self.enabled_capabilities),
            StatusKind::No => Err(TaskError::UnexpectedNoResponse(status_body)),
            StatusKind::Bad => Err(TaskError::UnexpectedBadResponse(status_body)),
        }
    }
}
