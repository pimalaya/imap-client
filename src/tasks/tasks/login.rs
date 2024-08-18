use imap_next::imap_types::{
    command::CommandBody,
    core::{AString, Vec1},
    response::{Capability, Code, Data, StatusBody, StatusKind},
    secret::Secret,
};

use super::TaskError;
use crate::Task;

#[derive(Clone, Debug)]
pub struct LoginTask {
    username: AString<'static>,
    password: Secret<AString<'static>>,
    output: Option<Vec1<Capability<'static>>>,
}

impl LoginTask {
    pub fn new(username: AString<'static>, password: Secret<AString<'static>>) -> Self {
        Self {
            username,
            password,
            output: None,
        }
    }
}

impl Task for LoginTask {
    type Output = Result<Option<Vec1<Capability<'static>>>, TaskError>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Login {
            username: self.username.clone(),
            password: self.password.clone(),
        }
    }

    // Capabilities may (unfortunately) be found in a data response.
    // See https://github.com/modern-email/defects/issues/18
    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        if let Data::Capability(capabilities) = data {
            self.output = Some(capabilities);
            None
        } else {
            Some(data)
        }
    }

    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => Ok(
                if let Some(Code::Capability(capabilities)) = status_body.code {
                    Some(capabilities)
                } else {
                    self.output
                },
            ),
            StatusKind::No => Err(TaskError::UnexpectedNoResponse(status_body)),
            StatusKind::Bad => Err(TaskError::UnexpectedBadResponse(status_body)),
        }
    }
}
