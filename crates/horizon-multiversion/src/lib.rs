//! Joining servers of other Minecraft versions, the way ViaFabricPlus does
//! for the Fabric client: a chain of steps, each translating every packet
//! between two adjacent protocol versions (ViaVersion's protocol chain),
//! between the bytes on the wire and the 26.3 packets Horizon reads and writes.
//!
//! A [`Pipeline`] runs the chain for one connection. It tracks the connection
//! state from the packets that switch it, rewrites the handshake's protocol
//! number, and hands each packet to the steps from the server's version up to
//! this client's (clientbound) or back down (serverbound). A step may drop a
//! packet, split it, or answer for the other side (a newer client expects
//! packets an older server never sends).
#![forbid(unsafe_code)]
use horizon_protocol::{
    Error, Result, State, ids,
    translate::{Packet, Shared, Translator},
};
use std::sync::{Arc, Mutex};

mod v26_2;
mod wire;

/// A protocol version Horizon can join.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Version {
    pub protocol: i32,
    pub name: &'static str,
}
/// This client's own version: no translation.
pub const NATIVE: Version = Version {
    protocol: horizon_protocol::PROTOCOL_VERSION,
    name: horizon_protocol::GAME_VERSION,
};
/// Every version Horizon can join, newest first.
pub const VERSIONS: &[Version] = &[
    NATIVE,
    Version {
        protocol: 776,
        name: "26.2",
    },
];

/// The supported version with this protocol number.
pub fn version(protocol: i32) -> Option<Version> {
    VERSIONS.iter().copied().find(|v| v.protocol == protocol)
}
/// The supported version with this name ("26.2").
pub fn by_name(name: &str) -> Option<Version> {
    VERSIONS.iter().copied().find(|v| v.name == name)
}

/// A translator to a server of `protocol`, or None when that is this
/// client's own version or one Horizon cannot join.
pub fn translator(protocol: i32) -> Option<Shared> {
    if protocol == NATIVE.protocol {
        return None;
    }
    let steps = steps_to(protocol)?;
    let pipeline: Box<dyn Translator> = Box::new(Pipeline::new(protocol, steps));
    Some(Arc::new(Mutex::new(pipeline)))
}

/// The steps between `protocol` and this client, the server's side first.
fn steps_to(protocol: i32) -> Option<Vec<Box<dyn Step>>> {
    let mut steps: Vec<Box<dyn Step>> = Vec::new();
    // Each older version adds its step in front of the newer ones.
    match protocol {
        776 => steps.push(Box::new(v26_2::Step::default())),
        _ => return None,
    }
    Some(steps)
}

/// What one step makes of one packet.
#[derive(Default)]
pub(crate) struct Out {
    /// The packets that go on in the same direction, in the next version.
    pub packets: Vec<Packet>,
    /// Packets for the client, in the version on the step's client side
    /// (from a serverbound packet).
    pub to_client: Vec<Packet>,
    /// Packets for the server, in the version on the step's server side
    /// (from a clientbound packet).
    pub to_server: Vec<Packet>,
}
impl Out {
    pub fn push(&mut self, id: i32, data: Vec<u8>) {
        self.packets.push(Packet::new(id, data));
    }
}

/// One protocol step: an older version on its server side, the next newer
/// one on its client side.
pub(crate) trait Step: Send {
    /// A packet from the server side, in the connection `state`.
    fn clientbound(&mut self, state: State, packet: Packet, out: &mut Out) -> Result<()>;
    /// A packet from the client side.
    fn serverbound(&mut self, state: State, packet: Packet, out: &mut Out) -> Result<()>;
}

/// The step chain for one connection.
pub struct Pipeline {
    protocol: i32,
    /// The server's side first.
    steps: Vec<Box<dyn Step>>,
    /// The state of each direction: they switch at different packets.
    inbound: State,
    outbound: State,
    /// Packets for the client made from serverbound ones, not read yet.
    to_client: Vec<Packet>,
    /// Packets for the server made from clientbound ones, not sent yet.
    to_server: Vec<Packet>,
    /// Packets that failed to translate, logged once each.
    failed: std::collections::HashSet<(State, i32)>,
}
impl Pipeline {
    fn new(protocol: i32, steps: Vec<Box<dyn Step>>) -> Self {
        Self {
            protocol,
            steps,
            inbound: State::Handshake,
            outbound: State::Handshake,
            to_client: Vec::new(),
            to_server: Vec::new(),
            failed: Default::default(),
        }
    }

    /// Run packets through the steps from `from` on, towards the client.
    fn up(&mut self, state: State, from: usize, mut packets: Vec<Packet>) -> Vec<Packet> {
        for i in from..self.steps.len() {
            let mut next = Vec::with_capacity(packets.len());
            for p in packets {
                let id = p.id;
                let mut out = Out::default();
                if let Err(e) = self.steps[i].clientbound(state, p, &mut out) {
                    self.report(state, id, "clientbound", &e);
                    continue;
                }
                next.extend(out.packets);
                if !out.to_server.is_empty() {
                    let back = self.down(state, i, out.to_server);
                    self.to_server.extend(back);
                }
            }
            packets = next;
        }
        packets
    }

    /// Run packets through the steps below `below`, towards the server.
    fn down(&mut self, state: State, below: usize, mut packets: Vec<Packet>) -> Vec<Packet> {
        for i in (0..below).rev() {
            let mut next = Vec::with_capacity(packets.len());
            for p in packets {
                let id = p.id;
                let mut out = Out::default();
                if let Err(e) = self.steps[i].serverbound(state, p, &mut out) {
                    self.report(state, id, "serverbound", &e);
                    continue;
                }
                next.extend(out.packets);
                if !out.to_client.is_empty() {
                    let inbound = self.inbound;
                    let forth = self.up(inbound, i + 1, out.to_client);
                    self.to_client.extend(forth);
                }
            }
            packets = next;
        }
        packets
    }

    fn report(&mut self, state: State, id: i32, direction: &str, e: &Error) {
        if self.failed.insert((state, id)) {
            eprintln!(
                "[multiversion] dropped a {direction} {state:?} packet {id} (server protocol {}): {e}",
                self.protocol
            );
        }
    }

    /// The handshake names the server's protocol instead of ours.
    fn handshake(&mut self, packet: Packet) -> Result<Packet> {
        let mut r = horizon_protocol::buf::Reader::new(&packet.data);
        r.varint()?;
        let host = r.string(255)?;
        let port = r.u16()?;
        let intent = r.varint()?;
        let state = if intent == horizon_protocol::packets::handshake::STATUS {
            State::Status
        } else {
            State::Login
        };
        self.inbound = state;
        self.outbound = state;
        let mut w = horizon_protocol::buf::Writer::new();
        w.varint(self.protocol);
        w.string(&host);
        w.u16(port);
        w.varint(intent);
        Ok(Packet::new(packet.id, w.data))
    }
}

impl Translator for Pipeline {
    fn clientbound(&mut self, packet: Packet, out: &mut Vec<Packet>) -> Result<()> {
        let state = self.inbound;
        for p in self.up(state, 0, vec![packet]) {
            // The client switches as it reads these (26.3 ids from here).
            self.inbound = match (state, p.id) {
                (State::Login, ids::login::clientbound::LOGIN_FINISHED) => State::Configuration,
                (State::Configuration, ids::configuration::clientbound::FINISH_CONFIGURATION) => {
                    State::Play
                }
                (State::Play, ids::play::clientbound::START_CONFIGURATION) => {
                    State::Configuration
                }
                _ => self.inbound,
            };
            out.push(p);
        }
        Ok(())
    }

    fn serverbound(&mut self, packet: Packet, out: &mut Vec<Packet>) -> Result<()> {
        let state = self.outbound;
        if state == State::Handshake {
            out.push(self.handshake(packet)?);
            return Ok(());
        }
        // The client switches as it sends these.
        self.outbound = match (state, packet.id) {
            (State::Login, ids::login::serverbound::LOGIN_ACKNOWLEDGED) => State::Configuration,
            (State::Configuration, ids::configuration::serverbound::FINISH_CONFIGURATION) => {
                State::Play
            }
            (State::Play, ids::play::serverbound::CONFIGURATION_ACKNOWLEDGED) => {
                State::Configuration
            }
            _ => state,
        };
        let below = self.steps.len();
        let packets = self.down(state, below, vec![packet]);
        out.append(&mut self.to_server);
        out.extend(packets);
        Ok(())
    }

    fn take_clientbound(&mut self) -> Vec<Packet> {
        std::mem::take(&mut self.to_client)
    }
}
