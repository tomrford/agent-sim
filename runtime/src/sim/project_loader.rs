use std::ffi::CStr;
use std::os::raw::c_char;

pub const MAX_FFI_BUFFER_CAPACITY: u32 = 65_536;

pub fn next_capacity(capacity: u32, context: &str) -> Result<u32, String> {
    if capacity >= MAX_FFI_BUFFER_CAPACITY {
        return Err(format!(
            "{context} exceeded max FFI buffer capacity {MAX_FFI_BUFFER_CAPACITY}"
        ));
    }
    Ok(capacity.saturating_mul(2).clamp(2, MAX_FFI_BUFFER_CAPACITY))
}

pub fn validate_written(written: u32, capacity: u32, context: &str) -> Result<usize, String> {
    if written > capacity {
        return Err(format!(
            "{context} reported {written} entries for capacity {capacity}"
        ));
    }
    Ok(written as usize)
}

pub fn decode_owned_cstr(ptr: *const c_char, context: &str) -> Result<String, String> {
    if ptr.is_null() {
        return Err(format!("missing {context} string in FFI metadata"));
    }
    Ok(unsafe { CStr::from_ptr(ptr) }.to_string_lossy().to_string())
}
