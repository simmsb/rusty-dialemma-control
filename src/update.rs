use std::path::PathBuf;
use std::pin::Pin;

use futures_io::AsyncWrite as _;
use futures_io::AsyncRead as _;
use tokio_serial::SerialPortBuilderExt;
use tokio_util::compat::FuturesAsyncReadCompatExt;

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

pub struct SerialMixer {
    in_: sluice::pipe::PipeWriter,
    out: sluice::pipe::PipeReader
}

impl tokio::io::AsyncWrite for SerialMixer {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.in_).poll_write(cx, buf)
    }

    fn poll_flush(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.in_).poll_flush(cx)
    }

    fn poll_shutdown(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.in_).poll_close(cx)
    }
}

impl tokio::io::AsyncRead for SerialMixer {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let mut c = (&mut self.out).compat();
        Pin::new(&mut c).poll_read(cx, buf)

    }
}

pub fn mixer(mut serial: tokio_serial::SerialStream) -> SerialMixer {
    let (from_serial_rx, from_serial_tx) = sluice::pipe::pipe();
    let (to_serial_rx, to_serial_tx) = sluice::pipe::pipe();

    let mut serial_side = SerialMixer {
        in_: from_serial_tx,
        out: to_serial_rx,
    };

    tokio::spawn(async move {
        let _ = tokio::io::copy_bidirectional(&mut serial, &mut serial_side).await;
    });

    SerialMixer {
        in_: to_serial_tx,
        out: from_serial_rx,
    }
}

impl UpdateOpts {
    pub async fn execute(self) -> color_eyre::eyre::Result<()> {
        let port = tokio_serial::available_ports()?
            .into_iter()
            .filter(|p| match &p.port_type {
                tokio_serial::SerialPortType::UsbPort(usb) => {
                    (usb.vid, usb.pid) == (0xf000, 0xbaaa)
                }
                _ => false,
            })
            .next()
            .ok_or_else(|| eyre::eyre!("Couldn't find the keyboard :("))?;

        let port = tokio_serial::new(port.port_name, 115200).open_native_async()?;

        let SerialMixer { in_, out } = mixer(port);

        Ok(())
    }
}
