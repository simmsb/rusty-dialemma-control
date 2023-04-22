use std::path::PathBuf;

use crate::{connection, logger};

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum Side {
    Left,
    Right,
}

#[derive(Debug, clap::Parser)]
pub struct UpdateOpts {
    fw: PathBuf,
    #[clap(short, long, value_enum)]
    side: Side,
}

impl UpdateOpts {
    pub async fn execute(self) -> color_eyre::eyre::Result<()> {
        let (_cmds_in, msgs_out) = connection::connect()?;

        tokio::spawn(logger::logger(msgs_out.resubscribe()));

        let _ = tokio::signal::ctrl_c().await;

        Ok(())
    }
}
