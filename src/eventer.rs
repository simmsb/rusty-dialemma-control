use core::hash::Hash;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use postcard::accumulator::{CobsAccumulator, FeedResult};
use serde::{de::DeserializeOwned, Serialize};
use rusty_dilemma_shared::cmd::{CmdOrAck, Command};
use tokio::sync::{Mutex, mpsc};
use futures::io::{AsyncReadExt, AsyncWriteExt};

const BUF_SIZE: usize = 128;

type Event = tokio::sync::oneshot::Sender<()>;

pub struct TransmittedMessage<T> {
    pub msg: T,
    pub timeout: Option<Duration>,
}

pub fn low_latency_msg<T>(msg: T) -> TransmittedMessage<T> {
    TransmittedMessage { msg, timeout: Some(Duration::from_millis(2)) }
}

pub fn reliable_msg<T>(msg: T) -> TransmittedMessage<T> {
    TransmittedMessage { msg, timeout: Some(Duration::from_millis(20)) }
}

pub fn unreliable_msg<T>(msg: T) -> TransmittedMessage<T> {
    TransmittedMessage { msg, timeout: None }
}

pub struct Eventer<T> {
    tx: sluice::pipe::PipeWriter,
    rx: sluice::pipe::PipeReader,
    mix_chan: (mpsc::Sender<CmdOrAck<T>>, mpsc::Receiver<CmdOrAck<T>>),
    waiters: Arc<Mutex<HashMap<u8, Arc<Event>>>>,
}

struct EventSender<T> {
    mix_chan: (mpsc::Sender<CmdOrAck<T>>, mpsc::Receiver<CmdOrAck<T>>),
    waiters: Arc<Mutex<HashMap<u8, Arc<Event>>>>,
}

struct EventOutProcessor<T> {
    tx: sluice::pipe::PipeWriter,
    mix_chan: (mpsc::Sender<CmdOrAck<T>>, mpsc::Receiver<CmdOrAck<T>>),
}

struct EventInProcessor<
    T,
    U: Clone,
> {
    rx: sluice::pipe::PipeReader,
    out_chan: mpsc::Sender<U>,
    mix_chan: (mpsc::Sender<CmdOrAck<T>>, mpsc::Receiver<CmdOrAck<T>>),
    waiters: Arc<Mutex<HashMap<u8, Arc<Event>>>>,
}

impl<T, U>
    EventInProcessor<T, U>
where
    U: DeserializeOwned + Hash + Clone,
{
    async fn recv_task_inner(&mut self) -> Option<()> {
        let mut accumulator = CobsAccumulator::<BUF_SIZE>::new();

        loop {
            let mut buf = [0u8; 1];
            self.rx.read(&mut buf).await.ok()?;
            let mut window = &buf[..];

            'cobs: while !window.is_empty() {
                window = match accumulator.feed(window) {
                    FeedResult::Consumed => break 'cobs,
                    FeedResult::OverFull(buf) => buf,
                    FeedResult::DeserError(buf) => {
                        eprintln!(
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
                                        self.mix_chan.0.send(CmdOrAck::Ack(ack)).await;
                                        self.out_chan.send(c.cmd).await;
                                    }
                                } else {
                                    eprintln!("Corrupted parsed command: {:?}", c);
                                }
                            }
                            CmdOrAck::Ack(a) => {
                                if let Some(a) = a.validate() {
                                    let mut waiters = self.waiters.lock().await;
                                    if let Some(waker) = waiters.remove(&a.uuid) {
                                        let _ = waker.send(());
                                    }
                                } else {
                                    eprintln!("Corrupted parsed ack");
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
    async fn task(self) {
        loop {
            let val = self.mix_chan.1.recv().await;

            let mut buf = [0u8; BUF_SIZE];
            if let Ok(buf) = postcard::to_slice_cobs(&val, &mut buf) {
                let r = self.tx.write(buf).await;
            }
        }
    }
}

impl<T: Hash + Clone> EventSender<T> {
    async fn send_unreliable(&self, cmd: T) {
        let cmd = Command::new_unreliable(cmd.clone());
        self.mix_chan.0.send(CmdOrAck::Cmd(cmd)).await;
    }

    async fn send_reliable(&self, cmd: T, timeout: Duration) {
        loop {
            let (cmd, uuid) = Command::new_reliable(cmd.clone());
            let waiter = self.register_waiter(uuid).await;
            self.mix_chan.0.send(CmdOrAck::Cmd(cmd)).await;

            match tokio::time::timeout(timeout, waiter.wait()).await {
                Ok(_) => {
                    return;
                }
                Err(_) => {
                    self.deregister_waiter(uuid).await;
                }
            }
        }
    }

    async fn register_waiter(&self, uuid: u8) -> Arc<P> {
        let signal = Arc::new(Event::new());
        let mut waiters = self.waiters.lock().await;
        if waiters.insert(uuid, signal.clone()).is_ok() {
            signal
        } else {
            panic!("Duped waiter uuid")
        }
    }

    async fn deregister_waiter(&self, uuid: u8) {
        self.waiters.lock().await.remove(&uuid);
    }
}

impl<'a, T, TX, RX> Eventer<T, TX, RX> {
    pub fn new(tx: TX, rx: RX) -> Self {
        Self {
            tx,
            rx,
            mix_chan: Channel::new(),
            waiters: Mutex::new(heapless::FnvIndexMap::new()),
        }
    }

    pub fn split_tasks<
        's,
        U,
        const N: usize,
        const CAP: usize,
        const SUBS: usize,
        const PUBS: usize,
    >(
        &'s mut self,
        cmd_chan: &'static Channel<ThreadModeRawMutex, TransmittedMessage<T>, N>,
        out_chan: Publisher<'static, ThreadModeRawMutex, U, CAP, SUBS, PUBS>,
    ) -> (impl Future + 's, impl Future + 's, impl Future + 's)
    where
        T: Hash + Clone + Serialize + Format,
        U: Hash + Clone + DeserializeOwned + Format,
        TX: embedded_io::asynch::Write,
        RX: embedded_io::asynch::Read,
        <TX as embedded_io::Io>::Error: Format,
    {
        let sender = EventSender {
            mix_chan: &self.mix_chan,
            waiters: &self.waiters,
        };

        let out_processor = EventOutProcessor {
            tx: &mut self.tx,
            mix_chan: &self.mix_chan,
        };

        let in_processor = EventInProcessor {
            rx: &mut self.rx,
            out_chan,
            mix_chan: &self.mix_chan,
            waiters: &self.waiters,
        };

        let sender_proc = async move {
            loop {
                let TransmittedMessage { msg, timeout } = cmd_chan.recv().await;
                if let Some(timeout) = timeout {
                    let _ = sender.send_reliable(msg, timeout).await;
                } else {
                    let _ = sender.send_unreliable(msg).await;
                }
            }
        };

        (sender_proc, out_processor.task(), in_processor.task())
    }
}
