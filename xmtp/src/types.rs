//! SDK types: enumerations, option structs, data structs, and signer trait.

/// XMTP network environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Env {
    /// Local development node.
    Local,
    /// Shared development environment.
    #[default]
    Dev,
    /// Production environment.
    Production,
}

impl Env {
    /// gRPC API URL for this environment.
    #[must_use]
    pub fn url(self) -> &'static str {
        match self {
            Self::Local => "http://localhost:5556",
            Self::Dev => "https://grpc.dev.xmtp.network:443",
            Self::Production => "https://grpc.production.xmtp.network:443",
        }
    }

    /// Whether this environment uses TLS.
    #[must_use]
    pub fn is_secure(self) -> bool {
        !matches!(self, Self::Local)
    }
}

macro_rules! ffi_enum {
    ($(#[$meta:meta])* $vis:vis enum $name:ident {
        $($(#[$vm:meta])* $variant:ident = $val:expr),* $(,)?
    }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[repr(i32)]
        $vis enum $name { $($(#[$vm])* $variant = $val),* }

        impl $name {
            /// Convert from FFI `i32`. Returns `None` for unknown values.
            #[must_use]
            pub fn from_ffi(v: i32) -> Option<Self> {
                match v { $($val => Some(Self::$variant),)* _ => None }
            }
        }
    };
}

ffi_enum! {
    /// Identifier kind.
    pub enum IdentifierKind {
        /// Externally-owned Ethereum account.
        Ethereum = 0,
        /// Passkey.
        Passkey = 1,
    }
}

ffi_enum! {
    /// Conversation type.
    pub enum ConversationType {
        /// Direct message.
        Dm = 0,
        /// Group conversation.
        Group = 1,
        /// Internal sync group.
        Sync = 2,
        /// One-shot conversation.
        Oneshot = 3,
    }
}

ffi_enum! {
    /// Consent state.
    pub enum ConsentState {
        /// Not yet determined.
        Unknown = 0,
        /// Explicitly allowed.
        Allowed = 1,
        /// Explicitly denied.
        Denied = 2,
    }
}

ffi_enum! {
    /// Consent entity type.
    pub enum ConsentEntityType {
        /// Group ID.
        GroupId = 0,
        /// Inbox ID.
        InboxId = 1,
    }
}

ffi_enum! {
    /// Message kind.
    pub enum MessageKind {
        /// Application-level content.
        Application = 0,
        /// MLS membership change.
        MembershipChange = 1,
    }
}

ffi_enum! {
    /// Message delivery status.
    pub enum DeliveryStatus {
        /// Not yet published.
        Unpublished = 0,
        /// Published to the network.
        Published = 1,
        /// Failed to publish.
        Failed = 2,
    }
}

ffi_enum! {
    /// Group member permission level.
    pub enum PermissionLevel {
        /// Regular member.
        Member = 0,
        /// Administrator.
        Admin = 1,
        /// Super administrator.
        SuperAdmin = 2,
    }
}

ffi_enum! {
    /// Group permissions preset.
    pub enum GroupPermissionsPreset {
        /// All members have equal permissions.
        AllMembers = 0,
        /// Only admins can modify the group.
        AdminOnly = 1,
        /// Custom permission set.
        Custom = 2,
    }
}

ffi_enum! {
    /// Group membership state.
    pub enum MembershipState {
        /// Allowed (active member).
        Allowed = 0,
        /// Rejected.
        Rejected = 1,
        /// Pending approval.
        Pending = 2,
        /// Restored after removal.
        Restored = 3,
        /// Pending removal.
        PendingRemove = 4,
    }
}

ffi_enum! {
    /// Sort direction for message listing.
    pub enum SortDirection {
        /// Ascending (oldest first).
        Ascending = 0,
        /// Descending (newest first).
        Descending = 1,
    }
}

ffi_enum! {
    /// Permission policy values.
    pub enum PermissionPolicy {
        /// Allow all.
        Allow = 1,
        /// Deny all.
        Deny = 2,
        /// Admin only.
        AdminOnly = 3,
        /// Super admin only.
        SuperAdminOnly = 4,
    }
}

ffi_enum! {
    /// Permission update type.
    pub enum PermissionUpdateType {
        /// Add member.
        AddMember = 1,
        /// Remove member.
        RemoveMember = 2,
        /// Add admin.
        AddAdmin = 3,
        /// Remove admin.
        RemoveAdmin = 4,
        /// Update metadata.
        UpdateMetadata = 5,
    }
}

/// An account identifier (address + kind).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AccountIdentifier {
    /// The account address or public key.
    pub address: String,
    /// The kind of identifier.
    pub kind: IdentifierKind,
}

/// Options for creating a group conversation.
#[derive(Debug, Clone, Default)]
pub struct CreateGroupOptions {
    /// Permission preset.
    pub permissions: Option<GroupPermissionsPreset>,
    /// Group name.
    pub name: Option<String>,
    /// Group description.
    pub description: Option<String>,
    /// Group image URL.
    pub image_url: Option<String>,
    /// Custom app data (max 8192 bytes).
    pub app_data: Option<String>,
    /// Disappearing message settings. `None` = disabled.
    pub disappearing: Option<DisappearingSettings>,
}

/// Options for creating a DM conversation.
#[derive(Debug, Clone, Copy, Default)]
pub struct CreateDmOptions {
    /// Disappearing message settings. `None` = disabled.
    pub disappearing: Option<DisappearingSettings>,
}

/// Options for listing messages.
#[derive(Debug, Clone, Copy, Default)]
pub struct ListMessagesOptions {
    /// Only messages sent after this timestamp (ns).
    pub sent_after_ns: i64,
    /// Only messages sent before this timestamp (ns).
    pub sent_before_ns: i64,
    /// Maximum number of messages to return.
    pub limit: i64,
    /// Sort direction.
    pub direction: Option<SortDirection>,
    /// Filter by delivery status. `None` = all.
    pub delivery_status: Option<DeliveryStatus>,
    /// Filter by message kind. `None` = all.
    pub kind: Option<MessageKind>,
}

/// Options for listing conversations.
#[derive(Debug, Clone, Default)]
pub struct ListConversationsOptions {
    /// Filter by conversation type. `None` = all.
    pub conversation_type: Option<ConversationType>,
    /// Maximum number of conversations.
    pub limit: i64,
    /// Only conversations created after this timestamp (ns).
    pub created_after_ns: i64,
    /// Only conversations created before this timestamp (ns).
    pub created_before_ns: i64,
    /// Filter by consent states. Empty = all.
    pub consent_states: Vec<ConsentState>,
}

/// Options for sending a message.
#[derive(Debug, Clone, Copy, Default)]
pub struct SendOptions {
    /// Whether to include in push notifications (default: true).
    pub should_push: bool,
}

/// Disappearing message settings.
#[derive(Debug, Clone, Copy, Default)]
pub struct DisappearingSettings {
    /// Start timestamp (ns).
    pub from_ns: i64,
    /// Duration (ns).
    pub in_ns: i64,
}

/// Full permission policy set for a conversation.
#[derive(Debug, Clone, Copy)]
pub struct PermissionPolicySet {
    /// Policy for adding members.
    pub add_member: PermissionPolicy,
    /// Policy for removing members.
    pub remove_member: PermissionPolicy,
    /// Policy for adding admins.
    pub add_admin: PermissionPolicy,
    /// Policy for removing admins.
    pub remove_admin: PermissionPolicy,
    /// Policy for updating the group name.
    pub update_group_name: PermissionPolicy,
    /// Policy for updating the group description.
    pub update_group_description: PermissionPolicy,
    /// Policy for updating the group image URL.
    pub update_group_image_url: PermissionPolicy,
    /// Policy for updating disappearing message settings.
    pub update_message_disappearing: PermissionPolicy,
    /// Policy for updating app data.
    pub update_app_data: PermissionPolicy,
}

/// Group permissions (preset + full policy set).
#[derive(Debug, Clone, Copy)]
pub struct Permissions {
    /// The permissions preset used when creating the group.
    pub preset: GroupPermissionsPreset,
    /// The full set of per-action policies.
    pub policies: PermissionPolicySet,
}

/// Conversation metadata (creator + type).
#[derive(Debug, Clone)]
pub struct ConversationMetadata {
    /// The inbox ID of the conversation creator.
    pub creator_inbox_id: String,
    /// The type of conversation.
    pub conversation_type: ConversationType,
}

/// A single cursor entry for debug info.
#[derive(Debug, Clone, Copy)]
pub struct Cursor {
    /// Originator node ID.
    pub originator_id: u32,
    /// Sequence number within the originator.
    pub sequence_id: u64,
}

/// Conversation debug information.
#[derive(Debug, Clone)]
pub struct ConversationDebugInfo {
    /// Current MLS epoch.
    pub epoch: u64,
    /// Whether a fork has been detected.
    pub maybe_forked: bool,
    /// Human-readable fork details.
    pub fork_details: Option<String>,
    /// Whether the commit log is forked. `None` = unknown.
    pub is_commit_log_forked: Option<bool>,
    /// Local commit log summary.
    pub local_commit_log: Option<String>,
    /// Remote commit log summary.
    pub remote_commit_log: Option<String>,
    /// Cursor entries for each originator.
    pub cursors: Vec<Cursor>,
}

/// A single HMAC key entry.
#[derive(Debug, Clone)]
pub struct HmacKey {
    /// The raw key bytes.
    pub key: Vec<u8>,
    /// The epoch this key belongs to.
    pub epoch: i64,
}

/// HMAC keys for a conversation group.
#[derive(Debug, Clone)]
pub struct HmacKeyEntry {
    /// Hex-encoded group ID.
    pub group_id: String,
    /// HMAC keys for each epoch.
    pub keys: Vec<HmacKey>,
}

/// Per-inbox last-read timestamp.
#[derive(Debug, Clone)]
pub struct LastReadTime {
    /// The inbox ID.
    pub inbox_id: String,
    /// Last-read timestamp in nanoseconds.
    pub timestamp_ns: i64,
}

/// MLS API call statistics.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiStats {
    /// Number of `upload_key_package` calls.
    pub upload_key_package: i64,
    /// Number of `fetch_key_package` calls.
    pub fetch_key_package: i64,
    /// Number of `send_group_messages` calls.
    pub send_group_messages: i64,
    /// Number of `send_welcome_messages` calls.
    pub send_welcome_messages: i64,
    /// Number of `query_group_messages` calls.
    pub query_group_messages: i64,
    /// Number of `query_welcome_messages` calls.
    pub query_welcome_messages: i64,
    /// Number of `subscribe_messages` calls.
    pub subscribe_messages: i64,
    /// Number of `subscribe_welcomes` calls.
    pub subscribe_welcomes: i64,
    /// Number of `publish_commit_log` calls.
    pub publish_commit_log: i64,
    /// Number of `query_commit_log` calls.
    pub query_commit_log: i64,
    /// Number of `get_newest_group_message` calls.
    pub get_newest_group_message: i64,
}

/// Identity API call statistics.
#[derive(Debug, Clone, Copy, Default)]
pub struct IdentityStats {
    /// Number of `publish_identity_update` calls.
    pub publish_identity_update: i64,
    /// Number of `get_identity_updates_v2` calls.
    pub get_identity_updates_v2: i64,
    /// Number of `get_inbox_ids` calls.
    pub get_inbox_ids: i64,
    /// Number of `verify_smart_contract_wallet_signature` calls.
    pub verify_smart_contract_wallet_signature: i64,
}

/// Key package status for an installation.
#[derive(Debug, Clone)]
pub struct KeyPackageStatus {
    /// Hex-encoded installation ID.
    pub installation_id: String,
    /// Whether the key package is valid.
    pub valid: bool,
    /// `not_before` timestamp (0 if unavailable).
    pub not_before: u64,
    /// `not_after` timestamp (0 if unavailable).
    pub not_after: u64,
    /// Validation error message, if any.
    pub validation_error: Option<String>,
}

/// Result of a sync operation.
#[derive(Debug, Clone, Copy)]
pub struct SyncResult {
    /// Number of conversations successfully synced.
    pub synced: u32,
    /// Number of conversations eligible for sync.
    pub eligible: u32,
}

/// Snapshot of an inbox's identity state.
#[derive(Debug, Clone)]
pub struct InboxState {
    /// The inbox ID.
    pub inbox_id: String,
    /// Recovery identifier.
    pub recovery_identifier: String,
    /// Associated account identifiers.
    pub identifiers: Vec<String>,
    /// Installation IDs (hex).
    pub installation_ids: Vec<String>,
}

/// Trait for signing messages during XMTP identity operations.
pub trait Signer: Send + Sync {
    /// The account identifier for this signer.
    fn identifier(&self) -> AccountIdentifier;

    /// Sign the given text and return raw signature bytes.
    fn sign(&self, text: &str) -> crate::error::Result<Vec<u8>>;

    /// Whether this is a smart contract wallet (ERC-1271). Default: `false`.
    fn is_smart_wallet(&self) -> bool {
        false
    }

    /// EVM chain ID for SCW verification.
    fn chain_id(&self) -> u64 {
        1
    }

    /// Block number for SCW verification. 0 = latest.
    fn block_number(&self) -> u64 {
        0
    }
}
