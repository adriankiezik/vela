//! Vela wire protocol — Minecraft Java Edition, protocol version 776 (MC 26.2).

pub mod buffer;
pub mod framing;
pub mod nbt;
pub mod text;
pub mod uuid;
pub mod varint;

/// Network protocol version advertised by MC 26.2.
/// Source: decompiled `SharedConstants.RELEASE_NETWORK_PROTOCOL_VERSION`.
pub const PROTOCOL_VERSION: i32 = 776;

/// The human-readable version string sent in the status response.
pub const VERSION_NAME: &str = "26.2";

/// Connection states. After the handshake the client requests one of
/// STATUS or LOGIN; LOGIN leads through CONFIGURATION into PLAY.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Handshake,
    Status,
    Login,
    Configuration,
    // Reached by handing the connection to `connection::play`, which owns the
    // split stream rather than looping on this enum — hence never constructed.
    #[allow(dead_code)]
    Play,
}

/// Client intent carried in the handshake packet.
/// Source: decompiled `handshake.ClientIntent` (STATUS=1, LOGIN=2, TRANSFER=3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    Status,
    Login,
    Transfer,
}

/// The error from [`Intent::try_from`]: the handshake carried an intent id
/// outside the known `STATUS`/`LOGIN`/`TRANSFER` range. Carries the bad value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownIntent(pub i32);

impl TryFrom<i32> for Intent {
    type Error = UnknownIntent;

    /// Decode the handshake intent id; an out-of-range value is the sole error.
    fn try_from(id: i32) -> Result<Self, UnknownIntent> {
        match id {
            1 => Ok(Intent::Status),
            2 => Ok(Intent::Login),
            3 => Ok(Intent::Transfer),
            _ => Err(UnknownIntent(id)),
        }
    }
}
