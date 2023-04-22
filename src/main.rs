use clap::Parser;
use color_eyre::eyre::Result;

mod update;
mod eventer;


#[derive(Debug, clap::Parser)]
struct Opts {
    #[clap(subcommand)]
    command: ControlCommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum ControlCommand {
    /// List possible ports
    Update(crate::update::UpdateOpts),
}

#[tokio::main]
async fn main() -> Result<()> {
    let opts = Opts::parse();
    color_eyre::install()?;

    match opts.command {
        ControlCommand::Update(u) => u.execute().await?,
    }


    Ok(())
}
