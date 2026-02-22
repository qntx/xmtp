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

// ---------------------------------------------------------------------------
// Stream conversations
// ---------------------------------------------------------------------------

/// Stream new conversations. Calls `callback` for each new conversation.
/// The callback receives a `*mut XmtpConversation` that the caller must free.
/// `context` is an opaque pointer passed through to the callback.
///
/// Returns a stream handle via `out` that must be closed with [`xmtp_stream_close`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_stream_conversations(
    client: *const XmtpClient,
    conversation_type: i32,
    callback: FnConversationCallback,
    context: *mut std::ffi::c_void,
    out: *mut *mut XmtpStreamHandle,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }

        let conv_type = parse_conv_type(conversation_type);
        let ctx = context as usize; // usize is Send; raw pointers are not

        let mut handle = MlsClient::stream_conversations_with_callback(
            c.inner.clone(),
            conv_type,
            move |result| {
                if let Ok(group) = result {
                    let ptr = into_raw(XmtpConversation { inner: group });
                    unsafe {
                        callback(ptr, ctx as *mut std::ffi::c_void);
                    }
                }
            },
            || {},
            false,
        );

        runtime().block_on(handle.wait_for_ready());
        let abort = handle.abort_handle();
        unsafe {
            write_out(
                out,
                XmtpStreamHandle {
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
/// The callback receives a `*mut XmtpMessage` that the caller must free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_stream_all_messages(
    client: *const XmtpClient,
    conversation_type: i32,
    callback: FnMessageCallback,
    context: *mut std::ffi::c_void,
    out: *mut *mut XmtpStreamHandle,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }

        let conv_type = parse_conv_type(conversation_type);
        let ctx = context as usize;

        let mut handle = MlsClient::stream_all_messages_with_callback(
            c.inner.context.clone(),
            conv_type,
            None,
            move |result| {
                if let Ok(msg) = result {
                    let ptr = into_raw(XmtpMessage { inner: msg });
                    unsafe {
                        callback(ptr, ctx as *mut std::ffi::c_void);
                    }
                }
            },
            || {},
        );

        runtime().block_on(handle.wait_for_ready());
        let abort = handle.abort_handle();
        unsafe {
            write_out(
                out,
                XmtpStreamHandle {
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
/// The callback receives a `*mut XmtpMessage` that the caller must free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_stream_messages(
    conv: *const XmtpConversation,
    callback: FnMessageCallback,
    context: *mut std::ffi::c_void,
    out: *mut *mut XmtpStreamHandle,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(conv)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }

        let ctx = context as usize;

        let mut handle = MlsGroup::stream_with_callback(
            c.inner.context.clone(),
            c.inner.group_id.clone(),
            move |result| {
                if let Ok(msg) = result {
                    let ptr = into_raw(XmtpMessage { inner: msg });
                    unsafe {
                        callback(ptr, ctx as *mut std::ffi::c_void);
                    }
                }
            },
            || {},
        );

        runtime().block_on(handle.wait_for_ready());
        let abort = handle.abort_handle();
        unsafe {
            write_out(
                out,
                XmtpStreamHandle {
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
pub unsafe extern "C" fn xmtp_stream_close(handle: *mut XmtpStreamHandle) {
    if !handle.is_null() {
        let h = unsafe { Box::from_raw(handle) };
        h.abort.end();
    }
}

/// Check if a stream is closed. Returns 1 if closed, 0 if active.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_stream_is_closed(handle: *const XmtpStreamHandle) -> i32 {
    match unsafe { ref_from(handle) } {
        Ok(h) => i32::from(h.abort.is_finished()),
        Err(_) => 1,
    }
}
