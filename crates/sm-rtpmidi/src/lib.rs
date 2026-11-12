//! # sm-rtpmidi
//!
//! A small, self-contained RTP-MIDI ("AppleMIDI" / "Network MIDI") implementation
//! built only on [`std::net::UdpSocket`] and std threads. It needs no OS MIDI
//! driver, no Bonjour/mDNS service and no async runtime, and behaves identically
//! on macOS, Windows and Linux.
//!
//! ## What is implemented
//!
//! * **AppleMIDI session protocol** ([`applemidi`]): `IN` / `OK` / `NO` / `BY`
//!   invitation exchange (protocol version 2), `CK` three-way clock
//!   synchronisation, `RS` receiver feedback, `RL` bitrate limit (parsed and
//!   ignored).
//! * **RTP-MIDI payload, RFC 6295** ([`rtp`]): RTP header, MIDI command section
//!   (B/J/Z/P flags, short and long length), delta times, running status on
//!   receive, SysEx including segmented SysEx (reassembled on receive, segmented
//!   on send when a message does not fit into one packet).
//! * **Recovery journal, sender side** ([`journal`]): channel chapters P (program
//!   change), C (control change) and N (note on/off), with the checkpoint advanced
//!   by `RS` feedback. Opt-in through [`SessionConfig::journal`]; the default is
//!   `false`, i.e. packets are sent with `J = 0`, which every known
//!   implementation accepts.
//! * **Recovery journal, receiver side** ([`recovery`]): always active,
//!   independent of our own sender setting. When a sequence gap is detected and
//!   the packet ending the loss carries a journal, chapters P, C and N are
//!   compared with the per-peer receiver state (program/bank, controller values,
//!   sounding notes) and the missing note-offs, controller values and program
//!   changes - and NoteOns the sender marked as recent (Y=1) - are delivered
//!   through the receiver callback *before* the packet's own commands. Counted
//!   in [`PeerInfo::recovered`]. Stale NoteOns (Y=0) are never replayed.
//! * **[`Session`]**: binds a control port `P` and a data port `P + 1`, accepts
//!   invitations (responder), initiates connections ([`Session::connect`]) with
//!   retry and optional automatic reconnect, keeps clocks in sync, times out
//!   silent peers, and sends/receives MIDI.
//!
//! ## Limitations (deliberate)
//!
//! * **No discovery in this crate.** Peers connect by `IP:port`. mDNS/Bonjour
//!   (`_apple-midi._udp`) advertisement and browsing live in the separate
//!   `sm-mdns` crate (optional comfort); a macOS/iOS/rtpMIDI peer can always be
//!   pointed at this session manually, or this session can [`Session::connect`]
//!   to it.
//! * **Journal chapters:** only P, C and N are emitted. Pitch bend, aftertouch,
//!   system messages (incl. MTC) and SysEx are not journalled. Chapter C always
//!   uses the plain 7-bit value tool; RPN/NRPN are not treated specially
//!   (no chapter M).
//! * **Recovery on receive** evaluates only chapters P, C (value tool) and N.
//!   Lost pitch bend, aftertouch, RPN/NRPN (chapter M), system messages and
//!   SysEx are not repaired; the system journal is skipped. Controller logs
//!   using the toggle/count tools and channel mode controllers (120-127) are
//!   not replayed. The journal of the first packet of a link is not evaluated
//!   (no history to compare with; replaying it could fire stale commands).
//!   Packet loss is counted in [`PeerInfo::packets_lost`] in any case.
//! * **Timestamps:** all messages of one [`Session::send`] call carry the session
//!   time of the call (delta time 0). The session clock runs at 10 kHz
//!   (100 µs units) like Apple's implementation. Received timestamps and delta
//!   times are parsed but incoming MIDI is delivered immediately.
//! * Outgoing packets never use running status and are never padded.
//! * If two sessions invite each other simultaneously, both links are kept
//!   (each side lists two peers); MIDI is still only sent once per remote SSRC.
//!
//! ## Example
//!
//! ```no_run
//! use sm_rtpmidi::{Session, SessionConfig};
//!
//! # fn main() -> Result<(), sm_rtpmidi::Error> {
//! let session = Session::start(SessionConfig::new("Show Master"))?;
//! session.connect("192.168.1.20:5004".parse().map_err(|_| sm_rtpmidi::Error::Closed)?)?;
//! session.send(&[0x90, 60, 100])?; // note on, to every connected peer
//! # Ok(())
//! # }
//! ```

pub mod applemidi;
pub mod journal;
pub mod recovery;
pub mod rtp;
mod session;

pub use session::{MidiReceiver, PeerInfo, PeerState, Session, SessionConfig};

/// Errors of the pure wire codecs ([`applemidi`], [`rtp`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CodecError {
    /// The packet ended before a complete structure could be read.
    #[error("packet truncated")]
    Truncated,
    /// An AppleMIDI packet did not start with `0xFFFF`.
    #[error("missing AppleMIDI signature")]
    BadSignature,
    /// Unknown two-letter AppleMIDI command.
    #[error("unknown AppleMIDI command 0x{0:04X}")]
    UnknownCommand(u16),
    /// RTP version field was not 2.
    #[error("unsupported RTP version {0}")]
    BadRtpVersion(u8),
    /// A data byte appeared where running status was required but none was active.
    #[error("data byte without running status")]
    NoRunningStatus,
    /// A status byte appeared where a data byte was expected.
    #[error("unexpected status byte 0x{0:02X} inside a MIDI command")]
    UnexpectedStatus(u8),
    /// A delta time used more than four octets.
    #[error("delta time longer than four octets")]
    DeltaTooLong,
    /// The MIDI list does not fit the 12-bit length field.
    #[error("MIDI list of {0} bytes exceeds the 4095 byte limit")]
    ListTooLong(usize),
}

/// Errors returned by [`Session`].
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Socket error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The control/data port pair could not be bound.
    #[error("cannot bind RTP-MIDI port pair starting at {port}: {source}")]
    Bind {
        /// Requested control port (0 = automatic).
        port: u16,
        /// Underlying socket error.
        source: std::io::Error,
    },
    /// The bytes passed to [`Session::send`] are not complete MIDI messages.
    #[error("invalid MIDI data: {0}")]
    InvalidMidi(&'static str),
    /// The remote address cannot be used (its data port would be > 65535).
    #[error("invalid peer address {0}")]
    InvalidAddress(std::net::SocketAddr),
    /// The configured maximum number of peers is reached.
    #[error("maximum number of peers reached")]
    TooManyPeers,
    /// The session has been closed.
    #[error("session is closed")]
    Closed,
    /// Wire encoding failed.
    #[error(transparent)]
    Codec(#[from] CodecError),
}
