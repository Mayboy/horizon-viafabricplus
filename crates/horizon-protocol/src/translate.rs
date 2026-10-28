//! A hook for playing on a server of another protocol version: a
//! [`Translator`] sits between the frames on the wire and the packets the
//! game reads and writes, rewriting each packet body (after decompression,
//! before encryption) from the server's version to this client's and back.
//! `horizon-multiversion` provides the translators.
use crate::Result;
use std::sync::{Arc, Mutex};

/// One packet body: its id and payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub id: i32,
    pub data: Vec<u8>,
}
impl Packet {
    pub fn new(id: i32, data: Vec<u8>) -> Self {
        Self { id, data }
    }
}

pub trait Translator: Send {
    /// A packet the server sent, in its version: the packets this client
    /// reads in its place, in order (none drops it).
    fn clientbound(&mut self, packet: Packet, out: &mut Vec<Packet>) -> Result<()>;
    /// A packet this client sends: the packets the server receives in its place.
    fn serverbound(&mut self, packet: Packet, out: &mut Vec<Packet>) -> Result<()>;
    /// Packets for this client that translating serverbound packets made
    /// (a newer client expects the server to send them).
    fn take_clientbound(&mut self) -> Vec<Packet>;
}

/// One translator shared by a connection's reader and writer.
pub type Shared = Arc<Mutex<Box<dyn Translator>>>;
