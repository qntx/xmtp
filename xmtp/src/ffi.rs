#![allow(
    unsafe_code,
    reason = "FFI utilities require unsafe for raw pointer and C string operations"
)]
//! Internal FFI utilities: RAII handle wrapper + C string helpers.

use std::ffi::{CStr, CString, c_char};
use std::ptr::NonNull;

use crate::content::Reaction;
use crate::error::{Result, XmtpError};

/// RAII wrapper for an opaque FFI pointer. Calls `free` on drop.
pub(crate) struct OwnedHandle<T> {
    ptr: NonNull<T>,
    free: unsafe extern "C" fn(*mut T),
}

// SAFETY: FFI handles are opaque pointers to thread-safe C objects managed by libxmtp.
// The C library guarantees thread-safe access to these handles.
unsafe impl<T> Send for OwnedHandle<T> {}

impl<T> OwnedHandle<T> {
    /// Wrap a raw FFI pointer. Returns [`XmtpError::NullPointer`] if null.
    pub(crate) fn new(ptr: *mut T, free: unsafe extern "C" fn(*mut T)) -> Result<Self> {
        NonNull::new(ptr)
            .map(|ptr| Self { ptr, free })
            .ok_or(XmtpError::NullPointer)
    }

    /// Const pointer for FFI read calls.
    #[inline]
    pub(crate) const fn as_ptr(&self) -> *const T {
        self.ptr.as_ptr().cast_const()
    }
}

impl<T> Drop for OwnedHandle<T> {
    fn drop(&mut self) {
        // SAFETY: `self.ptr` is a valid, non-null pointer obtained from the FFI layer,
        // and `self.free` is the matching deallocation function provided at construction.
        unsafe { (self.free)(self.ptr.as_ptr()) };
    }
}

impl<T> std::fmt::Debug for OwnedHandle<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnedHandle")
            .field("ptr", &self.ptr)
            .finish_non_exhaustive()
    }
}

/// Take ownership of a C string, convert to `String`, then free via `xmtp_free_string`.
pub(crate) unsafe fn take_c_string(ptr: *mut c_char) -> Result<String> {
    if ptr.is_null() {
        return Err(XmtpError::NullPointer);
    }
    // SAFETY: `ptr` is non-null and points to a valid NUL-terminated C string from the FFI layer.
    let cstr = unsafe { CStr::from_ptr(ptr) };
    let s = cstr
        .to_str()
        .map(String::from)
        .map_err(|_| XmtpError::InvalidUtf8);
    // SAFETY: `ptr` was allocated by the C library and must be freed with `xmtp_free_string`.
    unsafe { xmtp_sys::xmtp_free_string(ptr) };
    s
}

/// Convert `&str` to `CString` for FFI.
pub(crate) fn to_c_string(s: &str) -> Result<CString> {
    CString::new(s).map_err(|_| XmtpError::InvalidArgument("string contains NUL".into()))
}

/// Read a **borrowed** C string array into `Vec<String>`. Does NOT free anything.
pub(crate) unsafe fn read_borrowed_strings(ptr: *const *mut c_char, count: i32) -> Vec<String> {
    if ptr.is_null() || count <= 0 {
        return vec![];
    }
    (0..count.unsigned_abs() as usize)
        .filter_map(|i| {
            // SAFETY: `ptr` points to an array of at least `count` pointers, so `ptr.add(i)` is in-bounds.
            let elem = unsafe { ptr.add(i) };
            // SAFETY: `elem` is a valid aligned pointer within the array.
            let s = unsafe { *elem };
            if s.is_null() {
                return None;
            }
            // SAFETY: `s` is a non-null pointer to a valid NUL-terminated C string owned by the FFI layer.
            unsafe { CStr::from_ptr(s) }.to_str().ok().map(String::from)
        })
        .collect()
}

/// Convert a slice of string refs to C string arrays for FFI.
pub(crate) fn to_c_string_array(strings: &[&str]) -> Result<(Vec<CString>, Vec<*const c_char>)> {
    let owned: Vec<CString> = strings
        .iter()
        .map(|s| to_c_string(s))
        .collect::<Result<_>>()?;
    let ptrs = owned.iter().map(|c| c.as_ptr()).collect();
    Ok((owned, ptrs))
}

/// Convert `AccountIdentifier`s to parallel C arrays (addresses + kinds).
pub(crate) fn identifiers_to_ffi(
    ids: &[crate::types::AccountIdentifier],
) -> Result<(Vec<CString>, Vec<*const c_char>, Vec<i32>)> {
    let owned: Vec<CString> = ids
        .iter()
        .map(|id| to_c_string(&id.address))
        .collect::<Result<_>>()?;
    let ptrs = owned.iter().map(|c| c.as_ptr()).collect();
    let kinds = ids.iter().map(|id| id.kind as i32).collect();
    Ok((owned, ptrs, kinds))
}

/// Take ownership of a nullable C string. Returns `None` if null. Frees the string.
pub(crate) unsafe fn take_nullable_string(ptr: *mut c_char) -> Option<String> {
    if ptr.is_null() {
        None
    } else {
        // SAFETY: `ptr` is non-null; caller guarantees it points to a valid owned C string.
        unsafe { take_c_string(ptr) }.ok()
    }
}

/// Get pointer from an optional `CString` (null if `None`).
pub(crate) fn c_str_ptr(opt: Option<&CString>) -> *const c_char {
    opt.map_or(std::ptr::null(), |c| c.as_ptr())
}

/// Convert optional `&str` to optional `CString`.
pub(crate) fn optional_c_string(s: Option<&str>) -> Result<Option<CString>> {
    s.map(to_c_string).transpose()
}

/// RAII guard for an opaque FFI list pointer. Calls `free` on drop.
///
/// Guarantees the list is freed on all exit paths (including panics),
/// and normalises a null pointer or negative length to an empty iteration.
pub(crate) struct FfiList<T> {
    ptr: *mut T,
    len: i32,
    free: unsafe extern "C" fn(*mut T),
}

impl<T> FfiList<T> {
    /// Wrap a nullable FFI list pointer.
    ///
    /// `len_fn` takes `*const T` (read-only), `free` takes `*mut T` (ownership).
    pub(crate) fn new(
        ptr: *mut T,
        len_fn: unsafe extern "C" fn(*const T) -> i32,
        free: unsafe extern "C" fn(*mut T),
    ) -> Self {
        if ptr.is_null() {
            return Self { ptr, len: 0, free };
        }
        // SAFETY: `ptr` is non-null and points to a valid FFI list; `len_fn` is its length accessor.
        let len = unsafe { len_fn(ptr.cast_const()) }.max(0);
        Self { ptr, len, free }
    }

    #[inline]
    pub(crate) const fn len(&self) -> i32 {
        self.len
    }

    #[inline]
    pub(crate) const fn as_ptr(&self) -> *mut T {
        self.ptr
    }
}

impl<T> Drop for FfiList<T> {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            // SAFETY: `self.ptr` is non-null and was obtained from the FFI layer;
            // `self.free` is the matching deallocation function.
            unsafe { (self.free)(self.ptr) };
        }
    }
}

/// Convert an FFI `i32` length to `usize`, clamping negatives to zero.
#[inline]
pub(crate) fn ffi_usize(raw: i32) -> usize {
    raw.max(0) as usize
}

/// Convert a Rust `usize` length to FFI `i32`, returning an error on overflow.
pub(crate) fn to_ffi_len(len: usize) -> Result<i32> {
    i32::try_from(len).map_err(|_| XmtpError::InvalidArgument("length exceeds i32::MAX".into()))
}

/// Read a **borrowed** (non-owned) C string pointer. Returns empty string if null.
///
/// Unlike [`take_c_string`], this does **not** free the pointer.
#[allow(dead_code, reason = "used by conversation.rs which re-imports it")]
pub(crate) unsafe fn borrow_c_string(ptr: *mut c_char) -> String {
    if ptr.is_null() {
        String::new()
    } else {
        // SAFETY: `ptr` is non-null and points to a valid NUL-terminated C string.
        unsafe { CStr::from_ptr(ptr) }
            .to_str()
            .unwrap_or_default()
            .to_owned()
    }
}

/// Read a **borrowed** nullable C string pointer. Returns `None` if null.
///
/// Unlike [`take_nullable_string`], this does **not** free the pointer.
#[allow(dead_code, reason = "used by conversation.rs which re-imports it")]
pub(crate) unsafe fn borrow_nullable_string(ptr: *mut c_char) -> Option<String> {
    if ptr.is_null() {
        None
    } else {
        Some(
            // SAFETY: `ptr` is non-null and points to a valid NUL-terminated C string.
            unsafe { CStr::from_ptr(ptr) }
                .to_str()
                .unwrap_or_default()
                .to_owned(),
        )
    }
}

pub(crate) unsafe fn reactions_from_raw_parts(
    ptr: *mut xmtp_sys::XmtpFfiReaction,
    len: i32,
) -> Vec<Reaction> {
    if ptr.is_null() || len <= 0 {
        return Vec::new();
    }
    // SAFETY: `ptr` is non-null and `len` is positive
    let slice = unsafe { std::slice::from_raw_parts(ptr, ffi_usize(len)) };
    slice.iter().map(Reaction::from_ffi).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffi_usize_clamps_negative_to_zero() {
        assert_eq!(ffi_usize(5), 5);
        assert_eq!(ffi_usize(0), 0);
        assert_eq!(ffi_usize(-1), 0);
        assert_eq!(ffi_usize(i32::MIN), 0);
    }

    #[test]
    fn to_ffi_len_boundary() {
        assert_eq!(to_ffi_len(0).unwrap(), 0);
        assert_eq!(to_ffi_len(i32::MAX as usize).unwrap(), i32::MAX);
        assert!(to_ffi_len(i32::MAX as usize + 1).is_err());
    }

    #[test]
    fn to_c_string_rejects_interior_nul() {
        assert!(to_c_string("hello").is_ok());
        assert!(to_c_string("hello\0world").is_err());
    }

    #[test]
    fn to_c_string_array_preserves_order_and_content() {
        let input = &["foo", "bar", "baz"];
        let (owned, ptrs) = to_c_string_array(input).unwrap();
        assert_eq!(ptrs.len(), input.len());
        for (cs, &expected) in owned.iter().zip(input) {
            assert_eq!(cs.to_str().unwrap(), expected);
        }
    }

    #[test]
    fn borrow_c_string_null_returns_empty() {
        // SAFETY: Testing null pointer handling.
        assert!(unsafe { borrow_c_string(std::ptr::null_mut()) }.is_empty());
    }

    #[test]
    fn borrow_c_string_reads_without_freeing() {
        let cs = CString::new("test").unwrap();
        // SAFETY: `ptr` points to a valid NUL-terminated C string owned by `cs`.
        let s = unsafe { borrow_c_string(cs.as_ptr().cast_mut()) };
        assert_eq!(s, "test");
        assert_eq!(cs.to_str().unwrap(), "test");
    }

    #[test]
    fn borrow_nullable_string_null_returns_none() {
        // SAFETY: Testing null pointer handling.
        assert!(unsafe { borrow_nullable_string(std::ptr::null_mut()) }.is_none());
    }

    #[test]
    fn borrow_nullable_string_returns_some() {
        let cs = CString::new("hello").unwrap();
        // SAFETY: `ptr` points to a valid NUL-terminated C string owned by `cs`.
        let s = unsafe { borrow_nullable_string(cs.as_ptr().cast_mut()) };
        assert_eq!(s.as_deref(), Some("hello"));
    }

    #[test]
    fn identifiers_to_ffi_parallel_arrays() {
        use crate::types::{AccountIdentifier, IdentifierKind};
        let ids = vec![
            AccountIdentifier {
                address: "0xaaa".into(),
                kind: IdentifierKind::Ethereum,
            },
            AccountIdentifier {
                address: "pk".into(),
                kind: IdentifierKind::Passkey,
            },
        ];
        let (_owned, ptrs, kinds) = identifiers_to_ffi(&ids).unwrap();
        assert_eq!(ptrs.len(), 2);
        assert_eq!(kinds, vec![0, 1]);
    }
}
