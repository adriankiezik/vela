//! Online-mode cryptography: the RSA login key exchange, the AES-CFB8 stream
//! cipher, the Minecraft server-id auth hash, and the Mojang `hasJoined` call.
//!
//! Reference: decompiled `net.minecraft.util.Crypt` and
//! `ServerLoginPacketListenerImpl` (MC 26.2). The wire/auth math is kept 1:1:
//!   * RSA 1024-bit keypair, public key encoded as X.509 `SubjectPublicKeyInfo`
//!     DER (Java's `PublicKey.getEncoded()`), `RSA/ECB/PKCS1Padding` for the
//!     `Cipher.getInstance("RSA")` default used to wrap the shared secret;
//!   * the server-id hash is `SHA-1(serverId ++ secret ++ pubKeyDer)` rendered
//!     as Java's `BigInteger.toString(16)` — a *signed* hex string;
//!   * the stream cipher is `AES/CFB8/NoPadding` with the IV equal to the key.

use std::sync::OnceLock;

use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockEncrypt, KeyInit};
use aes::Aes128;
use rand::RngCore;
use rsa::pkcs8::EncodePublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPrivateKey, RsaPublicKey};
use sha1::{Digest, Sha1};
use uuid::Uuid;

/// The server's RSA keypair, generated once and shared by every online-mode
/// login. Vanilla generates a single 1024-bit keypair at server startup
/// (`MinecraftServer.keyPair`); we do the same, lazily on first use.
pub struct ServerKeys {
    private: RsaPrivateKey,
    /// The public key as X.509 SPKI DER — exactly the bytes sent in
    /// `ClientboundHelloPacket` and folded into the server-id hash.
    public_der: Vec<u8>,
}

impl ServerKeys {
    fn generate() -> Self {
        let mut rng = rand::thread_rng();
        // 1024 bits, matching `Crypt.generateKeyPair` (ASYMMETRIC_BITS).
        let private = RsaPrivateKey::new(&mut rng, 1024).expect("RSA keygen");
        let public = RsaPublicKey::from(&private);
        let public_der = public
            .to_public_key_der()
            .expect("encode RSA public key as SPKI DER")
            .as_bytes()
            .to_vec();
        Self {
            private,
            public_der,
        }
    }

    /// The X.509 SPKI DER public key bytes for `ClientboundHelloPacket`.
    pub fn public_der(&self) -> &[u8] {
        &self.public_der
    }

    /// Decrypt an RSA-`PKCS1`-wrapped blob (the shared secret or the verify
    /// token) with the server's private key. Mirrors `Crypt.decryptUsingKey`,
    /// whose `Cipher.getInstance("RSA")` defaults to `RSA/ECB/PKCS1Padding`.
    pub fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>, AuthError> {
        self.private
            .decrypt(Pkcs1v15Encrypt, data)
            .map_err(|_| AuthError::Crypt)
    }
}

/// Lazily-initialized process-global server keypair.
pub fn server_keys() -> &'static ServerKeys {
    static KEYS: OnceLock<ServerKeys> = OnceLock::new();
    KEYS.get_or_init(ServerKeys::generate)
}

/// A fresh 4-byte verify token (Java's `Ints.toByteArray(random.nextInt())`).
pub fn new_verify_token() -> [u8; 4] {
    let mut token = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut token);
    token
}

/// The Minecraft server-id hash: `SHA-1(serverId ++ secret ++ pubKeyDer)`
/// rendered as Java's `BigInteger.toString(16)` (a two's-complement *signed*
/// hex string). `serverId` is empty on a modern server.
pub fn server_id_hash(server_id: &str, secret: &[u8], public_der: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(server_id.as_bytes());
    hasher.update(secret);
    hasher.update(public_der);
    let digest: [u8; 20] = hasher.finalize().into();
    signed_hex(digest)
}

/// Render a 20-byte SHA-1 digest the way Java's `new BigInteger(bytes).toString(16)`
/// does: treat it as a big-endian *signed* integer. A leading sign bit yields a
/// negative number printed with a `-` and its two's-complement magnitude, and
/// leading zero nibbles are dropped.
fn signed_hex(mut bytes: [u8; 20]) -> String {
    let negative = bytes[0] & 0x80 != 0;
    if negative {
        // Two's complement: invert all bytes and add one.
        let mut carry = true;
        for b in bytes.iter_mut().rev() {
            *b = !*b;
            if carry {
                let (v, c) = b.overflowing_add(1);
                *b = v;
                carry = c;
            }
        }
    }
    let mut hex = String::with_capacity(41);
    if negative {
        hex.push('-');
    }
    let mut started = false;
    for b in bytes {
        if !started {
            if b == 0 {
                continue;
            }
            // First non-zero byte: skip a leading zero nibble.
            if b < 0x10 {
                hex.push(char::from_digit(b as u32, 16).unwrap());
                started = true;
                continue;
            }
            started = true;
        }
        hex.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        hex.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
    }
    if !started {
        hex.push('0');
    }
    hex
}

/// An `AES/CFB8/NoPadding` stream cipher in one direction, driven by hand over
/// the AES block cipher. CFB8 feeds the previous ciphertext byte back into the
/// IV one byte at a time, so the state persists across packets — exactly the
/// continuous stream Minecraft expects. The IV starts equal to the key
/// (`Crypt.getCipher`'s `new IvParameterSpec(key.getEncoded())`).
pub struct Cfb8 {
    cipher: Aes128,
    iv: [u8; 16],
}

impl Cfb8 {
    pub fn new(key: &[u8; 16]) -> Self {
        Self {
            cipher: Aes128::new(GenericArray::from_slice(key)),
            iv: *key,
        }
    }

    /// Encrypt `data` in place, advancing the cipher state.
    pub fn encrypt(&mut self, data: &mut [u8]) {
        for byte in data.iter_mut() {
            let keystream = self.keystream_byte();
            let c = *byte ^ keystream;
            self.feedback(c);
            *byte = c;
        }
    }

    /// Decrypt `data` in place, advancing the cipher state.
    pub fn decrypt(&mut self, data: &mut [u8]) {
        for byte in data.iter_mut() {
            let keystream = self.keystream_byte();
            let c = *byte; // the ciphertext is what feeds back, not the plaintext
            self.feedback(c);
            *byte = c ^ keystream;
        }
    }

    /// AES-encrypt the current IV block and take its first byte as keystream.
    fn keystream_byte(&self) -> u8 {
        let mut block = GenericArray::clone_from_slice(&self.iv);
        self.cipher.encrypt_block(&mut block);
        block[0]
    }

    /// Shift the IV left one byte and append the latest ciphertext byte.
    fn feedback(&mut self, c: u8) {
        self.iv.copy_within(1..16, 0);
        self.iv[15] = c;
    }
}

/// A profile resolved against the Mojang session server.
pub struct AuthProfile {
    pub uuid: Uuid,
    pub name: String,
    /// Signed properties (skin/cape) verbatim from `hasJoined`, to forward in
    /// `ClientboundLoginFinished`.
    pub properties: Vec<ProfileProperty>,
}

pub struct ProfileProperty {
    pub name: String,
    pub value: String,
    pub signature: Option<String>,
}

#[derive(Debug)]
pub enum AuthError {
    /// RSA/AES failure (bad ciphertext, wrong key) — a protocol error.
    Crypt,
    /// The verify token the client returned did not match the one we issued.
    BadVerifyToken,
    /// The session server returned no matching profile (invalid session).
    Unverified,
    /// The session server could not be reached.
    Unavailable,
}

/// Call Mojang's `hasJoined` to confirm the client authenticated against this
/// login and to fetch the real, signed `GameProfile`. Blocking; run it off the
/// async runtime via `spawn_blocking`. Mirrors `YggdrasilMinecraftSessionService`.
///
/// `ip` is supplied only when `prevent-proxy-connections` is set, matching
/// vanilla's optional `&ip=` parameter.
pub fn has_joined(name: &str, server_hash: &str, ip: Option<&str>) -> Result<AuthProfile, AuthError> {
    let mut url = format!(
        "https://sessionserver.mojang.com/session/minecraft/hasJoined?username={}&serverId={}",
        urlencode(name),
        urlencode(server_hash)
    );
    if let Some(ip) = ip {
        url.push_str("&ip=");
        url.push_str(&urlencode(ip));
    }

    let resp = match ureq::get(&url).call() {
        Ok(resp) => resp,
        // A 204 (no content) surfaces as a 2xx with empty body, not an error;
        // genuine transport failures land here.
        Err(ureq::Error::Status(_, resp)) => resp,
        Err(ureq::Error::Transport(_)) => return Err(AuthError::Unavailable),
    };

    // No body (HTTP 204) means the client did not authenticate for this server.
    let body = resp.into_string().map_err(|_| AuthError::Unavailable)?;
    if body.trim().is_empty() {
        return Err(AuthError::Unverified);
    }

    parse_profile(&body).ok_or(AuthError::Unverified)
}

/// Parse the `hasJoined` JSON: `{ id, name, properties: [{name,value,signature?}] }`.
/// `id` is the undashed 32-hex-char UUID.
fn parse_profile(body: &str) -> Option<AuthProfile> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let uuid = Uuid::parse_str(json.get("id")?.as_str()?).ok()?;
    let name = json.get("name")?.as_str()?.to_string();
    let mut properties = Vec::new();
    if let Some(arr) = json.get("properties").and_then(|p| p.as_array()) {
        for p in arr {
            let pname = p.get("name").and_then(|v| v.as_str())?.to_string();
            let value = p.get("value").and_then(|v| v.as_str())?.to_string();
            let signature = p
                .get("signature")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            properties.push(ProfileProperty {
                name: pname,
                value,
                signature,
            });
        }
    }
    Some(AuthProfile {
        uuid,
        name,
        properties,
    })
}

/// Percent-encode a query-string component. Names are `[A-Za-z0-9_]` and the
/// server hash is hex (optionally `-`), so only a tiny set ever needs escaping,
/// but we encode everything outside the unreserved set to be safe.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three canonical examples from wiki.vg's "Server List Ping" / auth
    /// documentation for the Minecraft signed-hex SHA-1.
    #[test]
    fn known_minecraft_hashes() {
        assert_eq!(notch_style("Notch"), "4ed1f46bbe04bc756bcb17c0c7ce3e4632f06a48");
        assert_eq!(notch_style("jeb_"), "-7c9d5b0044c130109a5d7b5fb5c317c02b4e28c1");
        assert_eq!(notch_style("simon"), "88e16a1019277b15d58faf0541e11910eb756f6");
    }

    /// Helper: the documented hashes are the SHA-1 of the literal name bytes,
    /// rendered through the same signed-hex path the auth uses.
    fn notch_style(input: &str) -> String {
        let mut hasher = Sha1::new();
        hasher.update(input.as_bytes());
        signed_hex(hasher.finalize().into())
    }

    #[test]
    fn cfb8_round_trips() {
        let key = [7u8; 16];
        let mut enc = Cfb8::new(&key);
        let mut dec = Cfb8::new(&key);
        let plain = b"hello, this is a longer message spanning many CFB8 blocks!!";
        let mut buf = plain.to_vec();
        enc.encrypt(&mut buf);
        assert_ne!(&buf[..], &plain[..]);
        dec.decrypt(&mut buf);
        assert_eq!(&buf[..], &plain[..]);
    }

    #[test]
    fn cfb8_streams_across_calls() {
        // Encrypting in two calls must match encrypting in one (continuous state).
        let key = [0x42u8; 16];
        let data = b"split me into two halves and reassemble";
        let mut whole = Cfb8::new(&key);
        let mut a = data.to_vec();
        whole.encrypt(&mut a);

        let mut split = Cfb8::new(&key);
        let mut b = data.to_vec();
        let (l, r) = b.split_at_mut(5);
        split.encrypt(l);
        split.encrypt(r);
        assert_eq!(a, b);
    }
}
