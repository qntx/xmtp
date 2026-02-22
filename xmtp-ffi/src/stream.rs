//! Callback-based streaming for conversations and messages.

use std::sync::Arc;

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
        _ => None, // -1 or any other value = all
    }
}

/// Parse a consent state filter from a raw int array.
fn parse_consent_states(
    states: *const i32,
    count: i32,
) -> Option<Vec<xmtp_db::consent_record::ConsentState>> {
    if states.is_null() || count <= 0 {
        return None;
    }
    let mut result = Vec::with_capacity(count as usize);
    for i in 0..count as usize {
        let s = unsafe { *states.add(i) };
        result.push(match s {
            0 => xmtp_db::consent_record::ConsentState::Unknown,
            1 => xmtp_db::consent_record::ConsentState::Allowed,
            2 => xmtp_db::consent_record::ConsentState::Denied,
            _ => continue,
        });
    }
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

// ---------------------------------------------------------------------------
// Stream conversations
// ---------------------------------------------------------------------------

/// Stream new conversations. Calls `callback` for each new conversation.
/// The callback receives a `*mut FfiConversation` that the caller must free.
/// `context` is an opaque pointer passed through to both callbacks.
/// `on_close` is called when the stream ends (pass null to ignore).
///
/// Returns a stream handle via `out` that must be closed with [`xmtp_stream_close`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_stream_conversations(
    client: *const FfiClient,
    conversation_type: i32,
    callback: FnConversationCallback,
    on_close: Option<FnOnCloseCallback>,
    context: *mut std::ffi::c_void,
    out: *mut *mut FfiStreamHandle,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }

        let conv_type = parse_conv_type(conversation_type);
        let ctx = context as usize;
        let close_ctx = ctx;

        let mut handle = MlsClient::stream_conversations_with_callback(
            c.inner.clone(),
            conv_type,
            move |result| {
                if let Ok(group) = result {
                    let ptr = into_raw(FfiConversation { inner: group });
                    unsafe { callback(ptr, ctx as *mut std::ffi::c_void) };
                }
            },
            move || {
                if let Some(cb) = on_close {
                    unsafe { cb(close_ctx as *mut std::ffi::c_void) };
                }
            },
            false,
        );

        runtime().block_on(handle.wait_for_ready());
        let abort = handle.abort_handle();
        unsafe {
            write_out(
                out,
                FfiStreamHandle {
                    abort: Arc::new(abort),
                },
            )?
        };
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Stream all messages
// ---------------------------------------------------------------------------

/// Stream all messages across conversations.
/// The callback receives a `*mut FfiMessage` that the caller must free.
/// `consent_states` / `consent_states_count`: optional consent filter (pass null/0 for all).
/// `on_close` is called when the stream ends (pass null to ignore).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_stream_all_messages(
    client: *const FfiClient,
    conversation_type: i32,
    consent_states: *const i32,
    consent_states_count: i32,
    callback: FnMessageCallback,
    on_close: Option<FnOnCloseCallback>,
    context: *mut std::ffi::c_void,
    out: *mut *mut FfiStreamHandle,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }

        let conv_type = parse_conv_type(conversation_type);
        let consents = parse_consent_states(consent_states, consent_states_count);
        let ctx = context as usize;
        let close_ctx = ctx;

        let mut handle = MlsClient::stream_all_messages_with_callback(
            c.inner.context.clone(),
            conv_type,
            consents,
            move |result| {
                if let Ok(msg) = result {
                    let ptr = into_raw(FfiMessage { inner: msg });
                    unsafe { callback(ptr, ctx as *mut std::ffi::c_void) };
                }
            },
            move || {
                if let Some(cb) = on_close {
                    unsafe { cb(close_ctx as *mut std::ffi::c_void) };
                }
            },
        );

        runtime().block_on(handle.wait_for_ready());
        let abort = handle.abort_handle();
        unsafe {
            write_out(
                out,
                FfiStreamHandle {
                    abort: Arc::new(abort),
                },
            )?
        };
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Stream single conversation messages
// ---------------------------------------------------------------------------

/// Stream messages for a single conversation.
/// The callback receives a `*mut FfiMessage` that the caller must free.
/// `on_close` is called when the stream ends (pass null to ignore).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_stream_messages(
    conv: *const FfiConversation,
    callback: FnMessageCallback,
    on_close: Option<FnOnCloseCallback>,
    context: *mut std::ffi::c_void,
    out: *mut *mut FfiStreamHandle,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(conv)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }

        let ctx = context as usize;
        let close_ctx = ctx;

        let mut handle = MlsGroup::stream_with_callback(
            c.inner.context.clone(),
            c.inner.group_id.clone(),
            move |result| {
                if let Ok(msg) = result {
                    let ptr = into_raw(FfiMessage { inner: msg });
                    unsafe { callback(ptr, ctx as *mut std::ffi::c_void) };
                }
            },
            move || {
                if let Some(cb) = on_close {
                    unsafe { cb(close_ctx as *mut std::ffi::c_void) };
                }
            },
        );

        runtime().block_on(handle.wait_for_ready());
        let abort = handle.abort_handle();
        unsafe {
            write_out(
                out,
                FfiStreamHandle {
                    abort: Arc::new(abort),
                },
            )?
        };
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Stream consent updates
// ---------------------------------------------------------------------------

/// Stream consent state changes. Callback receives an array of consent records.
/// Caller must free each `entity` string in the records after processing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_stream_consent(
    client: *const FfiClient,
    callback: FnConsentCallback,
    on_close: Option<FnOnCloseCallback>,
    context: *mut std::ffi::c_void,
    out: *mut *mut FfiStreamHandle,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }

        let ctx = context as usize;
        let close_ctx = ctx;

        let mut handle = MlsClient::stream_consent_with_callback(
            c.inner.clone(),
            move |result| {
                if let Ok(records) = result {
                    let c_records: Vec<FfiConsentRecord> =
                        records.iter().map(consent_record_to_c).collect();
                    let count = c_records.len() as i32;
                    let (ptr, _, _) = c_records.into_raw_parts();
                    unsafe { callback(ptr, count, ctx as *mut std::ffi::c_void) };
                }
            },
            move || {
                if let Some(cb) = on_close {
                    unsafe { cb(close_ctx as *mut std::ffi::c_void) };
                }
            },
        );

        runtime().block_on(handle.wait_for_ready());
        let abort = handle.abort_handle();
        unsafe {
            write_out(
                out,
                FfiStreamHandle {
                    abort: Arc::new(abort),
                },
            )?
        };
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Stream preference updates
// ---------------------------------------------------------------------------

/// Stream preference updates (consent changes + HMAC key rotations).
/// Callback receives an array of preference updates.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_stream_preferences(
    client: *const FfiClient,
    callback: FnPreferenceCallback,
    on_close: Option<FnOnCloseCallback>,
    context: *mut std::ffi::c_void,
    out: *mut *mut FfiStreamHandle,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }

        let ctx = context as usize;
        let close_ctx = ctx;

        let mut handle = MlsClient::stream_preferences_with_callback(
            c.inner.clone(),
            move |result| {
                if let Ok(updates) = result {
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
                    let count = c_updates.len() as i32;
                    let (ptr, _, _) = c_updates.into_raw_parts();
                    unsafe { callback(ptr, count, ctx as *mut std::ffi::c_void) };
                }
            },
            move || {
                if let Some(cb) = on_close {
                    unsafe { cb(close_ctx as *mut std::ffi::c_void) };
                }
            },
        );

        runtime().block_on(handle.wait_for_ready());
        let abort = handle.abort_handle();
        unsafe {
            write_out(
                out,
                FfiStreamHandle {
                    abort: Arc::new(abort),
                },
            )?
        };
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Stream message deletions
// ---------------------------------------------------------------------------

/// Stream message deletion events across all conversations.
/// The callback receives the deleted message ID as a hex string (caller must free).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_stream_message_deletions(
    client: *const FfiClient,
    callback: FnMessageDeletionCallback,
    context: *mut std::ffi::c_void,
    out: *mut *mut FfiStreamHandle,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }

        let ctx = context as usize;

        let handle =
            MlsClient::stream_message_deletions_with_callback(c.inner.clone(), move |result| {
                if let Ok(decoded) = result {
                    let msg_id_hex = to_c_string(&hex::encode(&decoded.metadata.id));
                    unsafe { callback(msg_id_hex, ctx as *mut std::ffi::c_void) };
                }
            });

        let abort = handle.abort_handle();
        unsafe {
            write_out(
                out,
                FfiStreamHandle {
                    abort: Arc::new(abort),
                },
            )?
        };
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Stream lifecycle
// ---------------------------------------------------------------------------

/// Close a stream and stop receiving events.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_stream_close(handle: *mut FfiStreamHandle) {
    if !handle.is_null() {
        let h = unsafe { Box::from_raw(handle) };
        h.abort.end();
    }
}

/// Check if a stream is closed. Returns 1 if closed, 0 if active.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_stream_is_closed(handle: *const FfiStreamHandle) -> i32 {
    match unsafe { ref_from(handle) } {
        Ok(h) => i32::from(h.abort.is_finished()),
        Err(_) => 1,
    }
}
