//! Device sync: archive creation, import, sync requests, and available archives.

use std::ffi::c_char;

use xmtp_mls::groups::device_sync::{
    ArchiveOptions as NativeArchiveOptions, BackupElementSelection,
    archive::{ArchiveImporter, ENC_KEY_SIZE, exporter::ArchiveExporter, insert_importer},
};

use crate::ffi::*;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse C `FfiArchiveOptions` into the native `ArchiveOptions`.
fn parse_archive_opts(opts: *const FfiArchiveOptions) -> NativeArchiveOptions {
    if opts.is_null() {
        return NativeArchiveOptions::default();
    }
    let o = unsafe { &*opts };
    let mut elements = Vec::new();
    if o.elements & 1 != 0 {
        elements.push(BackupElementSelection::Messages);
    }
    if o.elements & 2 != 0 {
        elements.push(BackupElementSelection::Consent);
    }
    NativeArchiveOptions {
        elements,
        start_ns: if o.start_ns > 0 {
            Some(o.start_ns)
        } else {
            None
        },
        end_ns: if o.end_ns > 0 { Some(o.end_ns) } else { None },
        exclude_disappearing_messages: o.exclude_disappearing_messages != 0,
    }
}

/// Validate and truncate an encryption key to `ENC_KEY_SIZE`.
fn check_key(key: *const u8, key_len: i32) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if key.is_null() || (key_len as usize) < ENC_KEY_SIZE {
        return Err(format!("encryption key must be at least {} bytes", ENC_KEY_SIZE).into());
    }
    let mut v = unsafe { std::slice::from_raw_parts(key, key_len as usize) }.to_vec();
    v.truncate(ENC_KEY_SIZE);
    Ok(v)
}

// ---------------------------------------------------------------------------
// Send sync request
// ---------------------------------------------------------------------------

/// Send a device sync request to retrieve records from another installation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_device_sync_send_request(
    client: *const FfiClient,
    opts: *const FfiArchiveOptions,
    server_url: *const c_char,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        let url = unsafe { c_str_to_string(server_url)? };
        let archive_opts = parse_archive_opts(opts);
        c.inner
            .device_sync_client()
            .send_sync_request(archive_opts, url)
            .await?;
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Send sync archive
// ---------------------------------------------------------------------------

/// Send a sync archive to the sync group with the given pin.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_device_sync_send_archive(
    client: *const FfiClient,
    opts: *const FfiArchiveOptions,
    server_url: *const c_char,
    pin: *const c_char,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        let url = unsafe { c_str_to_string(server_url)? };
        let pin_str = unsafe { c_str_to_string(pin)? };
        let archive_opts = parse_archive_opts(opts);
        c.inner
            .device_sync_client()
            .send_sync_archive(&archive_opts, &url, &pin_str)
            .await?;
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Process sync archive
// ---------------------------------------------------------------------------

/// Process a sync archive matching the given pin.
/// Pass null for `pin` to process the latest archive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_device_sync_process_archive(
    client: *const FfiClient,
    pin: *const c_char,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        let pin_str = unsafe { c_str_to_option(pin)? };
        c.inner
            .device_sync_client()
            .process_archive_with_pin(pin_str.as_deref())
            .await?;
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// List available archives
// ---------------------------------------------------------------------------

/// List archives available for import in the sync group.
/// `days_cutoff` limits how far back to look.
/// Caller must free with [`xmtp_available_archive_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_device_sync_list_available_archives(
    client: *const FfiClient,
    days_cutoff: i64,
    out: *mut *mut FfiAvailableArchiveList,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let archives = c
            .inner
            .device_sync_client()
            .list_available_archives(days_cutoff)?;
        let items: Vec<FfiAvailableArchive> = archives
            .into_iter()
            .map(|a| {
                let inst = a.sent_by_installation;
                let inst_len = inst.len() as i32;
                let (inst_ptr, _, _) = inst.into_raw_parts();
                FfiAvailableArchive {
                    pin: to_c_string(&a.pin),
                    backup_version: a.metadata.backup_version,
                    exported_at_ns: a.metadata.exported_at_ns,
                    sent_by_installation: inst_ptr,
                    sent_by_installation_len: inst_len,
                }
            })
            .collect();
        unsafe { write_out(out, FfiAvailableArchiveList { items })? };
        Ok(())
    })
}

ffi_list_len!(xmtp_available_archive_list_len, FfiAvailableArchiveList);

/// Get the pin string at index. Returns a borrowed pointer; do NOT free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_available_archive_pin(
    list: *const FfiAvailableArchiveList,
    index: i32,
) -> *const c_char {
    let l = match unsafe { ref_from(list) } {
        Ok(l) => l,
        Err(_) => return std::ptr::null(),
    };
    match l.items.get(index as usize) {
        Some(a) => a.pin as *const c_char,
        None => std::ptr::null(),
    }
}

/// Get the exported_at_ns at index.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_available_archive_exported_at_ns(
    list: *const FfiAvailableArchiveList,
    index: i32,
) -> i64 {
    let l = match unsafe { ref_from(list) } {
        Ok(l) => l,
        Err(_) => return 0,
    };
    l.items.get(index as usize).map_or(0, |a| a.exported_at_ns)
}

/// Free an available archive list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_available_archive_list_free(list: *mut FfiAvailableArchiveList) {
    if list.is_null() {
        return;
    }
    let l = unsafe { Box::from_raw(list) };
    for item in &l.items {
        free_c_strings!(item, pin);
        if !item.sent_by_installation.is_null() && item.sent_by_installation_len > 0 {
            drop(unsafe {
                Vec::from_raw_parts(
                    item.sent_by_installation,
                    item.sent_by_installation_len as usize,
                    item.sent_by_installation_len as usize,
                )
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Create archive (export to file)
// ---------------------------------------------------------------------------

/// Export an archive to a local file.
/// `key` must be at least 32 bytes (encryption key).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_device_sync_create_archive(
    client: *const FfiClient,
    path: *const c_char,
    opts: *const FfiArchiveOptions,
    key: *const u8,
    key_len: i32,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        let path_str = unsafe { c_str_to_string(path)? };
        let archive_opts = parse_archive_opts(opts);
        let enc_key = check_key(key, key_len)?;
        let db = c.inner.context.store().db();
        ArchiveExporter::export_to_file(archive_opts, db, path_str, &enc_key).await?;
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Import archive (from file)
// ---------------------------------------------------------------------------

/// Import a previously exported archive from a file.
/// `key` must be at least 32 bytes (encryption key).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_device_sync_import_archive(
    client: *const FfiClient,
    path: *const c_char,
    key: *const u8,
    key_len: i32,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        let path_str = unsafe { c_str_to_string(path)? };
        let enc_key = check_key(key, key_len)?;
        let mut importer = ArchiveImporter::from_file(path_str, &enc_key).await?;
        insert_importer(&mut importer, &c.inner.context).await?;
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Archive metadata
// ---------------------------------------------------------------------------

/// Read metadata from an archive file without loading its full contents.
/// `out_elements` is a bitmask: bit 0 = Messages, bit 1 = Consent.
/// `out_start_ns` / `out_end_ns` are 0 if not set. All output pointers are nullable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_device_sync_archive_metadata(
    path: *const c_char,
    key: *const u8,
    key_len: i32,
    out_version: *mut u16,
    out_exported_at_ns: *mut i64,
    out_elements: *mut i32,
    out_start_ns: *mut i64,
    out_end_ns: *mut i64,
) -> i32 {
    catch_async(|| async {
        let path_str = unsafe { c_str_to_string(path)? };
        let enc_key = check_key(key, key_len)?;
        let importer = ArchiveImporter::from_file(path_str, &enc_key).await?;
        let m = &importer.metadata;
        if !out_version.is_null() {
            unsafe { *out_version = m.backup_version };
        }
        if !out_exported_at_ns.is_null() {
            unsafe { *out_exported_at_ns = m.exported_at_ns };
        }
        if !out_elements.is_null() {
            let mut bits: i32 = 0;
            for e in &m.elements {
                match e {
                    BackupElementSelection::Messages => bits |= 1,
                    BackupElementSelection::Consent => bits |= 2,
                    BackupElementSelection::Event => bits |= 4,
                    _ => {}
                }
            }
            unsafe { *out_elements = bits };
        }
        if !out_start_ns.is_null() {
            unsafe { *out_start_ns = m.start_ns.unwrap_or(0) };
        }
        if !out_end_ns.is_null() {
            unsafe { *out_end_ns = m.end_ns.unwrap_or(0) };
        }
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Sync all device sync groups
// ---------------------------------------------------------------------------

/// Manually sync all device sync groups.
/// Writes the number of synced/eligible groups to the output pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_device_sync_sync_all(
    client: *const FfiClient,
    out_synced: *mut i32,
    out_eligible: *mut i32,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        c.inner.sync_welcomes().await?;
        let summary = c.inner.sync_all_device_sync_groups().await?;
        if !out_synced.is_null() {
            unsafe { *out_synced = summary.num_synced as i32 };
        }
        if !out_eligible.is_null() {
            unsafe { *out_eligible = summary.num_eligible as i32 };
        }
        Ok(())
    })
}
