pub mod args;
pub mod commands;
pub mod error;
pub mod output;

use crate::cli::args::CliArgs;
use crate::cli::commands::to_request;
use crate::cli::error::CliError;
use crate::connection::send_request;
use std::process::ExitCode;

pub async fn run_with_args(args: CliArgs) -> ExitCode {
    match run_inner(args).await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}

async fn run_inner(args: CliArgs) -> Result<ExitCode, CliError> {
    let request = to_request(&args)?;
    let response = send_request(&args.session, &request)
        .await
        .map_err(|e| CliError::CommandFailed(e.to_string()))?;
    output::print_response(&response, args.json);
    if response.success {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::from(1))
    }
}
