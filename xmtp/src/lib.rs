#![doc = include_str!("../README.md")]
#![allow(unsafe_code)]

pub mod client;
pub mod conversation;
pub mod error;
pub mod stream;
pub mod types;

mod ffi;

// Re-export core public API at crate root.
pub use client::{Client, ClientBuilder};
pub use conversation::{Conversation, GroupMember, Message};
pub use error::{Error, Result};
pub use stream::{ConsentUpdate, PreferenceUpdate, StreamHandle};
pub use types::{
    AccountIdentifier, ApiStats, ConsentEntityType, ConsentState, ConversationDebugInfo,
    ConversationMetadata, ConversationType, CreateDmOptions, CreateGroupOptions, Cursor,
    DeliveryStatus, DisappearingSettings, Env, GroupPermissionsPreset, HmacKey, HmacKeyEntry,
    IdentifierKind, IdentityStats, InboxState, KeyPackageStatus, LastReadTime,
    ListConversationsOptions, ListMessagesOptions, MembershipState, MessageKind, PermissionLevel,
    PermissionPolicy, PermissionPolicySet, PermissionUpdateType, Permissions, SendOptions, Signer,
    SortDirection, SyncResult,
};

// Re-export standalone functions.
pub use client::{
    generate_inbox_id, get_inbox_id_for_identifier, init_logger, is_address_authorized,
    is_installation_authorized, libxmtp_version, verify_signed_with_public_key,
};
