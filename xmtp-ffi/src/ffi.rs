//! Core FFI infrastructure: error handling, runtime, memory helpers, type aliases.

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char};
use std::sync::OnceLock;
use tokio::runtime::Runtime;

// ---------------------------------------------------------------------------
// Concrete type aliases (resolve all libxmtp generics once)
// ---------------------------------------------------------------------------

/// Fully resolved client type.
pub type InnerClient = xmtp_mls::Client<xmtp_mls::MlsContext>;

/// Fully resolved group type.
pub type InnerGroup = xmtp_mls::groups::MlsGroup<xmtp_mls::MlsContext>;

// ---------------------------------------------------------------------------
// Opaque handles exposed to C
// ---------------------------------------------------------------------------

/// Opaque client handle.
pub struct XmtpClient {
    pub(crate) inner: std::sync::Arc<InnerClient>,
    /// The account identifier used to create this client.
    pub(crate) account_identifier: String,
}

/// Opaque conversation handle.
pub struct XmtpConversation {
    pub(crate) inner: InnerGroup,
}

/// Opaque signature request handle.
pub struct XmtpSignatureRequest {
    pub(crate) request:
        std::sync::Arc<tokio::sync::Mutex<xmtp_id::associations::builder::SignatureRequest>>,
    pub(crate) scw_verifier:
        std::sync::Arc<Box<dyn xmtp_id::scw_verifier::SmartContractSignatureVerifier>>,
}

/// Opaque stream handle.
pub struct XmtpStreamHandle {
    pub(crate) abort: std::sync::Arc<Box<dyn xmtp_common::AbortHandle>>,
}

// ---------------------------------------------------------------------------
// Callback types
// ---------------------------------------------------------------------------

/// Callback for conversation stream events.
pub type FnConversationCallback =
    unsafe extern "C" fn(conversation: *mut XmtpConversation, context: *mut std::ffi::c_void);

/// Callback for message stream events.
pub type FnMessageCallback =
    unsafe extern "C" fn(message: *mut XmtpMessage, context: *mut std::ffi::c_void);

/// Callback invoked when a stream closes (either normally or on error).
/// Receives the opaque context pointer.
pub type FnOnCloseCallback = unsafe extern "C" fn(context: *mut std::ffi::c_void);

/// Callback for consent stream events.
pub type FnConsentCallback = unsafe extern "C" fn(
    records: *mut XmtpConsentRecord,
    count: i32,
    context: *mut std::ffi::c_void,
);

/// Callback for message deletion stream events.
/// Receives the message ID as a hex string (caller must free) and context.
pub type FnMessageDeletionCallback =
    unsafe extern "C" fn(message_id: *mut c_char, context: *mut std::ffi::c_void);

/// Callback for preference stream events.
pub type FnPreferenceCallback = unsafe extern "C" fn(
    updates: *mut XmtpPreferenceUpdate,
    count: i32,
    context: *mut std::ffi::c_void,
);

// ---------------------------------------------------------------------------
// Data transfer types (flat, repr(C))
// ---------------------------------------------------------------------------

/// A stored message exposed to C.
#[allow(dead_code)]
pub struct XmtpMessage {
    pub(crate) inner: xmtp_db::group_message::StoredGroupMessage,
}

/// A list of messages.
pub struct XmtpMessageList {
    pub(crate) items: Vec<xmtp_db::group_message::StoredGroupMessage>,
}

/// A list of conversations returned from queries.
pub struct XmtpConversationList {
    pub(crate) items: Vec<XmtpConversationListItem>,
}

pub struct XmtpConversationListItem {
    pub(crate) group: InnerGroup,
}

/// A single group member.
pub struct XmtpGroupMember {
    pub(crate) inbox_id: *mut c_char,
    pub(crate) permission_level: i32, // 0=Member, 1=Admin, 2=SuperAdmin
    pub(crate) consent_state: i32,    // 0=Unknown, 1=Allowed, 2=Denied
    /// Null-terminated array of account identifier strings. Each must be freed.
    pub(crate) account_identifiers: *mut *mut c_char,
    pub(crate) account_identifiers_count: i32,
    /// Null-terminated array of hex-encoded installation ID strings. Each must be freed.
    pub(crate) installation_ids: *mut *mut c_char,
    pub(crate) installation_ids_count: i32,
}

/// A list of group members.
pub struct XmtpGroupMemberList {
    pub(crate) members: Vec<XmtpGroupMember>,
}

/// A single inbox state entry (batch query result).
pub struct XmtpInboxStateItem {
    pub(crate) inbox_id: *mut c_char,
    pub(crate) recovery_identifier: *mut c_char,
    pub(crate) identifiers: *mut *mut c_char,
    pub(crate) identifiers_count: i32,
    pub(crate) installation_ids: *mut *mut c_char,
    pub(crate) installation_ids_count: i32,
}

/// A list of inbox states.
pub struct XmtpInboxStateList {
    pub(crate) items: Vec<XmtpInboxStateItem>,
}

/// A consent record exposed to C.
#[repr(C)]
pub struct XmtpConsentRecord {
    /// Entity type: 0=InboxId, 1=ConversationId
    pub entity_type: i32,
    /// Consent state: 0=Unknown, 1=Allowed, 2=Denied
    pub state: i32,
    /// Entity identifier string.
    pub entity: *mut c_char,
}

/// A preference update exposed to C.
#[repr(C)]
pub struct XmtpPreferenceUpdate {
    /// Update kind: 0=Consent, 1=HmacKey
    pub kind: i32,
    /// For Consent: the consent record. For HmacKey: zeroed.
    pub consent: XmtpConsentRecord,
    /// For HmacKey: the key bytes. For Consent: null/0.
    pub hmac_key: *mut u8,
    pub hmac_key_len: i32,
}

/// Options for sending a message.
#[repr(C)]
pub struct XmtpSendOpts {
    /// Whether to send a push notification. 1 = yes (default), 0 = no.
    pub should_push: i32,
}

/// MLS API call statistics (request counts).
#[repr(C)]
pub struct XmtpApiStats {
    pub upload_key_package: i64,
    pub fetch_key_package: i64,
    pub send_group_messages: i64,
    pub send_welcome_messages: i64,
    pub query_group_messages: i64,
    pub query_welcome_messages: i64,
    pub subscribe_messages: i64,
    pub subscribe_welcomes: i64,
    pub publish_commit_log: i64,
    pub query_commit_log: i64,
    pub get_newest_group_message: i64,
}

/// Identity API call statistics (request counts).
#[repr(C)]
pub struct XmtpIdentityStats {
    pub publish_identity_update: i64,
    pub get_identity_updates_v2: i64,
    pub get_inbox_ids: i64,
    pub verify_smart_contract_wallet_signature: i64,
}

/// Conversation debug info (epoch, fork status, commit logs).
#[repr(C)]
pub struct XmtpConversationDebugInfo {
    pub epoch: u64,
    pub maybe_forked: i32,
    pub fork_details: *mut c_char,
    /// -1 = unknown, 0 = no, 1 = yes
    pub is_commit_log_forked: i32,
    pub local_commit_log: *mut c_char,
    pub remote_commit_log: *mut c_char,
}

/// A single HMAC key (42-byte key + epoch).
#[repr(C)]
pub struct XmtpHmacKey {
    pub key: *mut u8,
    pub key_len: i32,
    pub epoch: i64,
}

/// A list of HMAC keys for one conversation.
#[repr(C)]
pub struct XmtpHmacKeyEntry {
    /// Hex-encoded group ID.
    pub group_id: *mut c_char,
    pub keys: *mut XmtpHmacKey,
    pub keys_count: i32,
}

/// A map of conversation ID → HMAC keys.
pub struct XmtpHmacKeyMap {
    pub(crate) entries: Vec<XmtpHmacKeyEntry>,
}

/// Options for device sync archive operations.
#[repr(C)]
pub struct XmtpArchiveOptions {
    /// Bitmask of element selections: bit 0 = Messages, bit 1 = Consent.
    pub elements: i32,
    /// Start timestamp filter (ns). 0 = no filter.
    pub start_ns: i64,
    /// End timestamp filter (ns). 0 = no filter.
    pub end_ns: i64,
    /// Whether to exclude disappearing messages. 0 = include, 1 = exclude.
    pub exclude_disappearing_messages: i32,
}

/// Info about an available archive in the sync group.
#[repr(C)]
pub struct XmtpAvailableArchive {
    pub pin: *mut c_char,
    pub backup_version: u16,
    pub exported_at_ns: i64,
    pub sent_by_installation: *mut u8,
    pub sent_by_installation_len: i32,
}

/// A list of available archives.
pub struct XmtpAvailableArchiveList {
    pub(crate) items: Vec<XmtpAvailableArchive>,
}

/// Opaque handle for gateway authentication credentials.
pub struct XmtpAuthHandle {
    pub(crate) inner: xmtp_api_d14n::AuthHandle,
}

/// Key package status for an installation.
#[repr(C)]
pub struct XmtpKeyPackageStatus {
    /// Installation ID as hex string (owned).
    pub installation_id: *mut c_char,
    /// 1 if valid, 0 if validation error.
    pub valid: i32,
    /// not_before timestamp (0 if unavailable).
    pub not_before: u64,
    /// not_after timestamp (0 if unavailable).
    pub not_after: u64,
    /// Validation error message (null if no error, owned).
    pub validation_error: *mut c_char,
}

/// A list of key package statuses.
pub struct XmtpKeyPackageStatusList {
    pub(crate) items: Vec<XmtpKeyPackageStatus>,
}

/// Inbox update count entry (inbox_id → count).
#[repr(C)]
pub struct XmtpInboxUpdateCount {
    pub inbox_id: *mut c_char,
    pub count: u32,
}

/// A list of inbox update counts.
pub struct XmtpInboxUpdateCountList {
    pub(crate) items: Vec<XmtpInboxUpdateCount>,
}

/// Group metadata (creator + conversation type).
#[repr(C)]
pub struct XmtpGroupMetadata {
    /// Creator inbox ID (owned string).
    pub creator_inbox_id: *mut c_char,
    /// Conversation type: 0=Group, 1=DM, 2=Sync.
    pub conversation_type: i32,
}

/// Permission policy set for a conversation.
/// Each field is an i32 encoding:
///   0=Allow, 1=Deny, 2=Admin, 3=SuperAdmin, 4=DoesNotExist, 5=Other
#[repr(C)]
pub struct XmtpPermissionPolicySet {
    pub add_member_policy: i32,
    pub remove_member_policy: i32,
    pub add_admin_policy: i32,
    pub remove_admin_policy: i32,
    pub update_group_name_policy: i32,
    pub update_group_description_policy: i32,
    pub update_group_image_url_square_policy: i32,
    pub update_message_disappearing_policy: i32,
    pub update_app_data_policy: i32,
}

/// Group permissions (policy type + policy set).
#[repr(C)]
pub struct XmtpGroupPermissions {
    /// 0=Default(AllMembers), 1=AdminOnly, 2=CustomPolicy.
    pub policy_type: i32,
    pub policy_set: XmtpPermissionPolicySet,
}

/// An enriched (decoded) message exposed to C.
/// Contains metadata + the original encoded content bytes for upper-layer decoding.
#[repr(C)]
pub struct XmtpEnrichedMessage {
    /// Message ID (hex string, owned).
    pub id: *mut c_char,
    /// Group ID (hex string, owned).
    pub group_id: *mut c_char,
    /// Sender inbox ID (owned string).
    pub sender_inbox_id: *mut c_char,
    /// Sender installation ID (hex string, owned).
    pub sender_installation_id: *mut c_char,
    /// Sent timestamp in nanoseconds.
    pub sent_at_ns: i64,
    /// Inserted-into-DB timestamp in nanoseconds.
    pub inserted_at_ns: i64,
    /// Message kind: 1=Application, 2=MembershipChange.
    pub kind: i32,
    /// Delivery status: 1=Unpublished, 2=Published, 3=Failed.
    pub delivery_status: i32,
    /// Content type ID string (e.g. "xmtp.org/text:1.0", owned).
    pub content_type: *mut c_char,
    /// Fallback text (nullable, owned).
    pub fallback_text: *mut c_char,
    /// Number of reactions.
    pub num_reactions: i32,
    /// Number of replies.
    pub num_replies: i32,
}

/// A list of enriched messages.
pub struct XmtpEnrichedMessageList {
    pub(crate) items: Vec<XmtpEnrichedMessage>,
}

/// Last-read-time entry (inbox_id → timestamp_ns).
#[repr(C)]
pub struct XmtpLastReadTimeEntry {
    pub inbox_id: *mut c_char,
    pub timestamp_ns: i64,
}

/// A list of last-read-time entries.
pub struct XmtpLastReadTimeList {
    pub(crate) items: Vec<XmtpLastReadTimeEntry>,
}

// ---------------------------------------------------------------------------
// Thread-local error
// ---------------------------------------------------------------------------

thread_local! {
    static LAST_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Store an error message for later retrieval.
pub(crate) fn set_last_error(msg: impl Into<String>) {
    LAST_ERROR.with(|e| *e.borrow_mut() = msg.into());
}

/// Get the length of the last error message (including NUL terminator).
/// Returns 0 if no error.
#[unsafe(no_mangle)]
pub extern "C" fn xmtp_last_error_length() -> i32 {
    LAST_ERROR.with(|e| {
        let s = e.borrow();
        if s.is_empty() { 0 } else { s.len() as i32 + 1 }
    })
}

/// Copy the last error message into `buf`. Returns bytes written (excluding NUL),
/// or -1 if `buf` is null or too small.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_last_error_message(buf: *mut c_char, buf_len: i32) -> i32 {
    if buf.is_null() || buf_len <= 0 {
        return -1;
    }
    LAST_ERROR.with(|e| {
        let s = e.borrow();
        if s.is_empty() {
            unsafe {
                *buf = 0;
            }
            return 0;
        }
        let bytes = s.as_bytes();
        let copy_len = bytes.len().min((buf_len - 1) as usize);
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf.cast::<u8>(), copy_len);
            *buf.add(copy_len) = 0;
        }
        copy_len as i32
    })
}

// ---------------------------------------------------------------------------
// Error-catching wrapper
// ---------------------------------------------------------------------------

/// Execute a closure, set thread-local error on failure, return code.
pub(crate) fn catch<F>(f: F) -> i32
where
    F: FnOnce() -> Result<(), Box<dyn std::error::Error>>,
{
    match f() {
        Ok(()) => 0,
        Err(e) => {
            set_last_error(e.to_string());
            -1
        }
    }
}

/// Execute an async closure on the shared runtime, set error on failure, return code.
pub(crate) fn catch_async<F, Fut>(f: F) -> i32
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<(), Box<dyn std::error::Error>>>,
{
    catch(|| runtime().block_on(f()))
}

// ---------------------------------------------------------------------------
// Shared tokio runtime
// ---------------------------------------------------------------------------

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// Get or initialize the global tokio runtime.
pub(crate) fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| Runtime::new().expect("failed to create tokio runtime"))
}

// ---------------------------------------------------------------------------
// String helpers
// ---------------------------------------------------------------------------

/// Convert a C string to an owned Rust `String`. Returns `Err` on null or invalid UTF-8.
pub(crate) unsafe fn c_str_to_string(
    s: *const c_char,
) -> Result<String, Box<dyn std::error::Error>> {
    if s.is_null() {
        return Err("null string pointer".into());
    }
    Ok(unsafe { CStr::from_ptr(s) }.to_str()?.to_owned())
}

/// Convert a nullable C string to `Option<String>`.
pub(crate) unsafe fn c_str_to_option(
    s: *const c_char,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    if s.is_null() {
        return Ok(None);
    }
    Ok(Some(unsafe { CStr::from_ptr(s) }.to_str()?.to_owned()))
}

/// Allocate a new C string from a Rust `&str`. Caller must free with [`xmtp_free_string`].
pub(crate) fn to_c_string(s: &str) -> *mut c_char {
    CString::new(s)
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

/// Free a string previously returned by this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_free_string(s: *mut c_char) {
    if !s.is_null() {
        drop(unsafe { CString::from_raw(s) });
    }
}

/// Free a byte buffer previously returned by this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_free_bytes(ptr: *mut u8, len: i32) {
    if !ptr.is_null() && len > 0 {
        drop(unsafe { Vec::from_raw_parts(ptr, len as usize, len as usize) });
    }
}

// ---------------------------------------------------------------------------
// Handle helpers
// ---------------------------------------------------------------------------

/// Validate a pointer and create a safe reference.
pub(crate) unsafe fn ref_from<'a, T>(ptr: *const T) -> Result<&'a T, Box<dyn std::error::Error>> {
    if ptr.is_null() {
        return Err("null handle".into());
    }
    Ok(unsafe { &*ptr })
}

/// Box a value and return a raw pointer.
pub(crate) fn into_raw<T>(val: T) -> *mut T {
    Box::into_raw(Box::new(val))
}

/// Write a raw pointer into an output parameter.
pub(crate) unsafe fn write_out<T>(
    out: *mut *mut T,
    val: T,
) -> Result<(), Box<dyn std::error::Error>> {
    if out.is_null() {
        return Err("null output pointer".into());
    }
    unsafe {
        *out = into_raw(val);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Identifier helpers (shared across modules)
// ---------------------------------------------------------------------------

/// Parse a single identifier from a C string + kind.
pub(crate) unsafe fn parse_identifier(
    s: *const c_char,
    kind: i32,
) -> Result<xmtp_id::associations::Identifier, Box<dyn std::error::Error>> {
    let val = unsafe { c_str_to_string(s)? };
    match kind {
        0 => Ok(xmtp_id::associations::Identifier::eth(val)?),
        1 => Ok(xmtp_id::associations::Identifier::passkey_str(&val, None)?),
        _ => Err("invalid identifier kind".into()),
    }
}

/// Collect parallel arrays of identifiers and kinds into `Vec<Identifier>`.
pub(crate) unsafe fn collect_identifiers(
    ptrs: *const *const c_char,
    kinds: *const i32,
    count: i32,
) -> Result<Vec<xmtp_id::associations::Identifier>, Box<dyn std::error::Error>> {
    if ptrs.is_null() || kinds.is_null() || count <= 0 {
        return Err("null pointer or invalid count".into());
    }
    let mut result = Vec::with_capacity(count as usize);
    for i in 0..count as usize {
        let s = unsafe { c_str_to_string(*ptrs.add(i))? };
        let kind = unsafe { *kinds.add(i) };
        result.push(match kind {
            0 => xmtp_id::associations::Identifier::eth(s)?,
            1 => xmtp_id::associations::Identifier::passkey_str(&s, None)?,
            _ => return Err("invalid identifier kind".into()),
        });
    }
    Ok(result)
}

/// Collect an array of C strings into `Vec<String>`.
pub(crate) unsafe fn collect_strings(
    ptrs: *const *const c_char,
    count: i32,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    if ptrs.is_null() || count <= 0 {
        return Err("null pointer or invalid count".into());
    }
    (0..count as usize)
        .map(|i| unsafe { c_str_to_string(*ptrs.add(i)) })
        .collect()
}

/// Convert a `Vec<String>` into a heap-allocated C string array.
/// Caller must free each string and the array itself.
pub(crate) fn string_vec_to_c(v: Vec<String>, out_count: *mut i32) -> *mut *mut c_char {
    let count = v.len();
    let mut ptrs: Vec<*mut c_char> = v.into_iter().map(|s| to_c_string(&s)).collect();
    let ptr = ptrs.as_mut_ptr();
    std::mem::forget(ptrs);
    unsafe {
        *out_count = count as i32;
    }
    ptr
}

// ---------------------------------------------------------------------------
// Array free helpers
// ---------------------------------------------------------------------------

/// Free a heap-allocated array of C strings.
pub(crate) fn free_c_string_array(arr: *mut *mut c_char, count: i32) {
    if arr.is_null() || count <= 0 {
        return;
    }
    for i in 0..count as usize {
        let s = unsafe { *arr.add(i) };
        if !s.is_null() {
            drop(unsafe { CString::from_raw(s) });
        }
    }
    drop(unsafe { Vec::from_raw_parts(arr, count as usize, count as usize) });
}

/// Free a string array returned by this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_free_string_array(arr: *mut *mut c_char, count: i32) {
    free_c_string_array(arr, count);
}

// ---------------------------------------------------------------------------
// Enum mapping helpers (shared across modules)
// ---------------------------------------------------------------------------

/// Map `ConsentState` → i32. 0=Unknown, 1=Allowed, 2=Denied.
pub(crate) fn consent_state_to_i32(s: xmtp_db::consent_record::ConsentState) -> i32 {
    match s {
        xmtp_db::consent_record::ConsentState::Unknown => 0,
        xmtp_db::consent_record::ConsentState::Allowed => 1,
        xmtp_db::consent_record::ConsentState::Denied => 2,
    }
}

/// Map i32 → `ConsentState`. Returns `Err` on invalid value.
pub(crate) fn i32_to_consent_state(
    v: i32,
) -> Result<xmtp_db::consent_record::ConsentState, Box<dyn std::error::Error>> {
    match v {
        0 => Ok(xmtp_db::consent_record::ConsentState::Unknown),
        1 => Ok(xmtp_db::consent_record::ConsentState::Allowed),
        2 => Ok(xmtp_db::consent_record::ConsentState::Denied),
        _ => Err("invalid consent state".into()),
    }
}

/// Map `ConsentType` → i32. 0=InboxId, 1=ConversationId.
pub(crate) fn consent_type_to_i32(t: xmtp_db::consent_record::ConsentType) -> i32 {
    match t {
        xmtp_db::consent_record::ConsentType::InboxId => 0,
        xmtp_db::consent_record::ConsentType::ConversationId => 1,
    }
}

/// Map i32 → `ConsentType`. Returns `Err` on invalid value.
pub(crate) fn i32_to_consent_type(
    v: i32,
) -> Result<xmtp_db::consent_record::ConsentType, Box<dyn std::error::Error>> {
    match v {
        0 => Ok(xmtp_db::consent_record::ConsentType::ConversationId),
        1 => Ok(xmtp_db::consent_record::ConsentType::InboxId),
        _ => Err("invalid entity type".into()),
    }
}

/// Convert a `StoredConsentRecord` to a C `XmtpConsentRecord`.
pub(crate) fn consent_record_to_c(
    r: &xmtp_db::consent_record::StoredConsentRecord,
) -> XmtpConsentRecord {
    XmtpConsentRecord {
        entity_type: consent_type_to_i32(r.entity_type),
        state: consent_state_to_i32(r.state),
        entity: to_c_string(&r.entity),
    }
}

// ---------------------------------------------------------------------------
// Logger initialization
// ---------------------------------------------------------------------------

static LOGGER_INIT: OnceLock<()> = OnceLock::new();

/// Initialize the tracing logger. Call at most once. `level` is a C string like
/// "debug", "info", "warn", "error", or "off". Pass null for default ("info").
/// Returns 0 on success.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_init_logger(level: *const c_char) -> i32 {
    catch(|| {
        use tracing_subscriber::{EnvFilter, fmt, prelude::*};
        LOGGER_INIT.get_or_init(|| {
            let filter_str = if level.is_null() {
                "info".to_string()
            } else {
                unsafe { CStr::from_ptr(level) }
                    .to_str()
                    .unwrap_or("info")
                    .to_string()
            };
            let filter = EnvFilter::builder().parse_lossy(&filter_str);
            tracing_subscriber::registry()
                .with(fmt::layer())
                .with(filter)
                .init();
        });
        Ok(())
    })
}
