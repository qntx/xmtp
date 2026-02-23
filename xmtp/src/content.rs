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

const XMTP_ORG: &str = "xmtp.org";

fn text_type_id() -> ContentTypeId {
    ContentTypeId {
        authority_id: XMTP_ORG.into(),
        type_id: "text".into(),
        version_major: 1,
        version_minor: 0,
    }
}

fn markdown_type_id() -> ContentTypeId {
    ContentTypeId {
        authority_id: XMTP_ORG.into(),
        type_id: "markdown".into(),
        version_major: 1,
        version_minor: 0,
    }
}

fn reaction_type_id() -> ContentTypeId {
    ContentTypeId {
        authority_id: XMTP_ORG.into(),
        type_id: "reaction".into(),
        version_major: 2,
        version_minor: 0,
    }
}

fn read_receipt_type_id() -> ContentTypeId {
    ContentTypeId {
        authority_id: XMTP_ORG.into(),
        type_id: "readReceipt".into(),
        version_major: 1,
        version_minor: 0,
    }
}

fn reply_type_id() -> ContentTypeId {
    ContentTypeId {
        authority_id: XMTP_ORG.into(),
        type_id: "reply".into(),
        version_major: 1,
        version_minor: 0,
    }
}

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
    /// Unknown or unsupported content type.
    Unknown {
        /// The content type string (e.g. `"xmtp.org/text:1.0"`).
        content_type: String,
        /// Raw protobuf-encoded [`EncodedContent`] bytes.
        raw: Vec<u8>,
    },
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

/// Encode a text string into protobuf bytes ready for [`Conversation::send`].
#[must_use]
pub fn encode_text(text: &str) -> Vec<u8> {
    let ec = EncodedContent {
        r#type: Some(text_type_id()),
        parameters: HashMap::from([("encoding".into(), "UTF-8".into())]),
        fallback: None,
        content: text.as_bytes().to_vec(),
        compression: None,
    };
    ec.encode_to_vec()
}

/// Encode a markdown string into protobuf bytes.
#[must_use]
pub fn encode_markdown(markdown: &str) -> Vec<u8> {
    let ec = EncodedContent {
        r#type: Some(markdown_type_id()),
        parameters: HashMap::from([("encoding".into(), "UTF-8".into())]),
        fallback: None,
        content: markdown.as_bytes().to_vec(),
        compression: None,
    };
    ec.encode_to_vec()
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
    let ec = EncodedContent {
        r#type: Some(reaction_type_id()),
        parameters: HashMap::new(),
        fallback: Some(format!("Reacted with \"{emoji}\" to an earlier message")),
        content: rv2.encode_to_vec(),
        compression: None,
    };
    ec.encode_to_vec()
}

/// Encode a read receipt into protobuf bytes.
#[must_use]
pub fn encode_read_receipt() -> Vec<u8> {
    let ec = EncodedContent {
        r#type: Some(read_receipt_type_id()),
        parameters: HashMap::new(),
        fallback: None,
        content: Vec::new(),
        compression: None,
    };
    ec.encode_to_vec()
}

/// Encode a reply into protobuf bytes.
///
/// `reference` is the hex-encoded message ID being replied to.
/// `inner_content` is the protobuf-encoded [`EncodedContent`] of the reply body.
#[must_use]
pub fn encode_reply(reference: &str, inner_content: &[u8]) -> Vec<u8> {
    let ec = EncodedContent {
        r#type: Some(reply_type_id()),
        parameters: HashMap::from([("reference".into(), reference.into())]),
        fallback: Some("Replied to an earlier message".into()),
        content: inner_content.to_vec(),
        compression: None,
    };
    ec.encode_to_vec()
}

/// Encode a text reply into protobuf bytes (convenience).
#[must_use]
pub fn encode_text_reply(reference: &str, text: &str) -> Vec<u8> {
    encode_reply(reference, &encode_text(text))
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
}
