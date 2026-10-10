use anyhow::Result;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum EvaluationCommand {
    /// Export tracked files at an exact SHA, never the current checkout or Git database.
    Prepare {
        #[arg(long)]
        repository: PathBuf,
        #[arg(long)]
        base_sha: String,
        #[arg(long)]
        facts: PathBuf,
        #[arg(long)]
        output: PathBuf,
        /// Explicit audited omissions, recorded in the snapshot manifest.
        #[arg(long)]
        exclude_prefix: Vec<String>,
    },
}
pub fn handle(command: &EvaluationCommand) -> Result<()> {
    match command {
        EvaluationCommand::Prepare {
            repository,
            base_sha,
            facts,
            output,
            exclude_prefix,
        } => {
            let value = feynman_local::evaluation::prepare(
                repository,
                base_sha,
                facts,
                output,
                exclude_prefix,
            )?;
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
    }
    Ok(())
}
