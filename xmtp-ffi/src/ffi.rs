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
}

/// Opaque conversation handle.
pub struct XmtpConversation {
    pub(crate) inner: InnerGroup,
}

/// Opaque signature request handle.
pub struct XmtpSignatureRequest {
    pub(crate) request: std::sync::Arc<tokio::sync::Mutex<xmtp_id::associations::builder::SignatureRequest>>,
    pub(crate) scw_verifier: std::sync::Arc<Box<dyn xmtp_id::scw_verifier::SmartContractSignatureVerifier>>,
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
    #[allow(dead_code)]
    pub(crate) last_message: Option<xmtp_db::group_message::StoredGroupMessage>,
}

/// A single group member.
pub struct XmtpGroupMember {
    pub(crate) inbox_id: *mut c_char,
    pub(crate) permission_level: i32, // 0=Member, 1=Admin, 2=SuperAdmin
}

/// A list of group members.
pub struct XmtpGroupMemberList {
    pub(crate) members: Vec<XmtpGroupMember>,
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
            unsafe { *buf = 0; }
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
pub(crate) unsafe fn c_str_to_string(s: *const c_char) -> Result<String, Box<dyn std::error::Error>> {
    if s.is_null() {
        return Err("null string pointer".into());
    }
    Ok(unsafe { CStr::from_ptr(s) }.to_str()?.to_owned())
}

/// Convert a nullable C string to `Option<String>`.
pub(crate) unsafe fn c_str_to_option(s: *const c_char) -> Result<Option<String>, Box<dyn std::error::Error>> {
    if s.is_null() {
        return Ok(None);
    }
    Ok(Some(unsafe { CStr::from_ptr(s) }.to_str()?.to_owned()))
}

/// Allocate a new C string from a Rust `&str`. Caller must free with [`xmtp_free_string`].
pub(crate) fn to_c_string(s: &str) -> *mut c_char {
    CString::new(s).map(CString::into_raw).unwrap_or(std::ptr::null_mut())
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

/// Validate a pointer and create a safe mutable reference.
#[allow(dead_code)]
pub(crate) unsafe fn mut_from<'a, T>(ptr: *mut T) -> Result<&'a mut T, Box<dyn std::error::Error>> {
    if ptr.is_null() {
        return Err("null handle".into());
    }
    Ok(unsafe { &mut *ptr })
}

/// Box a value and return a raw pointer.
pub(crate) fn into_raw<T>(val: T) -> *mut T {
    Box::into_raw(Box::new(val))
}

/// Write a raw pointer into an output parameter.
pub(crate) unsafe fn write_out<T>(out: *mut *mut T, val: T) -> Result<(), Box<dyn std::error::Error>> {
    if out.is_null() {
        return Err("null output pointer".into());
    }
    unsafe { *out = into_raw(val); }
    Ok(())
}

// ---------------------------------------------------------------------------
// Optional logger initialization
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
