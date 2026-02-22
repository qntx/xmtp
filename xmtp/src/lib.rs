#![doc = include_str!("../README.md")]
#![allow(unsafe_code)]

pub mod client;
pub mod conversation;
pub mod error;
pub mod stream;
pub mod types;

mod ffi;

// Re-export core public API at crate root.
pub use client::{Client, ClientBuilder, InboxState};
pub use conversation::{Conversation, GroupMember, Message};
pub use error::{Error, Result};
pub use stream::{ConsentUpdate, PreferenceUpdate, StreamHandle};
pub use types::{
    AccountIdentifier, ConsentEntityType, ConsentState, ConversationType, CreateGroupOptions,
    DeliveryStatus, DisappearingSettings, Env, GroupPermissionsPreset, IdentifierKind,
    ListConversationsOptions, ListMessagesOptions, MembershipState, MessageKind, PermissionLevel,
    PermissionPolicy, PermissionUpdateType, Signer, SortDirection,
};

// Re-export standalone functions.
pub use client::{generate_inbox_id, get_inbox_id_for_identifier, init_logger, libxmtp_version};
