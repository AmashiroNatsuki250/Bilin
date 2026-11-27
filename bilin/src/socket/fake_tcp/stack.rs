//! A minimum, userspace TCP based datagram stack
//!
//! # Overview
//!
//! `fake-tcp` is a reusable library that implements a minimum TCP stack in
//! user space using the Tun interface. It allows programs to send datagrams
//! as if they are part of a TCP connection. `fake-tcp` has been tested to
//! be able to pass through a variety of NAT and stateful firewalls while
//! fully preserves certain desirable behavior such as out of order delivery
//! and no congestion/flow controls.
//!
//! # Core Concepts
//!
//! The core of the `fake-tcp` crate compose of two structures. [`Stack`] and
//! [`Socket`].
//!
//! ## [`Stack`]
//!
//! [`Stack`] represents a virtual TCP stack that operates at
//! Layer 3. It is responsible for:
//!
//! * TCP active and passive open and handshake
//! * `RST` handling
//! * Interact with the Tun interface at Layer 3
//! * Distribute incoming datagrams to corresponding [`Socket`]
//!
//! ## [`Socket`]
//!
//! [`Socket`] represents a TCP connection. It registers the identifying
//! tuple `(src_ip, src_port, dest_ip, dest_port)` inside the [`Stack`] so
//! so that incoming packets can be distributed to the right [`Socket`] with
//! using a channel. It is also what the client should use for
//! sending/receiving datagrams.
//!
//! # Examples
//!
//! Please see [`client.rs`](https://github.com/dndx/phantun/blob/main/phantun/src/bin/client.rs)
//! and [`server.rs`](https://github.com/dndx/phantun/blob/main/phantun/src/bin/server.rs) files
//! from the `phantun` crate for how to use this library in client/server mode, respectively.

use super::packet::*;
use bytes::{Bytes, BytesMut};
use crossbeam::atomic::AtomicCell;
use std::collections::{HashMap, VecDeque};
use std::fmt;
#[cfg(test)]
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast;
use tokio::time;
use tokio_util::task::AbortOnDropHandle;
use tracing::{error, info, trace, warn};

const TIMEOUT: time::Duration = time::Duration::from_secs(1);
const MPMC_BUFFER_LEN: usize = 512;
const MAX_PENDING_FRAMES: usize = 64;
const MAX_PENDING_FRAME_SIZE: usize = 2048;
const PENDING_FRAME_TTL: time::Duration = time::Duration::from_secs(1);
const TCP_OPTION_END: u8 = 0;
const TCP_OPTION_NOP: u8 = 1;
const TCP_OPTION_SACK: u8 = 5;

struct TcpOptionIter<'a> {
    remaining: &'a [u8],
}

impl<'a> TcpOptionIter<'a> {
    fn new(options: &'a [u8]) -> Self {
        Self { remaining: options }
    }
}

impl<'a> Iterator for TcpOptionIter<'a> {
    type Item = (u8, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        let kind = *self.remaining.first()?;
        if kind == TCP_OPTION_END {
            self.remaining = &[];
            return None;
        }
        if kind == TCP_OPTION_NOP {
            self.remaining = &self.remaining[1..];
            return Some((kind, &[]));
        }
        let Some(&length) = self.remaining.get(1) else {
            self.remaining = &[];
            return None;
        };
        let length = usize::from(length);
        if length < 2 || length > self.remaining.len() {
            self.remaining = &[];
            return None;
        }
        let payload = &self.remaining[2..length];
        self.remaining = &self.remaining[length..];
        Some((kind, payload))
    }
}

#[async_trait::async_trait]
pub trait Tun: Send + Sync + 'static {
    async fn recv(&self, packet: &mut BytesMut) -> Result<usize, std::io::Error>;
    fn try_send(&self, packet: &Bytes) -> Result<(), std::io::Error>;
    fn driver_type(&self) -> &'static str;
}

#[derive(Hash, Eq, PartialEq, Clone, Debug)]
struct AddrTuple {
    local_addr: SocketAddr,
    remote_addr: SocketAddr,
}

impl AddrTuple {
    fn new(local_addr: SocketAddr, remote_addr: SocketAddr) -> AddrTuple {
        AddrTuple {
            local_addr,
            remote_addr,
        }
    }
}

#[derive(Default)]
struct StackState {
    tuples: HashMap<AddrTuple, flume::Sender<Bytes>>,
    pending: VecDeque<(time::Instant, AddrTuple, Bytes)>,
    closed: bool,
}

struct Shared {
    state: RwLock<StackState>,
    tun: Arc<dyn Tun>,
    tuples_purge: broadcast::Sender<AddrTuple>,
}

impl Shared {
    fn is_closed(&self) -> bool {
        self.state.read().unwrap().closed
    }

    fn mark_closed_and_clear_tuples(&self) -> usize {
        let mut state = self.state.write().unwrap();
        state.closed = true;
        let len = state.tuples.len();
        state.tuples.clear();
        state.pending.clear();
        len
    }
}

pub struct Stack {
    shared: Arc<Shared>,
    local_mac: MacAddr,
    reader_task: AbortOnDropHandle<()>,
}

#[derive(Hash, Eq, PartialEq, Clone, Copy, Debug)]
pub enum State {
    Idle,
    SynSent,
    Established,
}

pub struct Socket {
    shared: Arc<Shared>,
    tun: Arc<dyn Tun>,
    incoming: flume::Receiver<Bytes>,
    local_addr: SocketAddr,
    remote_addr: SocketAddr,
    local_mac: MacAddr,
    remote_mac: AtomicCell<Option<MacAddr>>,
    seq: AtomicCell<Option<u32>>,
    ack: AtomicCell<Option<u32>>,
    state: AtomicCell<State>,
}

/// A socket that represents a unique TCP connection between a server and client.
///
/// The `Socket` object itself satisfies `Sync` and `Send`, which means it can
/// be safely called within an async future.
///
/// To close a TCP connection that is no longer needed, simply drop this object
/// out of scope.
impl Socket {
    fn new(
        shared: Arc<Shared>,
        tun: Arc<dyn Tun>,
        local_addr: SocketAddr,
        remote_addr: SocketAddr,
        local_mac: MacAddr,
        remote_mac: Option<MacAddr>,
        state: State,
    ) -> (Socket, flume::Sender<Bytes>) {
        let (incoming_tx, incoming_rx) = flume::bounded(MPMC_BUFFER_LEN);

        (
            Socket {
                shared,
                tun,
                incoming: incoming_rx,
                local_addr,
                remote_addr,
                local_mac,
                remote_mac: AtomicCell::new(remote_mac),
                seq: AtomicCell::new(None),
                ack: AtomicCell::new(None),
                state: AtomicCell::new(state),
            },
            incoming_tx,
        )
    }

    fn build_tcp_packet(&self, seq: u32, flags: u8, payload: Option<&[u8]>) -> Bytes {
        build_tcp_packet(
            self.local_mac,
            self.remote_mac.load().unwrap_or_default(),
            self.local_addr,
            self.remote_addr,
            seq,
            self.ack.load().unwrap_or(0),
            flags,
            payload,
        )
    }

    /// Sends a datagram to the other end.
    ///
    /// This method takes `&self`, and it can be called safely by multiple threads
    /// at the same time.
    ///
    /// A return of `None` means the Tun socket returned an error
    /// and this socket must be closed.
    pub fn try_send(&self, payload: &[u8]) -> Option<()> {
        match self.state.load() {
            State::Established => {
                let seq = self
                    .seq
                    .fetch_update(|seq| seq.map(|seq| Some(seq.wrapping_add(payload.len() as u32))))
                    .ok()
                    .flatten()?;
                let buf = self.build_tcp_packet(seq, TCP_FLAG_ACK, Some(payload));
                self.tun.try_send(&buf).ok().and(Some(()))
            }
            _ => unreachable!(),
        }
    }

    pub fn close(&self) {
        if self.state.swap(State::Idle) != State::Idle
            && let Some(seq) = self.seq.load()
        {
            let buf = self.build_tcp_packet(seq, TCP_FLAG_RST, None);
            let _ = self.tun.try_send(&buf);
        }
    }

    /// Attempt to receive a datagram from the other end.
    ///
    /// This method takes `&self`, and it can be called safely by multiple threads
    /// at the same time.
    ///
    /// A return of `None` means the TCP connection is broken
    /// and this socket must be closed.
    pub async fn recv(&self, buf: &mut BytesMut) -> Option<usize> {
        tracing::trace!(
            "Socket recv called, local_addr: {:?}, remote_addr: {:?}",
            self.local_addr,
            self.remote_addr
        );
        loop {
            match self.state.load() {
                State::Established => {
                    let Ok(raw_buf) = self.incoming.recv_async().await else {
                        info!("Connection {} recv error", self);
                        return None;
                    };

                    let Some((src_mac, dst_mac, _v4_packet, tcp_packet)) =
                        parse_ip_packet(&raw_buf)
                    else {
                        trace!("Dropping malformed fake tcp packet for established socket");
                        continue;
                    };

                    tracing::trace!(
                        "Socket received TCP packet from {}({:?}) to {}({:?}): {:?}",
                        self.remote_addr,
                        src_mac,
                        self.local_addr,
                        dst_mac,
                        tcp_packet
                    );

                    self.remote_mac.store(Some(src_mac));

                    if tcp_packet.rst() {
                        info!("Connection {} reset by peer", self);
                        return None;
                    }

                    if tcp_packet.ack() {
                        // The first captured segment on a passive socket may carry data.
                        // Its ACK provides the OS handshake's initial send sequence.
                        advance_tcp_position(&self.seq, tcp_packet.ack_number().0 as u32);
                    }

                    let payload = tcp_packet.payload();

                    let new_ack =
                        (tcp_packet.seq_number().0 as u32).wrapping_add(payload.len() as u32);
                    advance_tcp_position(&self.ack, new_ack);

                    for (kind, option_payload) in TcpOptionIter::new(tcp_packet.options()) {
                        if kind == TCP_OPTION_SACK {
                            // SACK 选项类型为 5
                            for chunk in option_payload.chunks(8) {
                                if chunk.len() != 8 {
                                    continue;
                                }
                                let left = tcp_packet.ack_number().0 as u32;
                                let right = u32::from_be_bytes(chunk[0..4].try_into().unwrap());
                                let len = right.wrapping_sub(left);

                                let sack_end = u32::from_be_bytes(chunk[4..8].try_into().unwrap());
                                if len == 0 || sack_end <= left {
                                    continue;
                                }

                                let send_len = std::cmp::min(len, 1400) as usize;
                                let data = vec![0u8; send_len];

                                let buf = build_tcp_packet(
                                    self.local_mac,
                                    self.remote_mac.load().unwrap_or_default(),
                                    self.local_addr,
                                    self.remote_addr,
                                    left,
                                    self.ack.load().unwrap_or(0),
                                    TCP_FLAG_ACK,
                                    Some(&data),
                                );

                                if let Err(e) = self.tun.try_send(&buf) {
                                    tracing::error!("Failed to send SACK response: {}", e);
                                }
                                break;
                            }
                        }
                    }

                    if payload.is_empty() {
                        continue;
                    }

                    buf.extend_from_slice(payload);

                    return Some(payload.len());
                }
                State::SynSent => {
                    let Ok(Ok(buf)) = time::timeout(TIMEOUT, self.incoming.recv_async()).await
                    else {
                        info!("Waiting for client SYN + ACK timed out");
                        return None;
                    };
                    let Some((src_mac, _dst_mac, _v4_packet, tcp_packet)) = parse_ip_packet(&buf)
                    else {
                        trace!("Dropping malformed fake tcp packet during handshake");
                        continue;
                    };

                    if tcp_packet.rst() {
                        tracing::trace!("Connection {} reset by peer", self);
                        return None;
                    }

                    if tcp_packet.syn() && tcp_packet.ack() {
                        // found our SYN + ACK
                        self.seq.store(Some(tcp_packet.ack_number().0 as u32));
                        self.ack
                            .store(Some((tcp_packet.seq_number().0 as u32).wrapping_add(1)));
                        self.remote_mac.store(Some(src_mac));
                        self.state.store(State::Established);
                        return Some(0);
                    }
                }

                _ => unreachable!(),
            }
        }
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn remote_addr(&self) -> SocketAddr {
        self.remote_addr
    }
}

fn advance_tcp_position(position: &AtomicCell<Option<u32>>, candidate: u32) {
    let _ = position.fetch_update(|current| {
        if current.is_none_or(|current| (candidate.wrapping_sub(current) as i32) > 0) {
            Some(Some(candidate))
        } else {
            None
        }
    });
}

impl Drop for Socket {
    /// Drop the socket and close the TCP connection
    fn drop(&mut self) {
        let tuple = AddrTuple::new(self.local_addr, self.remote_addr);
        // dissociates ourself from the dispatch map
        let (removed, closed) = {
            let mut state = self.shared.state.write().unwrap();
            (state.tuples.remove(&tuple).is_some(), state.closed)
        };
        if !removed {
            if closed {
                trace!(?tuple, "Fake TCP tuple already removed after stack closed");
            } else {
                warn!(?tuple, "Fake TCP tuple missing while dropping socket");
            }
        }
        // purge cache
        let _ = self.shared.tuples_purge.send(tuple);

        self.close();

        info!("Fake TCP connection to {} closed", self);
    }
}

impl fmt::Display for Socket {
    /// User-friendly string representation of the socket
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "(Fake TCP connection from {} to {})",
            self.local_addr, self.remote_addr
        )
    }
}

/// A userspace TCP state machine
impl Stack {
    /// Create a new stack, `tun` is an array of [`Tun`](tokio_tun::Tun).
    /// When more than one [`Tun`](tokio_tun::Tun) object is passed in, same amount
    /// of reader will be spawned later. This allows user to utilize the performance
    /// benefit of Multiqueue Tun support on machines with SMP.
    pub fn new(tun: Arc<dyn Tun>, local_mac: Option<MacAddr>, listening: bool) -> Stack {
        let (tuples_purge_tx, _tuples_purge_rx) = broadcast::channel(16);
        let shared = Arc::new(Shared {
            state: RwLock::new(StackState::default()),
            tun: tun.clone(),
            tuples_purge: tuples_purge_tx.clone(),
        });

        let t = tokio::spawn(Stack::reader_task(
            tun,
            shared.clone(),
            tuples_purge_tx.subscribe(),
            listening,
        ));

        Stack {
            shared,
            local_mac: local_mac.unwrap_or_default(),
            reader_task: AbortOnDropHandle::new(t),
        }
    }

    /// Returns the driver type of the stack.
    pub fn driver_type(&self) -> &'static str {
        self.shared.tun.driver_type()
    }

    pub fn is_closed(&self) -> bool {
        self.shared.is_closed() || self.reader_task.is_finished()
    }

    pub fn try_alloc_established_socket(
        &self,
        local_addr: SocketAddr,
        remote_addr: SocketAddr,
        state: State,
    ) -> Option<Socket> {
        let tuple = AddrTuple::new(local_addr, remote_addr);
        let mut stack_state = self.shared.state.write().unwrap();
        if stack_state.closed || self.reader_task.is_finished() {
            stack_state.closed = true;
            warn!(
                ?tuple,
                "fake_tcp stack is closed, refusing to allocate socket"
            );
            return None;
        }
        let (sock, incoming) = Socket::new(
            self.shared.clone(),
            // self.shared.tun.choose(&mut rng).unwrap().clone(),
            self.shared.tun.clone(), // Simplification: just use the first tun
            local_addr,
            remote_addr,
            self.local_mac,
            None,
            state,
        );
        if state == State::Established {
            stack_state.pending.retain(|(when, pending_tuple, frame)| {
                if when.elapsed() >= PENDING_FRAME_TTL {
                    return false;
                }
                if pending_tuple == &tuple {
                    let _ = incoming.try_send(frame.clone());
                    return false;
                }
                true
            });
        }
        assert!(stack_state.tuples.insert(tuple, incoming).is_none());
        Some(sock)
    }

    async fn reader_task(
        tun: Arc<dyn Tun>,
        shared: Arc<Shared>,
        mut tuples_purge: broadcast::Receiver<AddrTuple>,
        listening: bool,
    ) {
        let mut tuples: HashMap<AddrTuple, flume::Sender<Bytes>> = HashMap::new();

        loop {
            let mut buf = BytesMut::new();

            tokio::select! {
                size = tun.recv(&mut buf) => {
                    let size = match size {
                        Ok(size) => size,
                        Err(e) => {
                            let shared_tuple_count = shared.mark_closed_and_clear_tuples();
                            let cached_tuple_count = tuples.len();
                            tuples.clear();
                            error!(
                                ?e,
                                driver_type = tun.driver_type(),
                                shared_tuple_count,
                                cached_tuple_count,
                                "fake_tcp tun recv failed, reader_task exiting"
                            );
                            break;
                        }
                    };
                    tracing::trace!(len = size, ?buf, "PnetTun received packet");
                    let buf = buf.split().freeze();

                    match parse_ip_packet(&buf) {
                        Some((_src_mac, _dst_mac, ip_packet, tcp_packet)) => {
                            let local_addr = SocketAddr::new(
                                ip_packet.get_destination(),
                                tcp_packet.dst_port(),
                            );
                            let remote_addr = SocketAddr::new(
                                ip_packet.get_source(),
                                tcp_packet.src_port(),
                            );

                            let tuple = AddrTuple::new(local_addr, remote_addr);
                            if let Some(c) = tuples.get(&tuple) {
                                if c.send_async(buf).await.is_err() {
                                    trace!("Cache hit, but receiver already closed, dropping packet");
                                }

                                continue;

                                // If not Ok, receiver has been closed and just fall through to the slow
                                // path below
                            } else {
                                trace!("Cache miss, checking the shared tuples table for connection");
                                let sender = {
                                    let mut state = shared.state.write().unwrap();
                                    let sender = state.tuples.get(&tuple).cloned();
                                    if sender.is_none() && listening && tcp_packet.ack()
                                        && !tcp_packet.rst() && !tcp_packet.syn()
                                        && !tcp_packet.payload().is_empty() && buf.len() <= MAX_PENDING_FRAME_SIZE
                                    {
                                        // The raw reader can see the first data packet before the
                                        // OS accept future registers its connection tuple.
                                        if state.pending.len() == MAX_PENDING_FRAMES {
                                            state.pending.pop_front();
                                        }
                                        state.pending.push_back((time::Instant::now(), tuple, buf));
                                        continue;
                                    }
                                    sender
                                };

                                if let Some(c) = sender {
                                    trace!("Storing connection information into local tuples");
                                    tuples.insert(tuple, c.clone());
                                    if let Err(e) = c.send_async(buf).await {
                                        trace!("Error sending packet to connection: {:?}", e);
                                    }
                                    continue;
                                }
                            }

                            if tcp_packet.rst() {
                                info!("Unknown RST TCP packet from {}, ignoring", remote_addr);
                                continue;
                            } else {
                                trace!("Unknown TCP packet from {}, ignoring", remote_addr);
                                continue;
                            }
                        }
                        None => {
                            trace!("Dropping packet with no IP/TCP header");
                            continue;
                        }
                    }
                },
                tuple = tuples_purge.recv() => {
                    match tuple {
                        Ok(tuple) => {
                            tuples.remove(&tuple);
                            trace!("Removed cached tuple: {:?}", tuple);
                        }
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            let cached_tuple_count = tuples.len();
                            tuples.clear();
                            warn!(
                                skipped,
                                cached_tuple_count,
                                "fake_tcp tuples purge receiver lagged, cleared local cache"
                            );
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            let shared_tuple_count = shared.mark_closed_and_clear_tuples();
                            let cached_tuple_count = tuples.len();
                            tuples.clear();
                            warn!(
                                shared_tuple_count,
                                cached_tuple_count,
                                "fake_tcp tuples purge channel closed, reader_task exiting"
                            );
                            break;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io, sync::Mutex};
    use tokio::{
        sync::{Notify, mpsc},
        time::{Duration, timeout},
    };

    struct RecordingTun {
        incoming: tokio::sync::Mutex<mpsc::Receiver<Bytes>>,
        outgoing: Mutex<Vec<Bytes>>,
    }

    #[async_trait::async_trait]
    impl Tun for RecordingTun {
        async fn recv(&self, packet: &mut BytesMut) -> Result<usize, io::Error> {
            let frame = self.incoming.lock().await.recv().await.unwrap();
            packet.extend_from_slice(&frame);
            Ok(frame.len())
        }

        fn try_send(&self, packet: &Bytes) -> Result<(), io::Error> {
            self.outgoing.lock().unwrap().push(packet.clone());
            Ok(())
        }

        fn driver_type(&self) -> &'static str {
            "test"
        }
    }

    #[tokio::test]
    async fn first_payload_before_accept_seeds_tcp_sequence_and_late_ack_cannot_rewind_it() {
        let (input, incoming) = mpsc::channel(4);
        let tun = Arc::new(RecordingTun {
            incoming: tokio::sync::Mutex::new(incoming),
            outgoing: Mutex::new(Vec::new()),
        });
        let local_addr = "192.0.2.10:22024".parse().unwrap();
        let remote_addr = "192.0.2.20:40000".parse().unwrap();
        let frame = |seq, ack, payload| {
            build_tcp_packet(
                MacAddr::default(),
                MacAddr::default(),
                remote_addr,
                local_addr,
                seq,
                ack,
                TCP_FLAG_ACK,
                payload,
            )
        };
        let stack = Stack::new(tun.clone(), None, true);
        input
            .send(frame(u32::MAX - 2, 901, Some(b"abc")))
            .await
            .unwrap();
        timeout(Duration::from_secs(1), async {
            while stack.shared.state.read().unwrap().pending.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let socket = stack
            .try_alloc_established_socket(local_addr, remote_addr, State::Established)
            .unwrap();
        let mut received = BytesMut::new();
        assert_eq!(
            timeout(Duration::from_secs(1), socket.recv(&mut received))
                .await
                .unwrap(),
            Some(3)
        );
        assert_eq!(received.as_ref(), b"abc");
        assert_eq!(socket.try_send(b"reply"), Some(()));

        input.send(frame(u32::MAX - 1, 901, None)).await.unwrap();
        input.send(frame(0, 901, Some(b"def"))).await.unwrap();
        received.clear();
        assert_eq!(
            timeout(Duration::from_secs(1), socket.recv(&mut received))
                .await
                .unwrap(),
            Some(3)
        );
        assert_eq!(received.as_ref(), b"def");
        assert_eq!(socket.try_send(b"next"), Some(()));

        let sent = tun.outgoing.lock().unwrap();
        let (_, _, _, first) = parse_ip_packet(&sent[0]).unwrap();
        let (_, _, _, second) = parse_ip_packet(&sent[1]).unwrap();
        assert_eq!((first.seq_number().0, first.ack_number().0), (901, 0));
        assert_eq!((second.seq_number().0, second.ack_number().0), (906, 3));
    }

    #[test]
    fn tcp_option_iterator_preserves_all_sack_blocks() {
        for block_count in 1..=4_u32 {
            let mut options = vec![TCP_OPTION_NOP, TCP_OPTION_SACK, (2 + block_count * 8) as u8];
            for block in 0..block_count {
                options.extend_from_slice(&(100 + block * 10).to_be_bytes());
                options.extend_from_slice(&(110 + block * 10).to_be_bytes());
            }
            options.push(TCP_OPTION_END);

            let parsed = TcpOptionIter::new(&options).collect::<Vec<_>>();
            assert_eq!(parsed[0], (TCP_OPTION_NOP, &[][..]));
            assert_eq!(parsed[1].0, TCP_OPTION_SACK);
            assert_eq!(parsed[1].1.len(), block_count as usize * 8);
            let last = parsed[1].1.len() - 8;
            assert_eq!(
                u32::from_be_bytes(parsed[1].1[last..last + 4].try_into().unwrap()),
                100 + (block_count - 1) * 10
            );
            assert_eq!(
                u32::from_be_bytes(parsed[1].1[last + 4..last + 8].try_into().unwrap()),
                110 + (block_count - 1) * 10
            );
        }
    }

    #[test]
    fn tcp_option_iterator_stops_at_end_or_malformed_length() {
        assert_eq!(
            TcpOptionIter::new(&[TCP_OPTION_END, TCP_OPTION_SACK, 2]).count(),
            0
        );
        assert_eq!(TcpOptionIter::new(&[TCP_OPTION_SACK]).count(), 0);
        assert_eq!(TcpOptionIter::new(&[TCP_OPTION_SACK, 1]).count(), 0);
        assert_eq!(TcpOptionIter::new(&[TCP_OPTION_SACK, 10, 0, 0]).count(), 0);
    }

    #[derive(Default)]
    struct FailingTun {
        fail: Notify,
    }

    impl FailingTun {
        fn fail(&self) {
            self.fail.notify_one();
        }
    }

    #[async_trait::async_trait]
    impl Tun for FailingTun {
        async fn recv(&self, _packet: &mut BytesMut) -> Result<usize, io::Error> {
            self.fail.notified().await;
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "test tun closed"))
        }

        fn try_send(&self, _packet: &Bytes) -> Result<(), io::Error> {
            Ok(())
        }

        fn driver_type(&self) -> &'static str {
            "test"
        }
    }

    #[tokio::test]
    async fn reader_task_closes_sockets_on_tun_recv_error() {
        let tun = Arc::new(FailingTun::default());
        let mut stack = Stack::new(tun.clone(), None, false);
        let socket = stack
            .try_alloc_established_socket(
                SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 10_000),
                SocketAddr::new(Ipv4Addr::new(192, 0, 2, 1).into(), 20_000),
                State::Established,
            )
            .expect("socket allocation should succeed before tun failure");

        tun.fail();

        let join_result = timeout(Duration::from_secs(1), &mut stack.reader_task)
            .await
            .expect("reader task should exit after tun recv error");
        assert!(join_result.is_ok());
        assert!(stack.is_closed());

        let mut buf = BytesMut::new();
        let recv_result = timeout(Duration::from_secs(1), socket.recv(&mut buf))
            .await
            .expect("socket recv should not hang after reader task exits");
        assert_eq!(recv_result, None);

        let new_socket = stack.try_alloc_established_socket(
            SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 10_001),
            SocketAddr::new(Ipv4Addr::new(192, 0, 2, 1).into(), 20_001),
            State::Established,
        );
        assert!(new_socket.is_none());

        drop(socket);
    }
}
