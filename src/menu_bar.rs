use std::{ffi::CString, os::raw::c_char, ptr};

unsafe extern "C" {
    fn kronos_update_menu_bar(clock: *const c_char);
}

pub fn update(elapsed_seconds: Option<u64>) {
    let clock = elapsed_seconds.map(|seconds| {
        CString::new(crate::ui::format_duration(seconds * 1000))
            .expect("formatted duration contains no NUL bytes")
    });
    // Called from the main thread. AppKit copies the title during this call.
    unsafe {
        kronos_update_menu_bar(clock.as_ref().map_or(ptr::null(), |clock| clock.as_ptr()));
    }
}
