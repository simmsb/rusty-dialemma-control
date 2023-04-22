use std::{fmt::Write, io::BufReader};

use rusty_dilemma_shared::{device_to_host::DeviceToHost, side::KeyboardSide};
use std::io::BufRead;
use tokio::sync::broadcast;
use tracing::info;

use crate::connection;

#[derive(Debug, clap::Parser)]
pub struct LogOpts {}

impl LogOpts {
    pub async fn execute(self) -> color_eyre::eyre::Result<()> {
        let (_cmds_in, msgs_out) = connection::connect()?;

        tokio::spawn(logger(msgs_out));

        let _ = tokio::signal::ctrl_c().await;

        Ok(())
    }
}

pub async fn logger(mut rx: broadcast::Receiver<DeviceToHost>) -> eyre::Result<()> {
    let (mut left_rb_tx, left_rb_rx) = ringbuf::SharedRb::new(512).split();
    let mut left_rb_rx = BufReader::new(left_rb_rx);
    let (mut right_rb_tx, right_rb_rx) = ringbuf::SharedRb::new(512).split();
    let mut right_rb_rx = BufReader::new(right_rb_rx);

    let mut s = String::new();

    loop {
        let msg = rx.recv().await?;

        match msg {
            DeviceToHost::Log { from_side, msg } => match from_side {
                KeyboardSide::Left => {
                    let _ =
                        left_rb_tx.write_str(std::str::from_utf8(&msg).unwrap_or("bad decode\r\n"));
                    if let Ok(_r) = left_rb_rx.read_line(&mut s) {
                        info!("left: {}", s.trim_end_matches(|c| c == '\r' || c == '\n'));
                        s.clear();
                    }
                }
                KeyboardSide::Right => {
                    let _ = right_rb_tx
                        .write_str(std::str::from_utf8(&msg).unwrap_or("bad decode\r\n"));
                    if let Ok(_r) = right_rb_rx.read_line(&mut s) {
                        info!("right: {}", s.trim_end_matches(|c| c == '\r' || c == '\n'));
                        s.clear();
                    }
                }
            },
        }
    }
}
