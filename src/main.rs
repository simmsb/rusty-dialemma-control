#![feature(type_alias_impl_trait)]

use clap::Parser;
use color_eyre::eyre::Result;

mod connection;
mod eventer;
mod logger;
mod update;

#[derive(Debug, clap::Parser)]
struct Opts {
    #[clap(subcommand)]
    command: ControlCommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum ControlCommand {
    /// Update the firmware
    Update(crate::update::UpdateOpts),

    /// Listen to log messages
    Log(crate::logger::LogOpts),
}

fn install_tracing() -> Result<()> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let fmt_layer = tracing_subscriber::fmt::layer().compact();
    let filter_layer = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(concat!(env!("CARGO_CRATE_NAME"), "=debug").parse()?)
        .from_env()?;

    tracing_subscriber::registry()
        .with(tracing_error::ErrorLayer::default())
        .with(filter_layer)
        .with(fmt_layer)
        .init();

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let opts = Opts::parse();
    color_eyre::install()?;
    install_tracing()?;

    match opts.command {
        ControlCommand::Update(u) => u.execute().await?,
        ControlCommand::Log(l) => l.execute().await?,
    }

    Ok(())
}
