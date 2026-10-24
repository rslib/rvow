//! The version of the protocol the wire types speak.

/// The major protocol version: a peer with another major cannot speak to us.
pub const PROTO_MAJOR: u32 = 1;
/// The minor protocol version: a later minor only adds fields.
pub const PROTO_MINOR: u32 = 0;
