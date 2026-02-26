//! Content type encoding, decoding, and typed send helpers.
//!
//! This module is available when the `content` feature is enabled (default).
//! It provides type-safe wrappers around the raw protobuf `EncodedContent`
//! wire format so callers never need to construct protobuf bytes manually.

use std::collections::HashMap;

use prost::Message as ProstMessage;

use crate::conversation::{Conversation, Message};
use crate::error::Result;

/// Content type identifier on the XMTP network.
#[derive(Clone, PartialEq, Eq, Hash, ProstMessage)]
pub struct ContentTypeId {
    /// Authority (e.g. `"xmtp.org"`).
    #[prost(string, tag = "1")]
    pub authority_id: String,
    /// Type name (e.g. `"text"`).
    #[prost(string, tag = "2")]
    pub type_id: String,
    /// Major version.
    #[prost(uint32, tag = "3")]
    pub version_major: u32,
    /// Minor version.
    #[prost(uint32, tag = "4")]
    pub version_minor: u32,
}

/// Encoded content envelope — the XMTP v3 wire format.
#[derive(Clone, PartialEq, Eq, ProstMessage)]
pub struct EncodedContent {
    /// Content type identifier.
    #[prost(message, optional, tag = "1")]
    pub r#type: Option<ContentTypeId>,
    /// Encoding parameters (e.g. `encoding=UTF-8`).
    #[prost(map = "string, string", tag = "2")]
    pub parameters: HashMap<String, String>,
    /// Fallback text for clients that cannot decode this content type.
    #[prost(string, optional, tag = "3")]
    pub fallback: Option<String>,
    /// Raw content bytes.
    #[prost(bytes = "vec", tag = "4")]
    pub content: Vec<u8>,
    /// Optional compression algorithm.
    #[prost(enumeration = "Compression", optional, tag = "5")]
    pub compression: Option<i32>,
}

/// Compression algorithm for encoded content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, prost::Enumeration)]
#[repr(i32)]
pub enum Compression {
    /// Deflate (zlib).
    Deflate = 1,
    /// Gzip.
    Gzip = 2,
}

/// A reaction to a message.
#[derive(Clone, PartialEq, Eq, Hash, ProstMessage)]
pub struct ReactionV2 {
    /// Hex-encoded message ID being reacted to.
    #[prost(string, tag = "1")]
    pub reference: String,
    /// Inbox ID of the sender of the referenced message.
    #[prost(string, tag = "2")]
    pub reference_inbox_id: String,
    /// Reaction action.
    #[prost(enumeration = "ReactionAction", tag = "3")]
    pub action: i32,
    /// The emoji / shortcode / custom string.
    #[prost(string, tag = "4")]
    pub content: String,
    /// Content schema.
    #[prost(enumeration = "ReactionSchema", tag = "5")]
    pub schema: i32,
}

/// Reaction action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, prost::Enumeration)]
#[repr(i32)]
pub enum ReactionAction {
    /// Unspecified.
    Unspecified = 0,
    /// Reaction added.
    Added = 1,
    /// Reaction removed.
    Removed = 2,
}

/// Reaction content schema.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, prost::Enumeration)]
#[repr(i32)]
pub enum ReactionSchema {
    /// Unspecified.
    Unspecified = 0,
    /// Unicode emoji (e.g. "👍").
    Unicode = 1,
    /// Shortcode (e.g. ":thumbsup:").
    Shortcode = 2,
    /// Custom string.
    Custom = 3,
}

/// Metadata for a remotely hosted encrypted attachment.
#[derive(Clone, PartialEq, Eq, Hash, ProstMessage)]
pub struct RemoteAttachmentInfo {
    /// SHA-256 digest of the encrypted payload (hex string).
    #[prost(string, tag = "1")]
    pub content_digest: String,
    /// 32-byte secret key for decryption.
    #[prost(bytes = "vec", tag = "2")]
    pub secret: Vec<u8>,
    /// Nonce used for encryption.
    #[prost(bytes = "vec", tag = "3")]
    pub nonce: Vec<u8>,
    /// Salt used for key derivation.
    #[prost(bytes = "vec", tag = "4")]
    pub salt: Vec<u8>,
    /// URL scheme (e.g. `"https"`).
    #[prost(string, tag = "5")]
    pub scheme: String,
    /// URL of the encrypted payload.
    #[prost(string, tag = "6")]
    pub url: String,
    /// Size of the encrypted content in bytes.
    #[prost(uint32, optional, tag = "7")]
    pub content_length: Option<u32>,
    /// Original filename.
    #[prost(string, optional, tag = "8")]
    pub filename: Option<String>,
}

const XMTP_ORG: &str = "xmtp.org";

/// Create a [`ContentTypeId`] for a well-known XMTP content type.
const fn xmtp_type(type_id: &'static str, major: u32) -> (&'static str, &'static str, u32, u32) {
    (XMTP_ORG, type_id, major, 0)
}

fn make_type_id(t: (&str, &str, u32, u32)) -> ContentTypeId {
    ContentTypeId {
        authority_id: t.0.into(),
        type_id: t.1.into(),
        version_major: t.2,
        version_minor: t.3,
    }
}

const TEXT: (&str, &str, u32, u32) = xmtp_type("text", 1);
const MARKDOWN: (&str, &str, u32, u32) = xmtp_type("markdown", 1);
const REACTION: (&str, &str, u32, u32) = xmtp_type("reaction", 2);
const READ_RECEIPT: (&str, &str, u32, u32) = xmtp_type("readReceipt", 1);
const REPLY: (&str, &str, u32, u32) = xmtp_type("reply", 1);
const ATTACHMENT: (&str, &str, u32, u32) = xmtp_type("attachment", 1);
const REMOTE_ATTACHMENT: (&str, &str, u32, u32) = xmtp_type("remoteStaticAttachment", 1);

/// Decoded message content.
#[derive(Debug, Clone)]
pub enum Content {
    /// Plain text message.
    Text(String),
    /// Markdown message.
    Markdown(String),
    /// Reaction to another message.
    Reaction(Reaction),
    /// Reply to another message.
    Reply(Reply),
    /// Read receipt (no payload).
    ReadReceipt,
    /// Inline file attachment.
    Attachment(Attachment),
    /// Remote (URL-hosted) encrypted attachment.
    RemoteAttachment(RemoteAttachment),
    /// Unknown or unsupported content type.
    Unknown {
        /// The content type string (e.g. `"xmtp.org/text:1.0"`).
        content_type: String,
        /// Raw protobuf-encoded [`EncodedContent`] bytes.
        raw: Vec<u8>,
    },
}

impl Content {
    /// Returns `true` if this is a [`Content::Text`].
    #[must_use]
    pub const fn is_text(&self) -> bool {
        matches!(self, Self::Text(_))
    }

    /// Returns `true` if this is a [`Content::Markdown`].
    #[must_use]
    pub const fn is_markdown(&self) -> bool {
        matches!(self, Self::Markdown(_))
    }

    /// Returns `true` if this is a [`Content::Reaction`].
    #[must_use]
    pub const fn is_reaction(&self) -> bool {
        matches!(self, Self::Reaction(_))
    }

    /// Returns `true` if this is a [`Content::Reply`].
    #[must_use]
    pub const fn is_reply(&self) -> bool {
        matches!(self, Self::Reply(_))
    }

    /// Returns `true` if this is a [`Content::ReadReceipt`].
    #[must_use]
    pub const fn is_read_receipt(&self) -> bool {
        matches!(self, Self::ReadReceipt)
    }

    /// Returns `true` if this is a [`Content::Attachment`].
    #[must_use]
    pub const fn is_attachment(&self) -> bool {
        matches!(self, Self::Attachment(_))
    }

    /// Returns `true` if this is a [`Content::RemoteAttachment`].
    #[must_use]
    pub const fn is_remote_attachment(&self) -> bool {
        matches!(self, Self::RemoteAttachment(_))
    }

    /// Returns `true` if this is a [`Content::Unknown`].
    #[must_use]
    pub const fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown { .. })
    }

    /// Returns the text if this is a [`Content::Text`], or `None`.
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        if let Self::Text(s) = self {
            Some(s)
        } else {
            None
        }
    }

    /// Returns the reaction if this is a [`Content::Reaction`], or `None`.
    #[must_use]
    pub const fn as_reaction(&self) -> Option<&Reaction> {
        if let Self::Reaction(r) = self {
            Some(r)
        } else {
            None
        }
    }

    /// Returns the reply if this is a [`Content::Reply`], or `None`.
    #[must_use]
    pub const fn as_reply(&self) -> Option<&Reply> {
        if let Self::Reply(r) = self {
            Some(r)
        } else {
            None
        }
    }

    /// Returns the attachment if this is a [`Content::Attachment`], or `None`.
    #[must_use]
    pub const fn as_attachment(&self) -> Option<&Attachment> {
        if let Self::Attachment(a) = self {
            Some(a)
        } else {
            None
        }
    }

    /// Returns the remote attachment if this is a
    /// [`Content::RemoteAttachment`], or `None`.
    #[must_use]
    pub const fn as_remote_attachment(&self) -> Option<&RemoteAttachment> {
        if let Self::RemoteAttachment(r) = self {
            Some(r)
        } else {
            None
        }
    }
}

/// A decoded reaction.
#[derive(Debug, Clone)]
pub struct Reaction {
    /// Hex-encoded message ID being reacted to.
    pub reference: String,
    /// Inbox ID of the referenced message's sender.
    pub reference_inbox_id: String,
    /// Reaction action.
    pub action: ReactionAction,
    /// The emoji / shortcode / custom content.
    pub content: String,
    /// Content schema.
    pub schema: ReactionSchema,
}

/// A decoded reply.
#[derive(Debug, Clone)]
pub struct Reply {
    /// Hex-encoded message ID being replied to.
    pub reference: String,
    /// Inbox ID of the referenced message's sender.
    pub reference_inbox_id: Option<String>,
    /// The reply content (protobuf-encoded inner `EncodedContent`).
    pub content: EncodedContent,
}

/// An inline file attachment.
#[derive(Debug, Clone)]
pub struct Attachment {
    /// Optional filename.
    pub filename: Option<String>,
    /// MIME type (e.g. `"image/png"`).
    pub mime_type: String,
    /// Raw file content.
    pub data: Vec<u8>,
}

/// A remote (URL-hosted) encrypted attachment.
#[derive(Debug, Clone)]
pub struct RemoteAttachment {
    /// URL of the encrypted payload.
    pub url: String,
    /// SHA-256 digest of the encrypted content (hex string).
    pub content_digest: String,
    /// 32-byte secret key for decryption.
    pub secret: Vec<u8>,
    /// Nonce used for encryption.
    pub nonce: Vec<u8>,
    /// Salt used for key derivation.
    pub salt: Vec<u8>,
    /// URL scheme (e.g. `"https"`).
    pub scheme: String,
    /// Size of the encrypted content in bytes.
    pub content_length: Option<u32>,
    /// Original filename.
    pub filename: Option<String>,
}

/// Encode a text string into protobuf bytes ready for [`Conversation::send`].
#[must_use]
pub fn encode_text(text: &str) -> Vec<u8> {
    EncodedContent {
        r#type: Some(make_type_id(TEXT)),
        parameters: HashMap::from([("encoding".into(), "UTF-8".into())]),
        fallback: None,
        content: text.as_bytes().to_vec(),
        compression: None,
    }
    .encode_to_vec()
}

/// Encode a markdown string into protobuf bytes.
#[must_use]
pub fn encode_markdown(markdown: &str) -> Vec<u8> {
    EncodedContent {
        r#type: Some(make_type_id(MARKDOWN)),
        parameters: HashMap::from([("encoding".into(), "UTF-8".into())]),
        fallback: None,
        content: markdown.as_bytes().to_vec(),
        compression: None,
    }
    .encode_to_vec()
}

/// Encode a reaction into protobuf bytes.
#[must_use]
pub fn encode_reaction(reference: &str, emoji: &str, action: ReactionAction) -> Vec<u8> {
    let rv2 = ReactionV2 {
        reference: reference.into(),
        reference_inbox_id: String::new(),
        action: action as i32,
        content: emoji.into(),
        schema: ReactionSchema::Unicode as i32,
    };
    EncodedContent {
        r#type: Some(make_type_id(REACTION)),
        parameters: HashMap::new(),
        fallback: Some(format!("Reacted with \"{emoji}\" to an earlier message")),
        content: rv2.encode_to_vec(),
        compression: None,
    }
    .encode_to_vec()
}

/// Encode a read receipt into protobuf bytes.
#[must_use]
pub fn encode_read_receipt() -> Vec<u8> {
    EncodedContent {
        r#type: Some(make_type_id(READ_RECEIPT)),
        parameters: HashMap::new(),
        fallback: None,
        content: Vec::new(),
        compression: None,
    }
    .encode_to_vec()
}

/// Encode a reply into protobuf bytes.
///
/// `reference` is the hex-encoded message ID being replied to.
/// `inner_content` is the protobuf-encoded [`EncodedContent`] of the reply body.
#[must_use]
pub fn encode_reply(reference: &str, inner_content: &[u8]) -> Vec<u8> {
    EncodedContent {
        r#type: Some(make_type_id(REPLY)),
        parameters: HashMap::from([("reference".into(), reference.into())]),
        fallback: Some("Replied to an earlier message".into()),
        content: inner_content.to_vec(),
        compression: None,
    }
    .encode_to_vec()
}

/// Encode a text reply into protobuf bytes (convenience).
#[must_use]
pub fn encode_text_reply(reference: &str, text: &str) -> Vec<u8> {
    encode_reply(reference, &encode_text(text))
}

/// Encode an inline file attachment into protobuf bytes.
#[must_use]
pub fn encode_attachment(attachment: &Attachment) -> Vec<u8> {
    let mut params = HashMap::from([("mimeType".into(), attachment.mime_type.clone())]);
    if let Some(f) = &attachment.filename {
        params.insert("filename".into(), f.clone());
    }
    let fallback = Some(format!(
        "Can't display {}. This app doesn't support attachments.",
        attachment.filename.as_deref().unwrap_or("this content")
    ));
    EncodedContent {
        r#type: Some(make_type_id(ATTACHMENT)),
        parameters: params,
        fallback,
        content: attachment.data.clone(),
        compression: None,
    }
    .encode_to_vec()
}

/// Encode a remote attachment into protobuf bytes.
///
/// Crypto fields (secret, nonce, salt) are hex-encoded in the parameters,
/// matching the official `xmtp.org/remoteStaticAttachment` wire format.
#[must_use]
pub fn encode_remote_attachment(ra: &RemoteAttachment) -> Vec<u8> {
    let mut params = HashMap::from([
        ("contentDigest".into(), ra.content_digest.clone()),
        ("salt".into(), hex::encode(&ra.salt)),
        ("nonce".into(), hex::encode(&ra.nonce)),
        ("secret".into(), hex::encode(&ra.secret)),
        ("scheme".into(), ra.scheme.clone()),
    ]);
    if let Some(len) = ra.content_length {
        params.insert("contentLength".into(), len.to_string());
    }
    if let Some(f) = &ra.filename {
        params.insert("filename".into(), f.clone());
    }
    let fallback = Some(format!(
        "Can't display {}. This app doesn't support remote attachments.",
        ra.filename.as_deref().unwrap_or("this content")
    ));
    EncodedContent {
        r#type: Some(make_type_id(REMOTE_ATTACHMENT)),
        parameters: params,
        fallback,
        content: ra.url.as_bytes().to_vec(),
        compression: None,
    }
    .encode_to_vec()
}

/// Decode raw `Message::content` bytes into a [`Content`] variant.
///
/// # Errors
///
/// Returns an error if the bytes cannot be parsed as protobuf `EncodedContent`.
pub fn decode(raw: &[u8]) -> Result<Content> {
    let ec = EncodedContent::decode(raw)
        .map_err(|e| crate::Error::Ffi(format!("protobuf decode: {e}")))?;

    let type_id = ec.r#type.as_ref().map(|t| t.type_id.as_str());

    match type_id {
        Some("text") => {
            let s = String::from_utf8(ec.content)
                .map_err(|e| crate::Error::Ffi(format!("invalid UTF-8 text: {e}")))?;
            Ok(Content::Text(s))
        }
        Some("markdown") => {
            let s = String::from_utf8(ec.content)
                .map_err(|e| crate::Error::Ffi(format!("invalid UTF-8 markdown: {e}")))?;
            Ok(Content::Markdown(s))
        }
        Some("reaction") => {
            let rv2 = ReactionV2::decode(ec.content.as_slice())
                .map_err(|e| crate::Error::Ffi(format!("reaction decode: {e}")))?;
            Ok(Content::Reaction(Reaction {
                reference: rv2.reference,
                reference_inbox_id: rv2.reference_inbox_id,
                action: ReactionAction::try_from(rv2.action).unwrap_or(ReactionAction::Unspecified),
                content: rv2.content,
                schema: ReactionSchema::try_from(rv2.schema).unwrap_or(ReactionSchema::Unspecified),
            }))
        }
        Some("readReceipt") => Ok(Content::ReadReceipt),
        Some("attachment") => {
            let mime_type = ec.parameters.get("mimeType").cloned().unwrap_or_default();
            let filename = ec.parameters.get("filename").cloned();
            Ok(Content::Attachment(Attachment {
                filename,
                mime_type,
                data: ec.content,
            }))
        }
        Some("remoteStaticAttachment") => {
            let content_digest = ec
                .parameters
                .get("contentDigest")
                .cloned()
                .unwrap_or_default();
            let salt = ec
                .parameters
                .get("salt")
                .and_then(|s| hex::decode(s).ok())
                .unwrap_or_default();
            let nonce = ec
                .parameters
                .get("nonce")
                .and_then(|s| hex::decode(s).ok())
                .unwrap_or_default();
            let secret = ec
                .parameters
                .get("secret")
                .and_then(|s| hex::decode(s).ok())
                .unwrap_or_default();
            let scheme = ec.parameters.get("scheme").cloned().unwrap_or_default();
            let content_length = ec
                .parameters
                .get("contentLength")
                .and_then(|s| s.parse().ok());
            let filename = ec.parameters.get("filename").cloned();
            let url = String::from_utf8(ec.content)
                .map_err(|e| crate::Error::Ffi(format!("invalid URL: {e}")))?;
            Ok(Content::RemoteAttachment(RemoteAttachment {
                url,
                content_digest,
                secret,
                nonce,
                salt,
                scheme,
                content_length,
                filename,
            }))
        }
        Some("reply") => {
            let inner = EncodedContent::decode(ec.content.as_slice()).unwrap_or_default();
            let reference = ec.parameters.get("reference").cloned().unwrap_or_default();
            let reference_inbox_id = ec.parameters.get("referenceInboxId").cloned();
            Ok(Content::Reply(Reply {
                reference,
                reference_inbox_id,
                content: inner,
            }))
        }
        _ => {
            let ct = ec.r#type.as_ref().map_or_else(String::new, |t| {
                format!(
                    "{}/{}:{}.{}",
                    t.authority_id, t.type_id, t.version_major, t.version_minor
                )
            });
            Ok(Content::Unknown {
                content_type: ct,
                raw: raw.to_vec(),
            })
        }
    }
}

impl Message {
    /// Decode the raw content bytes into a typed [`Content`] variant.
    ///
    /// # Errors
    ///
    /// Returns an error if the protobuf bytes are malformed.
    pub fn decode(&self) -> Result<Content> {
        decode(&self.content)
    }
}

impl Conversation {
    /// Send a plain text message.
    pub fn send_text(&self, text: &str) -> Result<String> {
        self.send(&encode_text(text))
    }

    /// Send a markdown message.
    pub fn send_markdown(&self, markdown: &str) -> Result<String> {
        self.send(&encode_markdown(markdown))
    }

    /// Send an emoji reaction to a message.
    pub fn send_reaction(
        &self,
        message_id: &str,
        emoji: &str,
        action: ReactionAction,
    ) -> Result<String> {
        self.send(&encode_reaction(message_id, emoji, action))
    }

    /// Send a read receipt.
    pub fn send_read_receipt(&self) -> Result<String> {
        self.send(&encode_read_receipt())
    }

    /// Send a text reply to a message.
    pub fn send_text_reply(&self, reference_id: &str, text: &str) -> Result<String> {
        self.send(&encode_text_reply(reference_id, text))
    }

    /// Send a reply with arbitrary encoded content.
    pub fn send_reply(&self, reference_id: &str, inner_content: &[u8]) -> Result<String> {
        self.send(&encode_reply(reference_id, inner_content))
    }

    /// Send an inline file attachment.
    pub fn send_attachment(&self, attachment: &Attachment) -> Result<String> {
        self.send(&encode_attachment(attachment))
    }

    /// Send a remote (URL-hosted) encrypted attachment.
    pub fn send_remote_attachment(&self, ra: &RemoteAttachment) -> Result<String> {
        self.send(&encode_remote_attachment(ra))
    }

    /// Optimistically send a plain text message (returns immediately).
    pub fn send_text_optimistic(&self, text: &str) -> Result<String> {
        self.send_optimistic(&encode_text(text))
    }

    /// Optimistically send a markdown message.
    pub fn send_markdown_optimistic(&self, markdown: &str) -> Result<String> {
        self.send_optimistic(&encode_markdown(markdown))
    }

    /// Optimistically send an emoji reaction.
    pub fn send_reaction_optimistic(
        &self,
        message_id: &str,
        emoji: &str,
        action: ReactionAction,
    ) -> Result<String> {
        self.send_optimistic(&encode_reaction(message_id, emoji, action))
    }

    /// Optimistically send a text reply.
    pub fn send_text_reply_optimistic(&self, reference_id: &str, text: &str) -> Result<String> {
        self.send_optimistic(&encode_text_reply(reference_id, text))
    }
}
