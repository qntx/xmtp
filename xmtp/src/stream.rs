#![allow(unsafe_code)]
//! Callback-based streaming for real-time event subscriptions.
//!
//! Each `stream_*` function takes a callback closure and returns a [`StreamHandle`]
//! that controls the stream lifetime. The stream stops when the handle is dropped.

use std::ffi::{CStr, c_void};
use std::ptr;

use crate::client::Client;
use crate::conversation::Conversation;
use crate::error::{self, Result};
use crate::ffi::OwnedHandle;
use crate::types::{ConsentEntityType, ConsentState, ConversationType};

/// RAII handle for an active FFI stream.
///
/// Signals the stream to stop on drop and frees the underlying handle.
/// The boxed callback context is also freed on drop.
#[derive(Debug)]
pub struct StreamHandle {
    handle: OwnedHandle<xmtp_sys::XmtpFfiStreamHandle>,
    _ctx: Option<Box<dyn std::any::Any + Send>>,
}

impl StreamHandle {
    /// Signal the stream to stop. Safe to call multiple times.
    pub fn end(&self) {
        unsafe { xmtp_sys::xmtp_stream_end(self.handle.as_ptr()) };
    }

    /// Whether the stream has finished.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        unsafe { xmtp_sys::xmtp_stream_is_closed(self.handle.as_ptr()) == 1 }
    }
}

/// Generic helper to start a stream. Handles context boxing, error recovery,
/// and wrapping the result in a `StreamHandle`.
///
/// **Callers must pass a pre-erased trait object** (e.g. `Box<dyn Fn(…)>`) so
/// that the heap allocation contains a full fat pointer (data + vtable). The
/// corresponding trampoline then casts the context back to the same
/// `Box<dyn Fn(…)>` type, reading exactly 16 bytes — matching what was stored.
fn start_stream<F: Send + 'static>(
    callback: F,
    start: impl FnOnce(*mut c_void, *mut *mut xmtp_sys::XmtpFfiStreamHandle) -> i32,
) -> Result<StreamHandle> {
    let boxed = Box::new(callback);
    let ctx_ptr = Box::into_raw(boxed).cast::<c_void>();
    let mut out: *mut xmtp_sys::XmtpFfiStreamHandle = ptr::null_mut();
    let rc = start(ctx_ptr, &raw mut out);
    if rc != 0 {
        // Reclaim the context to avoid a leak on error.
        let _ = unsafe { Box::from_raw(ctx_ptr.cast::<F>()) };
        return Err(error::last_ffi_error());
    }
    let handle = OwnedHandle::new(out, xmtp_sys::xmtp_stream_free)?;
    let ctx_box = unsafe { Box::from_raw(ctx_ptr.cast::<F>()) };
    Ok(StreamHandle {
        handle,
        _ctx: Some(ctx_box),
    })
}

/// Stream new conversations. The callback receives each new [`Conversation`].
///
/// Pass `None` for `conversation_type` to receive all types.
///
/// # Errors
///
/// Returns an error if the FFI stream could not be started.
pub fn stream_conversations(
    client: &Client,
    conversation_type: Option<ConversationType>,
    callback: impl Fn(Conversation) + Send + 'static,
) -> Result<StreamHandle> {
    let client_ptr = client.handle.as_ptr();
    let conv_type = conversation_type.map_or(-1, |t| t as i32);
    let dyn_cb: Box<dyn Fn(Conversation) + Send> = Box::new(callback);
    start_stream(dyn_cb, |ctx, out| unsafe {
        xmtp_sys::xmtp_stream_conversations(
            client_ptr,
            conv_type,
            Some(conv_trampoline),
            None,
            ctx,
            out,
        )
    })
}

unsafe extern "C" fn conv_trampoline(
    conv: *mut xmtp_sys::XmtpFfiConversation,
    context: *mut c_void,
) {
    unsafe {
        if context.is_null() || conv.is_null() {
            return;
        }
        let cb = &*context.cast::<Box<dyn Fn(Conversation) + Send>>();
        if let Ok(c) = Conversation::from_raw(conv) {
            cb(c);
        }
    }
}

/// Stream all messages across conversations.
///
/// The callback receives `(message_id, conversation_id)` — both hex-encoded.
/// Pass `None` for `conversation_type` to receive from all types.
///
/// # Errors
///
/// Returns an error if the FFI stream could not be started.
pub fn stream_all_messages(
    client: &Client,
    conversation_type: Option<ConversationType>,
    consent_states: &[ConsentState],
    callback: impl Fn(String, String) + Send + 'static,
) -> Result<StreamHandle> {
    let client_ptr = client.handle.as_ptr();
    let conv_type = conversation_type.map_or(-1, |t| t as i32);
    let cs: Vec<i32> = consent_states.iter().map(|s| *s as i32).collect();
    let cs_ptr = if cs.is_empty() {
        ptr::null()
    } else {
        cs.as_ptr()
    };
    let cs_len = cs.len() as i32;
    let dyn_cb: Box<dyn Fn(String, String) + Send> = Box::new(callback);
    start_stream(dyn_cb, |ctx, out| unsafe {
        xmtp_sys::xmtp_stream_all_messages(
            client_ptr,
            conv_type,
            cs_ptr,
            cs_len,
            Some(msg_trampoline),
            None,
            ctx,
            out,
        )
    })
}

/// Stream messages for a single conversation.
///
/// The callback receives `(message_id, conversation_id)` — both hex-encoded.
///
/// # Errors
///
/// Returns an error if the FFI stream could not be started.
pub fn stream_messages(
    conversation: &Conversation,
    callback: impl Fn(String, String) + Send + 'static,
) -> Result<StreamHandle> {
    let conv_ptr = conversation.handle_ptr();
    let dyn_cb: Box<dyn Fn(String, String) + Send> = Box::new(callback);
    start_stream(dyn_cb, |ctx, out| unsafe {
        xmtp_sys::xmtp_conversation_stream_messages(
            conv_ptr,
            Some(msg_trampoline),
            None,
            ctx,
            out,
        )
    })
}

unsafe extern "C" fn msg_trampoline(msg: *mut xmtp_sys::XmtpFfiMessage, context: *mut c_void) {
    unsafe {
        if context.is_null() || msg.is_null() {
            if !msg.is_null() {
                xmtp_sys::xmtp_message_free(msg);
            }
            return;
        }
        // Extract message ID and group ID before freeing.
        let id_ptr = xmtp_sys::xmtp_single_message_id(msg);
        let gid_ptr = xmtp_sys::xmtp_single_message_group_id(msg);
        xmtp_sys::xmtp_message_free(msg);

        let cb = &*context.cast::<Box<dyn Fn(String, String) + Send>>();
        let id = if id_ptr.is_null() {
            String::new()
        } else {
            let s = CStr::from_ptr(id_ptr)
                .to_str()
                .unwrap_or_default()
                .to_owned();
            xmtp_sys::xmtp_free_string(id_ptr);
            s
        };
        let gid = if gid_ptr.is_null() {
            String::new()
        } else {
            let s = CStr::from_ptr(gid_ptr)
                .to_str()
                .unwrap_or_default()
                .to_owned();
            xmtp_sys::xmtp_free_string(gid_ptr);
            s
        };
        cb(id, gid);
    }
}

/// A consent state change event.
#[derive(Debug, Clone)]
pub struct ConsentUpdate {
    /// Entity type (group ID or inbox ID).
    pub entity_type: ConsentEntityType,
    /// The consent state.
    pub state: ConsentState,
    /// The entity identifier.
    pub entity: String,
}

/// Stream consent state changes.
///
/// # Errors
///
/// Returns an error if the FFI stream could not be started.
pub fn stream_consent(
    client: &Client,
    callback: impl Fn(Vec<ConsentUpdate>) + Send + 'static,
) -> Result<StreamHandle> {
    let client_ptr = client.handle.as_ptr();
    let dyn_cb: Box<dyn Fn(Vec<ConsentUpdate>) + Send> = Box::new(callback);
    start_stream(dyn_cb, |ctx, out| unsafe {
        xmtp_sys::xmtp_stream_consent(
            client_ptr,
            Some(consent_trampoline),
            None,
            ctx,
            out,
        )
    })
}

unsafe extern "C" fn consent_trampoline(
    records: *const xmtp_sys::XmtpFfiConsentRecord,
    count: i32,
    context: *mut c_void,
) {
    unsafe {
        if context.is_null() || records.is_null() || count <= 0 {
            return;
        }
        let cb = &*context.cast::<Box<dyn Fn(Vec<ConsentUpdate>) + Send>>();
        let slice = std::slice::from_raw_parts(records, count.unsigned_abs() as usize);
        let updates: Vec<ConsentUpdate> = slice
            .iter()
            .filter_map(|r| {
                let entity_type = ConsentEntityType::from_ffi(r.entity_type as i32)?;
                let state = ConsentState::from_ffi(r.state as i32)?;
                let entity = CStr::from_ptr(r.entity).to_str().ok()?.to_owned();
                Some(ConsentUpdate {
                    entity_type,
                    state,
                    entity,
                })
            })
            .collect();
        if !updates.is_empty() {
            cb(updates);
        }
    }
}

/// A user preference update event.
#[derive(Debug, Clone)]
pub struct PreferenceUpdate {
    /// The kind of preference change (0 = Consent, 1 = `HmacKey`).
    pub kind: i32,
    /// For Consent updates: the consent change details.
    pub consent: Option<ConsentUpdate>,
}

/// Stream preference updates.
///
/// # Errors
///
/// Returns an error if the FFI stream could not be started.
pub fn stream_preferences(
    client: &Client,
    callback: impl Fn(Vec<PreferenceUpdate>) + Send + 'static,
) -> Result<StreamHandle> {
    let client_ptr = client.handle.as_ptr();
    let dyn_cb: Box<dyn Fn(Vec<PreferenceUpdate>) + Send> = Box::new(callback);
    start_stream(dyn_cb, |ctx, out| unsafe {
        xmtp_sys::xmtp_stream_preferences(
            client_ptr,
            Some(pref_trampoline),
            None,
            ctx,
            out,
        )
    })
}

unsafe extern "C" fn pref_trampoline(
    updates: *const xmtp_sys::XmtpFfiPreferenceUpdate,
    count: i32,
    context: *mut c_void,
) {
    unsafe {
        if context.is_null() || updates.is_null() || count <= 0 {
            return;
        }
        let cb = &*context.cast::<Box<dyn Fn(Vec<PreferenceUpdate>) + Send>>();
        let slice = std::slice::from_raw_parts(updates, count.unsigned_abs() as usize);
        let items: Vec<PreferenceUpdate> = slice
            .iter()
            .map(|u| {
                let kind = u.kind as i32;
                let consent = if kind == 0 {
                    // Consent update — extract from the embedded record.
                    let r = &u.consent;
                    let et = ConsentEntityType::from_ffi(r.entity_type as i32);
                    let st = ConsentState::from_ffi(r.state as i32);
                    let entity = if r.entity.is_null() {
                        String::new()
                    } else {
                        CStr::from_ptr(r.entity)
                            .to_str()
                            .unwrap_or_default()
                            .to_owned()
                    };
                    et.zip(st).map(|(entity_type, state)| ConsentUpdate {
                        entity_type,
                        state,
                        entity,
                    })
                } else {
                    None
                };
                PreferenceUpdate { kind, consent }
            })
            .collect();
        if !items.is_empty() {
            cb(items);
        }
    }
}

/// Stream message deletion events. The callback receives the hex message ID.
///
/// # Errors
///
/// Returns an error if the FFI stream could not be started.
pub fn stream_message_deletions(
    client: &Client,
    callback: impl Fn(String) + Send + 'static,
) -> Result<StreamHandle> {
    let client_ptr = client.handle.as_ptr();
    let dyn_cb: Box<dyn Fn(String) + Send> = Box::new(callback);
    start_stream(dyn_cb, |ctx, out| unsafe {
        xmtp_sys::xmtp_stream_message_deletions(
            client_ptr,
            Some(deletion_trampoline),
            None,
            ctx,
            out,
        )
    })
}

unsafe extern "C" fn deletion_trampoline(
    message_id: *const std::ffi::c_char,
    context: *mut c_void,
) {
    unsafe {
        if context.is_null() || message_id.is_null() {
            return;
        }
        let cb = &*context.cast::<Box<dyn Fn(String) + Send>>();
        if let Ok(id) = CStr::from_ptr(message_id).to_str() {
            cb(id.to_owned());
        }
    }
}
