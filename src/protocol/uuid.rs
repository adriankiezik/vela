//! Offline-mode player UUIDs.
//!
//! Vanilla derives them as a version-3 (MD5) name-based UUID over the
//! bytes of `"OfflinePlayer:" + name` — see decompiled
//! `net.minecraft.core.UUIDUtil.createOfflinePlayerUUID`.
//!
//! Note: this is *not* `Uuid::new_v3`, which hashes a namespace UUID
//! prepended to the name. Vanilla hashes the bare bytes, so we run MD5
//! ourselves (via the `md-5` crate) and set the version/variant bits the
//! way `java.util.UUID.nameUUIDFromBytes` does.

use md5::{Digest, Md5};
use uuid::Uuid;

/// `UUID.nameUUIDFromBytes("OfflinePlayer:<name>")`.
pub fn offline_uuid(name: &str) -> Uuid {
    let digest = Md5::digest(format!("OfflinePlayer:{name}").as_bytes());
    let mut bytes: [u8; 16] = digest.into();
    bytes[6] = (bytes[6] & 0x0f) | 0x30; // version 3
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // IETF variant
    Uuid::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_and_variant_bits() {
        let id = offline_uuid("Notch");
        assert_eq!(id.get_version_num(), 3, "must be a v3 (MD5) UUID");
        // IETF variant: top two bits of byte 8 are 0b10.
        assert_eq!(id.as_bytes()[8] & 0xc0, 0x80);
    }

    #[test]
    fn stable_for_same_name() {
        assert_eq!(offline_uuid("Steve"), offline_uuid("Steve"));
        assert_ne!(offline_uuid("Steve"), offline_uuid("Alex"));
    }
}
