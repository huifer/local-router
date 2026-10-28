//! The COSY envelope: how every QwenWork request is signed and encoded.
//!
//! QwenWork (and the sibling Qoder sites) sign each call with a `COSY.…`
//! Authorization header and encode the request body through a custom codec:
//!
//! 1. A fresh 16-byte AES-128-CBC key is generated per request. It is its
//!    own IV, and the user blob (account identity) is encrypted under it.
//! 2. The AES key is wrapped with the vendor's RSA-1024 public key
//!    (`Cosy-Key`).
//! 3. A `COSY.${payload}.${md5}` Authorization header is built from a base64
//!    payload and an MD5 signature over `payload\nkey\nts\nbody\npath`.
//! 4. The request body is base64-through-a-shuffled-alphabet, then its
//!    first and last thirds are swapped (`Encode=1`).
//!
//! The constants (RSA key, alphabet, scene, client type, version) come from
//! the QwenWork desktop client, as recorded by the magpie community plugin.

use std::collections::BTreeMap;

use aes::cipher::block_padding::Pkcs7;
use aes::cipher::generic_array::GenericArray;
use aes::cipher::KeyIvInit;
use aes::Aes128;
use base64::Engine;
use cbc::cipher::BlockEncryptMut;
use cbc::Encryptor;
use md5::Digest;
use rand::RngCore;
use rsa::pkcs8::DecodePublicKey;
use rsa::Pkcs1v15Encrypt;
use rsa::RsaPublicKey;
use serde_json::Value;

type Aes128CbcEnc = Encryptor<Aes128>;

/// QwenWork's embedded 1024-bit RSA public key, from the client's cosy
/// source (byte-equal to Qoder's; the headers a request carries are what
/// tell the sites apart).
pub const RSA_KEY_PEM: &str = "-----BEGIN PUBLIC KEY-----\nMIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQDA8iMH5c02LilrsERw9t6Pv5Nc\n4k6Pz1EaDicBMpdpxKduSZu5OANqUq8er4GM95omAGIOPOh+Nx0spthYA2BqGz+l\n6HRkPJ7S236FZz73In/KVuLnwI8JJ2CbuJap8kvheCCZpmAWpb/cPx/3Vr/J6I17\nXcW+ML9FoCI6AOvOzwIDAQAB\n-----END PUBLIC KEY-----";

/// The build the envelope claims (`Cosy-Version`). It must be the version
/// QwenWork's desktop client talks through, because the envelope also says
/// `Cosy-ClientType: 6` and carries that client's machine identity.
pub const COSY_VERSION: &str = "1.1.59";

/// The QwenWork desktop client's device-flow client id.
pub const CLIENT_ID: &str = "e883ade2-e6e3-4d6d-adf7-f92ceff5fdcb";

/// The shuffled base64 alphabet the body codec uses (`'$'` pads).
const ALPHABET: &[u8] = b"_doRTgHZBKcGVjlvpC,@aFSx#DPuNJme&i*MzLOEn)sUrthbf%Y^w.(kIQyXqWA!";

/// The scene and product QwenWork's public cloud expects.
pub const SCENE: &str = "qwork";
pub const BUSINESS_PRODUCT: &str = "qoder_work";
pub const SESSION_TYPE: &str = "qoder_work";
pub const CLIENT_TYPE: &str = "6";

/// QwenWork's system prompt, sent as the agent system turn.
pub const SYSTEM_PROMPT: &str = "You are QwenWork (qw), a helpful assistant. Use the instructions below and the tools available to you to assist the user.";

/// The machine OS string the desktop clients send.
fn machine_os() -> String {
    format!("{}_{}", std::env::consts::ARCH, std::env::consts::OS)
}

/// The account identity the envelope carries: AES-128-CBC under a fresh
/// key (which is its own IV), the key wrapped with the RSA public key.
pub fn user_blob(u: &CosyUser, rsa: &RsaPublicKey) -> anyhow::Result<CosyBlob> {
    let raw = serde_json::to_string(&serde_json::json!({
        "uid": u.uid,
        "aid": "",
        "name": u.name,
        "email": u.email,
        "security_oauth_token": u.access,
    }))?;
    let key = {
        let mut bytes = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut bytes);
        bytes
    };
    let cipher = Aes128CbcEnc::new(
        GenericArray::from_slice(&key),
        GenericArray::from_slice(&key),
    );
    // PKCS7 pads at most one block, so the buffer holds the message plus 16.
    let msg_len = raw.len();
    let mut buf = vec![0u8; msg_len + 16];
    buf[..msg_len].copy_from_slice(raw.as_bytes());
    let encrypted = cipher
        .encrypt_padded_mut::<Pkcs7>(&mut buf, msg_len)
        .map_err(|e| anyhow::anyhow!("the COSY user blob could not be encrypted: {e}"))?
        .to_vec();
    let info = base64::engine::general_purpose::STANDARD.encode(encrypted);
    let wrapped = rsa
        .encrypt(&mut rand::thread_rng(), Pkcs1v15Encrypt, &key)
        .map_err(|e| anyhow::anyhow!("the COSY AES key could not be wrapped: {e}"))?;
    let key_wrapped = base64::engine::general_purpose::STANDARD.encode(wrapped);
    Ok(CosyBlob {
        info,
        key: key_wrapped,
    })
}

/// The output of [`user_blob`].
pub struct CosyBlob {
    /// The RSA-wrapped AES key, base64.
    pub key: String,
    /// The AES-encrypted user identity, base64.
    pub info: String,
}

/// The account identity inside the envelope.
pub struct CosyUser<'a> {
    pub uid: &'a str,
    pub name: &'a str,
    pub email: &'a str,
    /// The device token, which QwenWork uses as the chat token.
    pub access: &'a str,
}

/// Build the COSY `Authorization` header and the family of `Cosy-*` headers.
///
/// `body` is the **wire** (encoded) body — the signature covers it exactly
/// as it goes on the wire. `path` is the URL path used in the signature.
pub fn cosy_headers(
    url: &str,
    u: &CosyUser,
    body: &str,
    rsa: &RsaPublicKey,
    machine_id: &str,
) -> anyhow::Result<BTreeMap<String, String>> {
    if machine_id.trim().is_empty() {
        anyhow::bail!("the QwenWork account has no machine id; sign in again");
    }
    let blob = user_blob(u, rsa)?;
    let ts = chrono::Utc::now().timestamp();
    let payload =
        base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&serde_json::json!({
            "version": "v1",
            "requestId": hex_id(),
            "info": blob.info,
            "cosyVersion": COSY_VERSION,
            "ideVersion": "",
        }))?);
    // The path the signature covers: the URL's path only (no query), with
    // the gateway's "/algo" prefix stripped — the same path the COSY
    // gateway signs with. A query string or the prefix left in makes the
    // MD5 differ, and the vendor answers "Signature invalid".
    let path = {
        let without_query = url
            .split(['?', '#'])
            .next()
            .unwrap_or("")
            .trim_end_matches('/');
        let host_and_path = without_query
            .split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or(without_query);
        let path = host_and_path
            .split_once('/')
            .map(|(_, rest)| rest)
            .unwrap_or("");
        let path = format!("/{}", path.strip_prefix("algo/").unwrap_or(path));
        path
    };
    let mut hasher = md5::Md5::new();
    hasher.update(format!("{payload}\n{}\n{ts}\n{body}\n{path}", blob.key).as_bytes());
    let sig = hex::encode(hasher.finalize());

    let mut headers = BTreeMap::new();
    headers.insert("Accept".to_string(), "application/json".to_string());
    headers.insert("Accept-Encoding".to_string(), "identity".to_string());
    headers.insert("Content-Type".to_string(), "application/json".to_string());
    headers.insert(
        "Authorization".to_string(),
        format!("Bearer COSY.{payload}.{sig}"),
    );
    headers.insert(
        "Cosy-Business-Product".to_string(),
        BUSINESS_PRODUCT.to_string(),
    );
    headers.insert("Cosy-Business-Type".to_string(), "agent".to_string());
    headers.insert("Cosy-ClientIp".to_string(), machine_id.to_string());
    headers.insert("Cosy-ClientType".to_string(), CLIENT_TYPE.to_string());
    headers.insert("Cosy-Data-Policy".to_string(), "disagree".to_string());
    headers.insert("Cosy-Date".to_string(), ts.to_string());
    headers.insert("Cosy-Key".to_string(), blob.key);
    headers.insert("Cosy-MachineId".to_string(), machine_id.to_string());
    headers.insert("Cosy-MachineToken".to_string(), machine_id.to_string());
    headers.insert("Cosy-MachineType".to_string(), "5".to_string());
    headers.insert("Cosy-MachineOS".to_string(), machine_os());
    headers.insert("Cosy-Scene".to_string(), SCENE.to_string());
    headers.insert("Cosy-User".to_string(), u.uid.to_string());
    headers.insert("Cosy-Version".to_string(), COSY_VERSION.to_string());
    headers.insert("Login-Version".to_string(), "v2".to_string());
    Ok(headers)
}

/// A per-request hex id (no dashes).
pub fn hex_id() -> String {
    uuid::Uuid::new_v4().simple().to_string().to_uppercase()
}

/// Encode a plaintext JSON body the way QwenWork's `Encode=1` expects:
/// base64 through the shuffled alphabet, first and last thirds swapped.
pub fn encode_body(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut s = String::new();
    let mut acc: u32 = 0;
    let mut nb = 0u32;
    for &b in bytes {
        acc = ((acc << 8) | b as u32) & 0xffffff;
        nb += 8;
        while nb >= 6 {
            nb -= 6;
            s.push(ALPHABET[((acc >> nb) & 63) as usize] as char);
        }
    }
    if nb > 0 {
        s.push(ALPHABET[((acc << (6 - nb)) & 63) as usize] as char);
    }
    while s.len() % 4 != 0 {
        s.push('$');
    }
    swap_thirds(&s)
}

/// Reverse [`encode_body`].
pub fn decode_body(wire: &str) -> anyhow::Result<String> {
    let s = swap_thirds(wire);
    let mut out = Vec::new();
    let mut i = 0;
    while i + 4 <= s.len() {
        let vals: Vec<i32> = s[i..i + 4]
            .chars()
            .filter(|c| *c != '$')
            .map(|c| {
                ALPHABET
                    .iter()
                    .position(|&x| x == c as u8)
                    .map(|p| p as i32)
                    .unwrap_or(0)
            })
            .collect();
        let mut x: i64 = 0;
        for &v in &vals {
            x = x * 64 + v as i64;
        }
        let n = (6 * vals.len()) / 8;
        for k in 0..n {
            out.push(((x >> (6 * vals.len() - 8 * (k + 1))) & 255) as u8);
        }
        i += 4;
    }
    String::from_utf8(out).map_err(|e| anyhow::anyhow!("the QwenWork body was not utf-8: {e}"))
}

/// Swap the first and last thirds of a string.
fn swap_thirds(s: &str) -> String {
    let t = s.len() / 3;
    if t == 0 {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&bytes[s.len() - t..]);
    out.extend_from_slice(&bytes[t..s.len() - t]);
    out.extend_from_slice(&bytes[..t]);
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// Parse QwenWork's RSA public key once.
pub fn rsa_key() -> anyhow::Result<RsaPublicKey> {
    RsaPublicKey::from_public_key_pem(RSA_KEY_PEM)
        .map_err(|e| anyhow::anyhow!("the QwenWork RSA key is invalid: {e}"))
}

/// A number as Go's `%g` writes it, as QwenWork's account-context uses.
pub fn gfmt(n: f64) -> String {
    let s = format!("{:.6e}", n);
    let (mantissa, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let x: i32 = exp.parse().unwrap_or(0);
    if !(-4..6).contains(&x) {
        format!("{mantissa}e{x:0>3}")
    } else if n.fract() == 0.0 {
        format!("{:.0}", n)
    } else {
        format!("{n}")
    }
}

/// Unused helper kept for parity: pull a nested value out of a JSON payload.
pub fn _unused_value(_v: &Value) -> Option<&Value> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------- keys --

    #[test]
    fn the_rsa_public_key_parses_and_wraps() {
        let key = rsa_key().expect("the built-in RSA key must parse");
        // A wrapped AES-128 key is always the RSA modulus size: 128 bytes
        // for a 1024-bit key.
        let wrapped = key
            .encrypt(&mut rand::thread_rng(), Pkcs1v15Encrypt, &[7u8; 16])
            .unwrap();
        assert_eq!(wrapped.len(), 128);
    }

    // ------------------------------------------------------------ codec --

    #[test]
    fn body_codec_round_trips() {
        let samples = [
            "",
            "{}",
            "hello",
            "{\"model\":\"qw-qwen3-coder-plus\",\"stream\":true}",
            // A 4-byte body exercises the padding and the third-swap edge.
            "abcd",
            // Multi-byte utf-8 must survive.
            "你好，千问办公",
        ];
        for text in samples {
            let wire = encode_body(text);
            assert_eq!(
                decode_body(&wire).unwrap(),
                text,
                "round-trip failed for {text:?}"
            );
        }
    }

    #[test]
    fn encoded_bodies_use_the_shuffled_alphabet_and_dollar_padding() {
        let wire = encode_body("{}");
        assert!(wire.len() % 4 == 0);
        for c in wire.chars() {
            assert!(
                c == '$' || ALPHABET.contains(&(c as u8)),
                "unexpected char {c}"
            );
        }
        // Standard base64 would put a '+' or '/'; this alphabet has none.
        assert!(!wire.contains('+'));
        assert!(!wire.contains('/'));
    }

    #[test]
    fn decode_rejects_broken_bodies_gracefully() {
        // A body of pure padding carries no bytes: it decodes to empty.
        assert_eq!(decode_body("$$$$").unwrap(), "");
        // Unknown characters land at position 0 rather than panicking.
        let _ = decode_body("!!!!");
    }

    #[test]
    fn swap_thirds_is_an_involution() {
        let text = "abcdefghi"; // length divisible by three
        let swapped = swap_thirds(text);
        // last third + middle third + first third
        assert_eq!(swapped, "ghidefabc");
        assert_eq!(swap_thirds(&swapped), text);
        // Too short to split: unchanged.
        assert_eq!(swap_thirds("ab"), "ab");
    }

    // ------------------------------------------------------------ ids --

    #[test]
    fn hex_ids_are_uppercase_without_dashes() {
        for _ in 0..8 {
            let id = hex_id();
            assert_eq!(id.len(), 32);
            assert!(id
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase()));
        }
        assert_ne!(hex_id(), hex_id());
    }

    // --------------------------------------------------------- gfmt --

    #[test]
    fn gfmt_matches_go_percent_g_shapes() {
        assert_eq!(gfmt(0.0), "0");
        assert_eq!(gfmt(1000.0), "1000");
        assert_eq!(gfmt(980.0), "980");
        assert_eq!(gfmt(1.5), "1.5");
        // Large magnitudes keep the exponent form.
        assert!(gfmt(1.0e8).contains('e'));
    }

    // --------------------------------------------------------- blob --

    #[test]
    fn user_blob_wraps_an_aes_key_rsa_can_sized() {
        let rsa = rsa_key().unwrap();
        let user = CosyUser {
            uid: "u-1",
            name: "Zhang San",
            email: "z@example.com",
            access: "device-token",
        };
        let blob = user_blob(&user, &rsa).unwrap();
        // The wrapped key is 128 bytes of RSA-1024 ciphertext, base64.
        let key_raw = base64::engine::general_purpose::STANDARD
            .decode(&blob.key)
            .unwrap();
        assert_eq!(key_raw.len(), 128);
        // The identity payload is non-empty and base64.
        assert!(!blob.info.is_empty());
        base64::engine::general_purpose::STANDARD
            .decode(&blob.info)
            .expect("the info field must be valid base64");
    }

    // ------------------------------------------------------- headers --

    #[test]
    fn cosy_headers_carry_the_full_family_and_sign_the_wire_body() {
        let rsa = rsa_key().unwrap();
        let user = CosyUser {
            uid: "u-1",
            name: "Zhang San",
            email: "z@example.com",
            access: "device-token",
        };
        let headers = cosy_headers(
            "https://gateway.qwenwork.cn/algo/api/v2/model/list?Encode=1",
            &user,
            "WIRE-BODY",
            &rsa,
            "machine-1",
        )
        .unwrap();
        assert!(headers["Authorization"].starts_with("Bearer COSY."));
        assert_eq!(headers["Cosy-ClientType"], CLIENT_TYPE);
        assert_eq!(headers["Cosy-Scene"], SCENE);
        assert_eq!(headers["Cosy-Business-Product"], BUSINESS_PRODUCT);
        assert_eq!(headers["Cosy-Version"], COSY_VERSION);
        assert_eq!(headers["Cosy-MachineId"], "machine-1");
        assert_eq!(headers["Cosy-User"], "u-1");
        assert_eq!(headers["Cosy-Key"], headers["Cosy-Key"]); // present
        assert_eq!(headers["Content-Type"], "application/json");
        assert!(headers.contains_key("Cosy-Date"));
        assert!(headers.contains_key("Cosy-MachineOS"));
        assert_eq!(headers["Login-Version"], "v2");

        // The signature covers the URL's path only: the query string is
        // left out, and the gateway's "/algo" prefix is stripped — the
        // same path the COSY gateway signs with.
        let auth = &headers["Authorization"];
        let sig = auth.rsplit('.').next().unwrap();
        let payload = auth
            .strip_prefix("Bearer COSY.")
            .and_then(|rest| rest.split('.').next())
            .unwrap()
            .to_string();
        let mut hasher = md5::Md5::new();
        hasher.update(format!(
            "{payload}\n{}\n{}\nWIRE-BODY\n/api/v2/model/list",
            headers["Cosy-Key"], headers["Cosy-Date"],
        ));
        assert_eq!(sig, hex::encode(hasher.finalize()));
    }

    #[test]
    fn cosy_headers_refuse_an_empty_machine_id() {
        let rsa = rsa_key().unwrap();
        let user = CosyUser {
            uid: "u-1",
            name: "n",
            email: "e",
            access: "t",
        };
        assert!(cosy_headers("https://gateway.qwenwork.cn/a", &user, "", &rsa, "  ").is_err());
    }

    #[test]
    fn the_authorization_signature_covers_payload_key_ts_body_path() {
        // Recompute the MD5 by hand from the header parts and confirm it
        // matches: the signature must be exactly `payload\nkey\nts\nbody\npath`.
        let rsa = rsa_key().unwrap();
        let user = CosyUser {
            uid: "u-1",
            name: "n",
            email: "e",
            access: "t",
        };
        let url = "https://gateway.qwenwork.cn/algo/api/v2/model/list?Encode=1";
        let headers = cosy_headers(url, &user, "WIRE", &rsa, "machine-1").unwrap();
        let auth = headers["Authorization"]
            .strip_prefix("Bearer COSY.")
            .unwrap()
            .to_string();
        let (payload, sig) = auth.rsplit_once('.').unwrap();
        let payload_json: Value = serde_json::from_slice(
            &base64::engine::general_purpose::STANDARD
                .decode(payload)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(payload_json["version"], "v1");
        assert_eq!(payload_json["cosyVersion"], COSY_VERSION);
        // The encrypted identity rides inside the payload too.
        assert!(payload_json["info"].as_str().is_some_and(|s| !s.is_empty()));
        let key = headers["Cosy-Key"].clone();
        let ts = headers["Cosy-Date"].clone();
        // The signed path is the URL's path only: the query string is left
        // out, and the gateway's "/algo" prefix is stripped.
        let path = "/api/v2/model/list";
        let mut hasher = md5::Md5::new();
        hasher.update(format!("{payload}\n{key}\n{ts}\nWIRE\n{path}").as_bytes());
        assert_eq!(hex::encode(hasher.finalize()), sig);
    }
}
