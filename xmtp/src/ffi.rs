#![allow(unsafe_code)]
//! Internal FFI utilities: RAII handle wrapper + C string helpers.

use std::ffi::{CStr, CString, c_char};
use std::ptr;
use std::ptr::NonNull;

use crate::error::{Error, Result};

/// RAII wrapper for an opaque FFI pointer. Calls `free` on drop.
pub(crate) struct OwnedHandle<T> {
    ptr: NonNull<T>,
    free: unsafe extern "C" fn(*mut T),
}

unsafe impl<T> Send for OwnedHandle<T> {}

impl<T> OwnedHandle<T> {
    /// Wrap a raw FFI pointer. Returns [`Error::NullPointer`] if null.
    pub(crate) fn new(ptr: *mut T, free: unsafe extern "C" fn(*mut T)) -> Result<Self> {
        NonNull::new(ptr)
            .map(|ptr| Self { ptr, free })
            .ok_or(Error::NullPointer)
    }

    /// Const pointer for FFI read calls.
    #[inline]
    pub(crate) fn as_ptr(&self) -> *const T {
        self.ptr.as_ptr().cast_const()
    }
}

impl<T> Drop for OwnedHandle<T> {
    fn drop(&mut self) {
        unsafe { (self.free)(self.ptr.as_ptr()) };
    }
}

impl<T> std::fmt::Debug for OwnedHandle<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnedHandle")
            .field("ptr", &self.ptr)
            .finish()
    }
}

/// Take ownership of a C string, convert to `String`, then free via `xmtp_free_string`.
pub(crate) unsafe fn take_c_string(ptr: *mut c_char) -> Result<String> {
    if ptr.is_null() {
        return Err(Error::NullPointer);
    }
    let cstr = unsafe { CStr::from_ptr(ptr) };
    let s = cstr
        .to_str()
        .map(String::from)
        .map_err(|_| Error::InvalidUtf8);
    unsafe { xmtp_sys::xmtp_free_string(ptr) };
    s
}

/// Convert `&str` to `CString` for FFI.
pub(crate) fn to_c_string(s: &str) -> Result<CString> {
    CString::new(s).map_err(|_| Error::InvalidArgument("string contains NUL".into()))
}

/// Read a **borrowed** C string array into `Vec<String>`. Does NOT free anything.
pub(crate) unsafe fn read_borrowed_strings(ptr: *const *mut c_char, count: i32) -> Vec<String> {
    if ptr.is_null() || count <= 0 {
        return vec![];
    }
    (0..count as usize)
        .filter_map(|i| {
            let s = unsafe { *ptr.add(i) };
            if s.is_null() {
                return None;
            }
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
        unsafe { take_c_string(ptr) }.ok()
    }
}

/// Get pointer from an optional `CString` (null if `None`).
pub(crate) fn c_str_ptr(opt: &Option<CString>) -> *const c_char {
    opt.as_ref().map_or(ptr::null(), |c| c.as_ptr())
}

/// Convert optional `&str` to optional `CString`.
pub(crate) fn optional_c_string(s: Option<&str>) -> Result<Option<CString>> {
    s.map(to_c_string).transpose()
}
