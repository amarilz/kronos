use std::{ffi::CString, os::raw::c_char, ptr};

unsafe extern "C" {
    fn kronos_update_menu_bar(clock: *const c_char, filled: bool);
}

pub fn update(elapsed_seconds: Option<u64>) {
    let clock = elapsed_seconds.map(|seconds| {
        CString::new(crate::ui::format_duration(seconds * 1000))
            .expect("formatted duration contains no NUL bytes")
    });
    let filled = elapsed_seconds.is_some_and(|seconds| seconds % 2 == 0);
    // Called from the main thread. AppKit copies the title during this call.
    unsafe {
        kronos_update_menu_bar(
            clock.as_ref().map_or(ptr::null(), |clock| clock.as_ptr()),
            filled,
        );
    }
}
