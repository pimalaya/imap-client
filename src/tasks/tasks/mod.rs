use imap_next::imap_types::response::StatusBody;
use thiserror::Error;

pub mod append;
pub mod appenduid;
pub mod authenticate;
pub mod capability;
pub mod check;
pub mod copy;
pub mod create;
pub mod delete;
pub mod enable;
pub mod expunge;
pub mod fetch;
pub mod id;
pub mod list;
pub mod login;
pub mod logout;
pub mod r#move;
pub mod noop;
pub mod search;
pub mod select;
pub mod sort;
pub mod store;
pub mod thread;

#[derive(Debug, Error)]
pub enum TaskError {
    #[error("unexpected BAD response: {}", .0.text)]
    UnexpectedBadResponse(StatusBody<'static>),

    #[error("unexpected NO response: {}", .0.text)]
    UnexpectedNoResponse(StatusBody<'static>),

    #[error("missing required data for command {0}")]
    MissingData(String),
}
