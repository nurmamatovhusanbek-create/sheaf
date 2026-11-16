//! SEC-ISOLATE: what the worker takes away from itself before it opens any
//! document. The worker is the one process that parses hostile files, so a
//! memory-safety bug in PDFium or an image decoder lands here.
//!
//! Windows process mitigation policies, applied to the running process. Each
//! is best effort (an older Windows may not know one) and none can be undone
//! by an attacker once set:
//!
//! - no dynamic code: no new executable memory, no making data executable;
//! - no remote or low-integrity images: DLLs only from local, normal files;
//! - no legacy extension points (AppInit DLLs, IMEs, global hooks);
//! - strict handle checks: using a bad handle is a crash, not a silent success.
//!
//! Deliberately not applied: Win32k lockdown (PDFium finds system fonts
//! through GDI) and a child-process ban (OCR starts Tesseract).
//! The Job Object in `sheaf-jobs` limits memory and process count from outside.
//! Still open: a restricted token or low integrity level (PH-7).

/// Returns how many of the policies took effect.
#[cfg(windows)]
#[allow(unsafe_code)] // SetProcessMitigationPolicy calls; each states its invariant.
pub fn apply() -> usize {
    use std::ffi::c_void;
    use windows_sys::Win32::System::Threading::{
        ProcessDynamicCodePolicy, ProcessExtensionPointDisablePolicy, ProcessImageLoadPolicy,
        ProcessStrictHandleCheckPolicy, SetProcessMitigationPolicy, PROCESS_MITIGATION_POLICY,
    };
    // Every policy structure is a single DWORD of bit flags.
    let policies: [(PROCESS_MITIGATION_POLICY, u32); 4] = [
        (ProcessDynamicCodePolicy, 0b1),           // ProhibitDynamicCode
        (ProcessImageLoadPolicy, 0b11),            // NoRemoteImages | NoLowMandatoryLabelImages
        (ProcessExtensionPointDisablePolicy, 0b1), // DisableExtensionPoints
        (ProcessStrictHandleCheckPolicy, 0b11),    // RaiseExceptionOnInvalidHandleReference | permanent
    ];
    policies
        .iter()
        .filter(|(policy, flags)| {
            // SAFETY: the pointer is to a live u32 and the length is its size,
            // which is the layout every one of these policy structures has.
            unsafe { SetProcessMitigationPolicy(*policy, std::ptr::from_ref(flags).cast::<c_void>(), 4) != 0 }
        })
        .count()
}

#[cfg(not(windows))]
pub fn apply() -> usize {
    0
}
