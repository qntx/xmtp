#![doc = include_str!("../README.md")]
#![allow(unsafe_code)]
// FFI wrapper crate: i32 ↔ usize casts at the C boundary are systematic and
// bounds-checked. Every public function follows the same error pattern (FFI
// call failure), so per-function `# Errors` docs would be pure boilerplate.
#![allow(
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::missing_errors_doc
)]

pub mod client;
pub mod conversation;
pub mod error;
pub mod stream;
pub mod types;

mod ffi;

#[cfg(feature = "content")]
pub mod content;

pub mod resolve;

#[cfg(feature = "alloy")]
mod signer;

#[cfg(feature = "ledger")]
mod ledger;

#[cfg(feature = "ens")]
mod ens;

// Re-export core public API at crate root.
pub use client::{Client, ClientBuilder};
pub use conversation::{Conversation, GroupMember, Message};
#[cfg(feature = "ens")]
pub use ens::{DEFAULT_RPC, EnsResolver};
pub use error::{Error, Result};
#[cfg(feature = "ledger")]
pub use ledger::LedgerSigner;
pub use resolve::{Recipient, Resolver};
#[cfg(feature = "alloy")]
pub use signer::AlloySigner;
pub use stream::{ConsentUpdate, MessageEvent, PreferenceUpdate, Subscription};
pub use types::{
    AccountIdentifier, ApiStats, ConsentEntityType, ConsentState, ConversationDebugInfo,
    ConversationMetadata, ConversationOrderBy, ConversationType, CreateDmOptions,
    CreateGroupOptions, Cursor, DeliveryStatus, DisappearingSettings, Env, GroupPermissionsPreset,
    HmacKey, HmacKeyEntry, IdentifierKind, IdentityStats, InboxState, KeyPackageStatus,
    LastReadTime, ListConversationsOptions, ListMessagesOptions, MembershipState, MessageKind,
    MetadataField, PermissionLevel, PermissionPolicy, PermissionPolicySet, PermissionUpdateType,
    Permissions, SendOptions, Signer, SortDirection, SyncResult,
};

// Re-export standalone functions.
pub use client::{
    generate_inbox_id, get_inbox_id_for_identifier, init_logger, is_address_authorized,
    is_installation_authorized, libxmtp_version, verify_signed_with_public_key,
};
