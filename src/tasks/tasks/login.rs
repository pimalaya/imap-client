use imap_next::imap_types::{
    command::CommandBody,
    core::AString,
    response::{StatusBody, StatusKind},
    secret::Secret,
};

use super::TaskError;
use crate::Task;

#[derive(Clone, Debug)]
pub struct LoginTask {
    username: AString<'static>,
    password: Secret<AString<'static>>,
}

impl LoginTask {
    pub fn new(username: AString<'static>, password: Secret<AString<'static>>) -> Self {
        Self { username, password }
    }
}

impl Task for LoginTask {
    type Output = Result<(), TaskError>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Login {
            username: self.username.clone(),
            password: self.password.clone(),
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
