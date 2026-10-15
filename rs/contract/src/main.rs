//! Command-line entry point for the contract comparator.

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let group = match (args.next().as_deref(), args.next()) {
        (Some("--group"), Some(group)) => group,
        _ => {
            eprintln!("usage: verdaxis-contract --group GROUP");
            return ExitCode::from(2);
        }
    };
    match verdaxis_contract::compare_group(&group) {
        Ok(()) => ExitCode::SUCCESS,
        Err(reason) => {
            eprintln!("{reason}");
            ExitCode::FAILURE
        }
    }
}
