use rusty_dilemma_shared::device_to_host::DeviceToHost;
use rusty_dilemma_shared::host_to_device::HostToDevice;
use std::pin::Pin;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio_serial::SerialPortBuilderExt;
use tracing::info;

use crate::eventer;
use crate::eventer::TransmittedMessage;

mod bytechan {
    use bytes::Bytes;
    use futures::SinkExt;
    use tokio::sync::mpsc;

    use tokio_stream::wrappers::ReceiverStream;
    use tokio_stream::StreamExt;
    use tokio_util::io::CopyToBytes;
    use tokio_util::io::SinkWriter;
    use tokio_util::io::StreamReader;
    use tokio_util::sync::PollSender;
    pub type ByteWriter = impl tokio::io::AsyncWrite + Send;
    pub type ByteReader = impl tokio::io::AsyncRead + Send;

    pub fn byte_chan() -> (ByteWriter, ByteReader) {
        let (tx, rx) = mpsc::channel::<Bytes>(16);
        let sink = PollSender::new(tx)
            .sink_map_err(|_| std::io::Error::from(std::io::ErrorKind::BrokenPipe));
        let writer = SinkWriter::new(CopyToBytes::new(sink));

        let reader = StreamReader::new(ReceiverStream::new(rx).map(|e| Ok::<_, std::io::Error>(e)));

        (writer, reader)
    }
}

pub use bytechan::*;

pub struct SerialMixer {
    in_: ByteWriter,
    out: ByteReader,
}

impl tokio::io::AsyncWrite for SerialMixer {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.in_).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.in_).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.in_).poll_shutdown(cx)
    }
}

impl tokio::io::AsyncRead for SerialMixer {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.out).poll_read(cx, buf)
    }
}

pub fn mixer(mut serial: tokio_serial::SerialStream) -> SerialMixer {
    let (from_serial_tx, from_serial_rx) = byte_chan();
    let (to_serial_tx, to_serial_rx) = byte_chan();

    let mut serial_side = SerialMixer {
        in_: from_serial_tx,
        out: to_serial_rx,
    };

    // tokio::spawn(async move {
    //     let mut buf = [0u8; 16];
    //     loop {
    //         serial.read(&mut buf).await;
    //         print!("{:?}", buf);
    //     }
    // });

    tokio::spawn(async move {
        let _ = tokio::io::copy_bidirectional(&mut serial, &mut serial_side).await;
        info!("byte copy task closed");
    });

    SerialMixer {
        in_: to_serial_tx,
        out: from_serial_rx,
    }
}

pub fn connect() -> eyre::Result<(
    mpsc::Sender<TransmittedMessage<HostToDevice>>,
    broadcast::Receiver<DeviceToHost>,
)> {
    let port = serialport::available_ports()?
        .into_iter()
        .inspect(|p| info!(port = ?p, "Found a serial port"))
        .filter(|p| match &p.port_type {
            serialport::SerialPortType::UsbPort(usb) => (usb.vid, usb.pid) == (0xf000, 0xbaaa),
            _ => false,
        })
        .next()
        .ok_or_else(|| eyre::eyre!("Couldn't find the keyboard :("))?;

    let port = tokio_serial::new(port.port_name, 115200).open_native_async()?;

    let SerialMixer { in_, out } = mixer(port);

    let (broadcast_tx, broadcast_rx) = broadcast::channel(16);
    let (cmd_tx, cmd_rx) = mpsc::channel(16);

    tokio::spawn(eventer::eventer::<HostToDevice, DeviceToHost>(
        in_,
        out,
        cmd_rx,
        broadcast_tx,
    ));

    Ok((cmd_tx, broadcast_rx))
}
