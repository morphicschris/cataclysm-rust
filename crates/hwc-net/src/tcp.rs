//! Connections over TCP, without blocking: a host listening for clients, and a client's
//! connection to it. Messages are framed by [`crate::wire`].

use std::io::{self, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::wire::{self, Decoder, Msg};

/// One connection.
#[derive(Debug)]
pub struct Conn {
    stream: TcpStream,
    decoder: Decoder,
    out: Vec<u8>,
    closed: bool,
}

impl Conn {
    pub fn new(stream: TcpStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        stream.set_nodelay(true)?;
        Ok(Conn { stream, decoder: Decoder::default(), out: Vec::new(), closed: false })
    }

    /// Connects to `addr`, waiting at most `timeout`.
    pub fn connect(addr: impl ToSocketAddrs, timeout: Duration) -> io::Result<Self> {
        let mut last = io::Error::new(ErrorKind::NotFound, "no address");
        for a in addr.to_socket_addrs()? {
            match TcpStream::connect_timeout(&a, timeout) {
                Ok(s) => return Conn::new(s),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    pub fn peer(&self) -> Option<SocketAddr> {
        self.stream.peer_addr().ok()
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Queues a message; [`Conn::flush`] sends it.
    pub fn send(&mut self, msg: &Msg) {
        self.out.extend_from_slice(&wire::encode(msg));
    }

    /// Sends what the socket takes now.
    pub fn flush(&mut self) {
        while !self.out.is_empty() && !self.closed {
            match self.stream.write(&self.out) {
                Ok(0) => self.closed = true,
                Ok(n) => {
                    self.out.drain(..n);
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => self.closed = true,
            }
        }
    }

    /// The messages that have come; the connection closes on an error or a bad message.
    pub fn poll(&mut self) -> Vec<Msg> {
        let mut buf = [0u8; 16384];
        while !self.closed {
            match self.stream.read(&mut buf) {
                Ok(0) => self.closed = true,
                Ok(n) => self.decoder.push(&buf[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => self.closed = true,
            }
        }
        let mut out = Vec::new();
        while let Some(m) = self.decoder.next() {
            match m {
                Ok(m) => out.push(m),
                Err(_) => {
                    self.closed = true;
                    break;
                }
            }
        }
        out
    }

    /// Sends what is queued, waiting up to `timeout` (for a last message before closing).
    pub fn flush_blocking(&mut self, timeout: Duration) {
        let end = std::time::Instant::now() + timeout;
        while !self.out.is_empty() && !self.closed && std::time::Instant::now() < end {
            self.flush();
            if !self.out.is_empty() {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

/// A host's listening socket.
#[derive(Debug)]
pub struct Listener {
    listener: TcpListener,
}

impl Listener {
    pub fn bind(addr: impl ToSocketAddrs) -> io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        listener.set_nonblocking(true)?;
        Ok(Listener { listener })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// The clients that have connected since the last call.
    pub fn accept(&self) -> Vec<Conn> {
        let mut out = Vec::new();
        loop {
            match self.listener.accept() {
                Ok((s, _)) => {
                    if let Ok(c) = Conn::new(s) {
                        out.push(c);
                    }
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        out
    }
}
