use std::{path::PathBuf, time::Duration};

use rusty_dilemma_shared::{host_to_device::{HostToDevice, HostToDeviceMsg}, side::KeyboardSide, fw::FWCmd};
use tracing::info;

use crate::{connection, logger, eventer::reliable_msg};

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
        let (cmds_in, msgs_out) = connection::connect()?;

        tokio::spawn(async move {
            if let Err(err) = logger::logger(msgs_out).await {
                tracing::error!(?err, "Logger borked");
            }
        });

        let fw = std::fs::read(&self.fw)?;

        info!(file = ?self.fw, "Starting fw update");

        let side = match self.side {
            Side::Left => KeyboardSide::Left,
            Side::Right => KeyboardSide::Right,
        };

        cmds_in.send(reliable_msg(HostToDevice { target_side: Some(side), msg: HostToDeviceMsg::FWCmd(FWCmd::Prepare) })).await?;

        let mut progress = 0;
        for chunk in fw.chunks(rusty_dilemma_shared::fw::FW_CHUNK_SIZE) {
            let cmd = FWCmd::WriteChunk { offset: progress, buf: heapless::Vec::from_slice(chunk).unwrap() };
            progress += chunk.len() as u32;
            cmds_in.send(reliable_msg(HostToDevice { target_side: Some(side), msg: HostToDeviceMsg::FWCmd(cmd) })).await?;
            info!("Progress: {}/{}", progress, fw.len());
        }
        info!("Finished sending firmware, issuing reboot");
        cmds_in.send(reliable_msg(HostToDevice { target_side: Some(side), msg: HostToDeviceMsg::FWCmd(FWCmd::Commit) })).await?;
        let _ = tokio::signal::ctrl_c().await;

        Ok(())
    }
}
