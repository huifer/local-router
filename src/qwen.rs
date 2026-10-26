//! The QwenWork (千问办公) personal subscription — a self-contained client.
//!
//! QwenWork is Alibaba's own coding subscription. It speaks the same
//! protocol as Qoder, but its chat, models and usage calls all travel
//! inside a **COSY envelope**: a per-request AES-128 key wrapped with the
//! vendor's RSA public key, an MD5 signature over the payload, and a
//! shuffled-base64 body codec. The login is a PKCE device flow that yields
//! a device token, which doubles as the chat token.
//!
//! This module is intentionally self-contained: the generic gateway cannot
//! sign or encode a COSY request, so the QwenWork chat path lives here and
//! the gateway routes the `qwenwork` provider straight to it.
//!
//! The protocol is reverse-engineered from the desktop client and the
//! magpie community plugin (`magpie-community/plugins`, `qoder` package,
//! `qwenwork` site). Only the personal public cloud is supported
//! (`gateway.qwenwork.cn`); the enterprise flagship (its own VPC domain)
//! is out of scope by design.

pub mod cosy;

pub mod account;
pub mod client;
pub mod login;

pub use account::*;
pub use client::*;
pub use login::*;
