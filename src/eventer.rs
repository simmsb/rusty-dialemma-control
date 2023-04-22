use core::hash::Hash;
use postcard::accumulator::{CobsAccumulator, FeedResult};
use rusty_dilemma_shared::cmd::{CmdOrAck, Command};
use serde::{de::DeserializeOwned, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{broadcast, mpsc, Mutex};
use tracing::warn;

use crate::connection::{ByteReader, ByteWriter};

const BUF_SIZE: usize = 128;

type Event = tokio::sync::oneshot::Sender<()>;

pub struct TransmittedMessage<T> {
    pub msg: T,
    pub timeout: Option<Duration>,
}

pub fn low_latency_msg<T>(msg: T) -> TransmittedMessage<T> {
    TransmittedMessage {
        msg,
        timeout: Some(Duration::from_millis(2)),
    }
}

pub fn reliable_msg<T>(msg: T) -> TransmittedMessage<T> {
    TransmittedMessage {
        msg,
        timeout: Some(Duration::from_millis(20)),
    }
}

pub fn unreliable_msg<T>(msg: T) -> TransmittedMessage<T> {
    TransmittedMessage { msg, timeout: None }
}

pub struct Eventer<T> {
    tx: ByteWriter,
    rx: ByteReader,
    mix_chan: (mpsc::Sender<CmdOrAck<T>>, mpsc::Receiver<CmdOrAck<T>>),
    waiters: Arc<Mutex<HashMap<u8, Event>>>,
}

struct EventSender<T> {
    mix_chan: mpsc::Sender<CmdOrAck<T>>,
    waiters: Arc<Mutex<HashMap<u8, Event>>>,
}

struct EventOutProcessor<T> {
    tx: ByteWriter,
    mix_chan: mpsc::Receiver<CmdOrAck<T>>,
}

struct EventInProcessor<T, U: Clone> {
    rx: ByteReader,
    out_chan: broadcast::Sender<U>,
    mix_chan: mpsc::Sender<CmdOrAck<T>>,
    waiters: Arc<Mutex<HashMap<u8, Event>>>,
}

impl<T, U> EventInProcessor<T, U>
where
    U: DeserializeOwned + Hash + Clone + std::fmt::Debug,
{
    async fn recv_task_inner(&mut self) -> Option<()> {
        let mut accumulator = CobsAccumulator::<BUF_SIZE>::new();

        loop {
            let mut buf = [0u8; 16];
            let n = self.rx.read(&mut buf).await.ok()?;
            let mut window = &buf[..n];

            'cobs: while !window.is_empty() {
                window = match accumulator.feed(window) {
                    FeedResult::Consumed => break 'cobs,
                    FeedResult::OverFull(buf) => buf,
                    FeedResult::DeserError(buf) => {
                        warn!(
                            "Message decoder failed to deserialize a message of type {}: {:?}",
                            core::any::type_name::<CmdOrAck<U>>(),
                            buf
                        );
                        buf
                    }
                    FeedResult::Success { data, remaining } => {
                        let data: CmdOrAck<U> = data;

                        match data {
                            CmdOrAck::Cmd(c) => {
                                if c.validate() {
                                    if let Some(ack) = c.ack() {
                                        let _ = self.mix_chan.send(CmdOrAck::Ack(ack)).await;
                                    }
                                    let _ = self.out_chan.send(c.cmd);
                                } else {
                                    warn!("Corrupted parsed command: {:?}", c);
                                }
                            }
                            CmdOrAck::Ack(a) => {
                                if let Some(a) = a.validate() {
                                    let mut waiters = self.waiters.lock().await;
                                    if let Some(waker) = waiters.remove(&a.uuid) {
                                        let _ = waker.send(());
                                    }
                                } else {
                                    warn!("Corrupted parsed ack");
                                }
                            }
                        }

                        remaining
                    }
                }
            }
        }
    }

    async fn task(mut self) {
        loop {
            let _ = self.recv_task_inner().await;
        }
    }
}

impl<T> EventOutProcessor<T>
where
    T: Serialize,
{
    async fn task(mut self) {
        loop {
            let val = self.mix_chan.recv().await;

            let mut buf = [0u8; BUF_SIZE];
            if let Ok(buf) = postcard::to_slice_cobs(&val, &mut buf) {
                let _r = self.tx.write(buf).await;
            }
        }
    }
}

impl<T: Hash + Clone> EventSender<T> {
    async fn send_unreliable(&self, cmd: T) {
        let cmd = Command::new_unreliable(cmd.clone());
        let _ = self.mix_chan.send(CmdOrAck::Cmd(cmd)).await;
    }

    async fn send_reliable(&self, cmd: T, timeout: Duration) {
        loop {
            let (cmd, uuid) = Command::new_reliable(cmd.clone());
            let waiter = self.register_waiter(uuid).await;
            let _ = self.mix_chan.send(CmdOrAck::Cmd(cmd)).await;

            match tokio::time::timeout(timeout, waiter).await {
                Ok(_) => {
                    return;
                }
                Err(_) => {
                    self.deregister_waiter(uuid).await;
                }
            }
        }
    }

    async fn register_waiter(&self, uuid: u8) -> tokio::sync::oneshot::Receiver<()> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let mut waiters = self.waiters.lock().await;
        if waiters.insert(uuid, sender).is_some() {
            receiver
        } else {
            panic!("Duped waiter uuid")
        }
    }

    async fn deregister_waiter(&self, uuid: u8) {
        self.waiters.lock().await.remove(&uuid);
    }
}

pub async fn eventer<Sent, Received>(
    tx: ByteWriter,
    rx: ByteReader,
    mut cmd_chan: mpsc::Receiver<TransmittedMessage<Sent>>,
    out_chan: broadcast::Sender<Received>,
) where
    Sent: Hash + Clone + Serialize,
    Received: Hash + Clone + DeserializeOwned + std::fmt::Debug,
{
    let mix_chan = mpsc::channel(16);
    let waiters = Arc::new(Mutex::new(HashMap::new()));

    let sender = EventSender {
        mix_chan: mix_chan.0.clone(),
        waiters: Arc::clone(&waiters),
    };

    let out_processor = EventOutProcessor {
        tx,
        mix_chan: mix_chan.1,
    };

    let in_processor = EventInProcessor {
        rx,
        out_chan,
        mix_chan: mix_chan.0,
        waiters,
    };

    let sender_proc = async move {
        loop {
            let Some(TransmittedMessage { msg, timeout }) = cmd_chan.recv().await else { break };
            if let Some(timeout) = timeout {
                let _ = sender.send_reliable(msg, timeout).await;
            } else {
                let _ = sender.send_unreliable(msg).await;
            }
        }
    };

    tokio::join!(sender_proc, out_processor.task(), in_processor.task());
}
