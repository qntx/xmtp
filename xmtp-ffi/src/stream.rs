//! Callback-based streaming for conversations and messages.
//!
//! # Ownership model
//! - Conversation / message callbacks transfer ownership (`*mut`) — caller must free.
//! - Consent / preference / deletion callbacks lend data (`*const`) — valid only during callback.
//! - `on_close` receives a borrowed error string (null = normal close).
//!
//! # Lifecycle
//! `xmtp_stream_end(handle)` → signal stop.
//! `xmtp_stream_is_closed(handle)` → poll status.
//! `xmtp_stream_free(handle)` → release handle memory.

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use xmtp_common::StreamHandle;
use xmtp_mls::Client as MlsClient;
use xmtp_mls::groups::MlsGroup;

use crate::ffi::*;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a conversation type filter from an int.
fn parse_conv_type(v: i32) -> Option<xmtp_db::group::ConversationType> {
    match v {
        0 => Some(xmtp_db::group::ConversationType::Dm),
        1 => Some(xmtp_db::group::ConversationType::Group),
        2 => Some(xmtp_db::group::ConversationType::Sync),
        _ => None,
    }
}

/// Guard ensuring `on_close` is called at most once across data-error and close paths.
type OnCloseGuard = Arc<AtomicBool>;

/// Create a fresh guard (shared between data-callback and close-callback closures).
fn new_on_close_guard() -> OnCloseGuard {
    Arc::new(AtomicBool::new(false))
}

/// Invoke the on_close callback with a null error (normal close).
/// No-op if already called.
fn invoke_on_close_ok(on_close: Option<FnOnCloseCallback>, ctx: usize, guard: &OnCloseGuard) {
    if guard.swap(true, Ordering::AcqRel) {
        return; // already fired
    }
    if let Some(cb) = on_close {
        unsafe { cb(std::ptr::null(), ctx as *mut c_void) };
    }
}

/// Invoke the on_close callback with an error message.
/// No-op if already called.
fn invoke_on_close_err(
    on_close: Option<FnOnCloseCallback>,
    ctx: usize,
    err: &str,
    guard: &OnCloseGuard,
) {
    if guard.swap(true, Ordering::AcqRel) {
        return; // already fired
    }
    if let Some(cb) = on_close {
        let c_err = std::ffi::CString::new(err).unwrap_or_default();
        unsafe { cb(c_err.as_ptr(), ctx as *mut c_void) };
    }
}

/// Finalize a stream handle: wait_for_ready, extract abort handle, write to output.
fn finalize_stream(
    handle: &mut impl StreamHandle,
    out: *mut *mut FfiStreamHandle,
) -> Result<(), Box<dyn std::error::Error>> {
    runtime().block_on(handle.wait_for_ready());
    let abort = handle.abort_handle();
    unsafe {
        write_out(
            out,
            FfiStreamHandle {
                abort: Arc::new(abort),
            },
        )
    }
}

// ---------------------------------------------------------------------------
// Stream conversations
// ---------------------------------------------------------------------------

/// Stream new conversations. Callback receives owned `*mut FfiConversation` (caller must free).
/// `on_close(error, ctx)`: null error = normal close; non-null = borrowed error string.
/// Caller must end with `xmtp_stream_end` and free with `xmtp_stream_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_stream_conversations(
    client: *const FfiClient,
    conversation_type: i32,
    callback: FnConversationCallback,
    on_close: Option<FnOnCloseCallback>,
    context: *mut c_void,
    out: *mut *mut FfiStreamHandle,
) -> i32 {
    catch(|| {
        let _rt = runtime().enter();
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let conv_type = parse_conv_type(conversation_type);
        let ctx = context as usize;
        let guard = new_on_close_guard();
        let g1 = guard.clone();
        let g2 = guard;

        let mut handle = MlsClient::stream_conversations_with_callback(
            c.inner.clone(),
            conv_type,
            move |result| match result {
                Ok(group) => {
                    let ptr = into_raw(FfiConversation { inner: group });
                    unsafe { callback(ptr, ctx as *mut c_void) };
                }
                Err(e) => invoke_on_close_err(on_close, ctx, &e.to_string(), &g1),
            },
            move || invoke_on_close_ok(on_close, ctx, &g2),
            false,
        );
        finalize_stream(&mut handle, out)
    })
}

// ---------------------------------------------------------------------------
// Stream all messages
// ---------------------------------------------------------------------------

/// Stream all messages across conversations. Callback receives owned `*mut FfiMessage`.
/// `consent_states` / `consent_states_count`: optional filter (null/0 = all).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_stream_all_messages(
    client: *const FfiClient,
    conversation_type: i32,
    consent_states: *const i32,
    consent_states_count: i32,
    callback: FnMessageCallback,
    on_close: Option<FnOnCloseCallback>,
    context: *mut c_void,
    out: *mut *mut FfiStreamHandle,
) -> i32 {
    catch(|| {
        let _rt = runtime().enter();
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let conv_type = parse_conv_type(conversation_type);
        let consents = parse_consent_states(consent_states, consent_states_count);
        let ctx = context as usize;
        let guard = new_on_close_guard();
        let g1 = guard.clone();
        let g2 = guard;

        let mut handle = MlsClient::stream_all_messages_with_callback(
            c.inner.context.clone(),
            conv_type,
            consents,
            move |result| match result {
                Ok(msg) => {
                    let ptr = into_raw(FfiMessage { inner: msg });
                    unsafe { callback(ptr, ctx as *mut c_void) };
                }
                Err(e) => invoke_on_close_err(on_close, ctx, &e.to_string(), &g1),
            },
            move || invoke_on_close_ok(on_close, ctx, &g2),
        );
        finalize_stream(&mut handle, out)
    })
}

// ---------------------------------------------------------------------------
// Stream single conversation messages
// ---------------------------------------------------------------------------

/// Stream messages for a single conversation. Callback receives owned `*mut FfiMessage`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_stream_messages(
    conv: *const FfiConversation,
    callback: FnMessageCallback,
    on_close: Option<FnOnCloseCallback>,
    context: *mut c_void,
    out: *mut *mut FfiStreamHandle,
) -> i32 {
    catch(|| {
        let _rt = runtime().enter();
        let c = unsafe { ref_from(conv)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let ctx = context as usize;
        let guard = new_on_close_guard();
        let g1 = guard.clone();
        let g2 = guard;

        let mut handle = MlsGroup::stream_with_callback(
            c.inner.context.clone(),
            c.inner.group_id.clone(),
            move |result| match result {
                Ok(msg) => {
                    let ptr = into_raw(FfiMessage { inner: msg });
                    unsafe { callback(ptr, ctx as *mut c_void) };
                }
                Err(e) => invoke_on_close_err(on_close, ctx, &e.to_string(), &g1),
            },
            move || invoke_on_close_ok(on_close, ctx, &g2),
        );
        finalize_stream(&mut handle, out)
    })
}

// ---------------------------------------------------------------------------
// Stream consent updates
// ---------------------------------------------------------------------------

/// Stream consent state changes. Callback receives a borrowed array of consent records
/// (`*const FfiConsentRecord`) — valid only during the callback invocation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_stream_consent(
    client: *const FfiClient,
    callback: FnConsentCallback,
    on_close: Option<FnOnCloseCallback>,
    context: *mut c_void,
    out: *mut *mut FfiStreamHandle,
) -> i32 {
    catch(|| {
        let _rt = runtime().enter();
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let ctx = context as usize;

        let guard = new_on_close_guard();
        let g1 = guard.clone();
        let g2 = guard;

        let mut handle = MlsClient::stream_consent_with_callback(
            c.inner.clone(),
            move |result| match result {
                Ok(records) => {
                    let c_records: Vec<FfiConsentRecord> =
                        records.iter().map(consent_record_to_c).collect();
                    unsafe {
                        callback(
                            c_records.as_ptr(),
                            c_records.len() as i32,
                            ctx as *mut c_void,
                        )
                    };
                    // Free inner allocations that FfiConsentRecord doesn't Drop
                    for r in &c_records {
                        if !r.entity.is_null() {
                            drop(unsafe { std::ffi::CString::from_raw(r.entity) });
                        }
                    }
                }
                Err(e) => invoke_on_close_err(on_close, ctx, &e.to_string(), &g1),
            },
            move || invoke_on_close_ok(on_close, ctx, &g2),
        );
        finalize_stream(&mut handle, out)
    })
}

// ---------------------------------------------------------------------------
// Stream preference updates
// ---------------------------------------------------------------------------

/// Stream preference updates (consent changes + HMAC key rotations).
/// Callback receives a borrowed array (`*const FfiPreferenceUpdate`) — valid only during callback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_stream_preferences(
    client: *const FfiClient,
    callback: FnPreferenceCallback,
    on_close: Option<FnOnCloseCallback>,
    context: *mut c_void,
    out: *mut *mut FfiStreamHandle,
) -> i32 {
    catch(|| {
        let _rt = runtime().enter();
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let ctx = context as usize;

        let guard = new_on_close_guard();
        let g1 = guard.clone();
        let g2 = guard;

        let mut handle = MlsClient::stream_preferences_with_callback(
            c.inner.clone(),
            move |result| match result {
                Ok(updates) => {
                    use xmtp_mls::groups::device_sync::preference_sync::PreferenceUpdate;
                    let c_updates: Vec<FfiPreferenceUpdate> = updates
                        .into_iter()
                        .map(|u| match u {
                            PreferenceUpdate::Consent(r) => FfiPreferenceUpdate {
                                kind: FfiPreferenceUpdateKind::Consent,
                                consent: consent_record_to_c(&r),
                                hmac_key: std::ptr::null_mut(),
                                hmac_key_len: 0,
                            },
                            PreferenceUpdate::Hmac { key, .. } => {
                                let len = key.len() as i32;
                                let boxed = key.into_boxed_slice();
                                let ptr = Box::into_raw(boxed) as *mut u8;
                                FfiPreferenceUpdate {
                                    kind: FfiPreferenceUpdateKind::HmacKey,
                                    consent: FfiConsentRecord {
                                        entity_type: FfiConsentEntityType::GroupId,
                                        state: FfiConsentState::Unknown,
                                        entity: std::ptr::null_mut(),
                                    },
                                    hmac_key: ptr,
                                    hmac_key_len: len,
                                }
                            }
                        })
                        .collect();
                    unsafe {
                        callback(
                            c_updates.as_ptr(),
                            c_updates.len() as i32,
                            ctx as *mut c_void,
                        )
                    };
                    // Free inner allocations that FfiPreferenceUpdate doesn't Drop
                    for u in &c_updates {
                        if !u.consent.entity.is_null() {
                            drop(unsafe { std::ffi::CString::from_raw(u.consent.entity) });
                        }
                        if !u.hmac_key.is_null() && u.hmac_key_len > 0 {
                            drop(unsafe {
                                Box::from_raw(std::slice::from_raw_parts_mut(
                                    u.hmac_key,
                                    u.hmac_key_len as usize,
                                ))
                            });
                        }
                    }
                }
                Err(e) => invoke_on_close_err(on_close, ctx, &e.to_string(), &g1),
            },
            move || invoke_on_close_ok(on_close, ctx, &g2),
        );
        finalize_stream(&mut handle, out)
    })
}

// ---------------------------------------------------------------------------
// Stream message deletions
// ---------------------------------------------------------------------------

/// Stream message deletion events. Callback receives a borrowed hex message ID
/// (`*const c_char`) — valid only during the callback invocation.
/// Now includes `on_close` for API consistency with other stream functions.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_stream_message_deletions(
    client: *const FfiClient,
    callback: FnMessageDeletionCallback,
    on_close: Option<FnOnCloseCallback>,
    context: *mut c_void,
    out: *mut *mut FfiStreamHandle,
) -> i32 {
    catch(|| {
        let _rt = runtime().enter();
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let ctx = context as usize;

        let guard = new_on_close_guard();
        let g1 = guard;

        let mut handle =
            MlsClient::stream_message_deletions_with_callback(c.inner.clone(), move |result| {
                match result {
                    Ok(decoded) => {
                        let id_hex = hex::encode(&decoded.metadata.id);
                        let c_str = std::ffi::CString::new(id_hex).unwrap_or_default();
                        unsafe { callback(c_str.as_ptr(), ctx as *mut c_void) };
                        // c_str dropped here — borrowed during callback only
                    }
                    Err(e) => invoke_on_close_err(on_close, ctx, &e.to_string(), &g1),
                }
            });
        finalize_stream(&mut handle, out)
    })
}

// ---------------------------------------------------------------------------
// Stream lifecycle
// ---------------------------------------------------------------------------

/// Signal a stream to stop. Does NOT free the handle — call `xmtp_stream_free` afterwards.
/// Safe to call multiple times.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_stream_end(handle: *const FfiStreamHandle) {
    if let Ok(h) = unsafe { ref_from(handle) } {
        h.abort.end();
    }
}

/// Check if a stream has finished. Returns 1 if closed, 0 if active.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_stream_is_closed(handle: *const FfiStreamHandle) -> i32 {
    match unsafe { ref_from(handle) } {
        Ok(h) => i32::from(h.abort.is_finished()),
        Err(_) => 1,
    }
}

/// Free a stream handle. Must be called after `xmtp_stream_end`.
/// Calling this on an active (non-ended) stream will also end it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_stream_free(handle: *mut FfiStreamHandle) {
    if !handle.is_null() {
        let h = unsafe { Box::from_raw(handle) };
        h.abort.end();
    }
}
