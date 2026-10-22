//! Command boundary; implementations are introduced by their owning tasks.
use std::ffi::OsString;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum Exit {
    Ok = 0,
    Usage = 2,
    Precondition = 3,
    Dependency = 4,
    Ambiguous = 5,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Serve(()),
}

pub fn parse_arguments(_args: Vec<OsString>) -> Result<Command, Exit> {
    Err(Exit::Precondition)
}
