#![allow(unsafe_code)]
//! Channel-based streaming for real-time event subscriptions.
//!
//! Each function returns a [`Subscription<T>`] that yields typed events via
//! an internal channel. Implements [`Iterator`] for idiomatic consumption.
//! The stream stops when the subscription is dropped.

use std::ffi::{CStr, c_void};
use std::sync::mpsc;
use std::{fmt, ptr};

use crate::client::Client;
use crate::conversation::Conversation;
use crate::error::{self, Result};
use crate::ffi::OwnedHandle;
use crate::types::{ConsentEntityType, ConsentState, ConversationType};

/// A real-time event subscription backed by an internal channel.
///
/// Yields events of type `T` via [`recv`](Self::recv),
/// [`try_recv`](Self::try_recv), or [`Iterator`] consumption.
/// The underlying FFI stream is stopped when this value is dropped.
pub struct Subscription<T> {
    rx: mpsc::Receiver<T>,
    handle: OwnedHandle<xmtp_sys::XmtpFfiStreamHandle>,
    _ctx: Option<Box<dyn std::any::Any + Send>>,
}

impl<T> Subscription<T> {
    /// Block until the next event, or `None` if the stream ended.
    #[must_use]
    pub fn recv(&self) -> Option<T> {
        self.rx.recv().ok()
    }

    /// Non-blocking receive. Returns `None` if no event is ready.
    #[must_use]
    pub fn try_recv(&self) -> Option<T> {
        self.rx.try_recv().ok()
    }

    /// Signal the stream to stop. Safe to call multiple times.
    pub fn close(&self) {
        unsafe { xmtp_sys::xmtp_stream_end(self.handle.as_ptr()) };
    }

    /// Whether the stream has finished.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        unsafe { xmtp_sys::xmtp_stream_is_closed(self.handle.as_ptr()) == 1 }
    }
}

impl<T> Iterator for Subscription<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        self.rx.recv().ok()
    }
}

impl<T> Drop for Subscription<T> {
    fn drop(&mut self) {
        // Signal the FFI stream to stop before OwnedHandle frees the resource.
        unsafe { xmtp_sys::xmtp_stream_end(self.handle.as_ptr()) };
    }
}

impl<T> fmt::Debug for Subscription<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Subscription")
            .field("is_closed", &self.is_closed())
            .finish()
    }
}

/// A new-message event from a message stream.
#[derive(Debug, Clone)]
pub struct MessageEvent {
    /// Hex-encoded message ID.
    pub message_id: String,
    /// Hex-encoded conversation (group) ID.
    pub conversation_id: String,
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

/// A user preference update event.
#[derive(Debug, Clone)]
pub struct PreferenceUpdate {
    /// The kind of preference change (0 = Consent, 1 = `HmacKey`).
    pub kind: i32,
    /// For Consent updates: the consent change details.
    pub consent: Option<ConsentUpdate>,
}

/// Start an FFI stream and wire its callback to a channel receiver.
///
/// The callback `F` is a pre-erased trait object (`Box<dyn Fn(…)>`) whose
/// raw pointer is passed to the FFI trampoline. The corresponding trampoline
/// casts the context back to the same type, reading the fat pointer correctly.
fn subscribe<T: Send + 'static, F: Send + 'static>(
    callback: F,
    rx: mpsc::Receiver<T>,
    start: impl FnOnce(*mut c_void, *mut *mut xmtp_sys::XmtpFfiStreamHandle) -> i32,
) -> Result<Subscription<T>> {
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
    Ok(Subscription {
        rx,
        handle,
        _ctx: Some(ctx_box),
    })
}

/// Stream new conversations.
///
/// Pass `None` for `conversation_type` to receive all types.
pub fn conversations(
    client: &Client,
    conversation_type: Option<ConversationType>,
) -> Result<Subscription<Conversation>> {
    let (tx, rx) = mpsc::channel();
    let client_ptr = client.handle.as_ptr();
    let conv_type = conversation_type.map_or(-1, |t| t as i32);
    let cb: Box<dyn Fn(Conversation) + Send> = Box::new(move |conv| {
        let _ = tx.send(conv);
    });
    subscribe(cb, rx, |ctx, out| unsafe {
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

/// Stream all messages across conversations.
///
/// Pass `None` for `conversation_type` to receive from all types.
pub fn messages(
    client: &Client,
    conversation_type: Option<ConversationType>,
    consent_states: &[ConsentState],
) -> Result<Subscription<MessageEvent>> {
    let (tx, rx) = mpsc::channel();
    let client_ptr = client.handle.as_ptr();
    let conv_type = conversation_type.map_or(-1, |t| t as i32);
    let cs: Vec<i32> = consent_states.iter().map(|s| *s as i32).collect();
    let cs_ptr = if cs.is_empty() {
        ptr::null()
    } else {
        cs.as_ptr()
    };
    let cs_len = cs.len() as i32;
    let cb: Box<dyn Fn(String, String) + Send> = Box::new(move |mid, cid| {
        let _ = tx.send(MessageEvent {
            message_id: mid,
            conversation_id: cid,
        });
    });
    subscribe(cb, rx, |ctx, out| unsafe {
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
pub fn conversation_messages(conversation: &Conversation) -> Result<Subscription<MessageEvent>> {
    let (tx, rx) = mpsc::channel();
    let conv_ptr = conversation.handle_ptr();
    let cb: Box<dyn Fn(String, String) + Send> = Box::new(move |mid, cid| {
        let _ = tx.send(MessageEvent {
            message_id: mid,
            conversation_id: cid,
        });
    });
    subscribe(cb, rx, |ctx, out| unsafe {
        xmtp_sys::xmtp_conversation_stream_messages(conv_ptr, Some(msg_trampoline), None, ctx, out)
    })
}

/// Stream consent state changes.
pub fn consent(client: &Client) -> Result<Subscription<Vec<ConsentUpdate>>> {
    let (tx, rx) = mpsc::channel();
    let client_ptr = client.handle.as_ptr();
    let cb: Box<dyn Fn(Vec<ConsentUpdate>) + Send> = Box::new(move |updates| {
        let _ = tx.send(updates);
    });
    subscribe(cb, rx, |ctx, out| unsafe {
        xmtp_sys::xmtp_stream_consent(client_ptr, Some(consent_trampoline), None, ctx, out)
    })
}

/// Stream preference updates.
pub fn preferences(client: &Client) -> Result<Subscription<Vec<PreferenceUpdate>>> {
    let (tx, rx) = mpsc::channel();
    let client_ptr = client.handle.as_ptr();
    let cb: Box<dyn Fn(Vec<PreferenceUpdate>) + Send> = Box::new(move |updates| {
        let _ = tx.send(updates);
    });
    subscribe(cb, rx, |ctx, out| unsafe {
        xmtp_sys::xmtp_stream_preferences(client_ptr, Some(pref_trampoline), None, ctx, out)
    })
}

/// Stream message deletion events. Each event yields the hex message ID.
pub fn message_deletions(client: &Client) -> Result<Subscription<String>> {
    let (tx, rx) = mpsc::channel();
    let client_ptr = client.handle.as_ptr();
    let cb: Box<dyn Fn(String) + Send> = Box::new(move |id| {
        let _ = tx.send(id);
    });
    subscribe(cb, rx, |ctx, out| unsafe {
        xmtp_sys::xmtp_stream_message_deletions(
            client_ptr,
            Some(deletion_trampoline),
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
