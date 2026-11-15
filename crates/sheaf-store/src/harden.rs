//! Process hardening, applied first thing by every executable (SEC-ISOLATE).
//!
//! Explorer verbs, the wheel and "Open with" start our programs with the
//! working directory set to the folder of a document the user (or someone who
//! sent it) controls. Windows can be told never to look for a DLL there, or
//! on a network share, so a planted `dbghelp.dll` next to a PDF is not loaded
//! into a process that opens PDFs. Everything we need is loaded by absolute
//! path from the app's own folder or from System32.
//!
//! Win32 wrapper module (DOC-AGENT-RULES rule 5). A no-op elsewhere.

/// Restrict DLL loading and image loading for this process. Failures are
/// ignored: an older Windows without a policy simply does not get it.
pub fn harden_process() {
    #[cfg(windows)]
    win::apply();
}

/// Further restrictions for a worker, which only parses and renders documents and has no use for the
/// things they take away: legacy extension points (global hooks and the like), invalid-handle use
/// (a crash instead of silent misbehaviour) and generating code at run time (so a memory bug in a
/// parser cannot be turned into new executable code). Applied by `sheaf-worker` after
/// [`harden_process`]. Failures are ignored. Not applied: a lock-down of the window system (PDFium
/// draws text through GDI) and a ban on child processes (Tesseract is one).
pub fn harden_worker() {
    #[cfg(windows)]
    win::worker();
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod win {
    use windows::core::w;
    use windows::Win32::System::LibraryLoader::{
        SetDefaultDllDirectories, SetDllDirectoryW, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
    };
    use windows::Win32::System::SystemServices::{
        PROCESS_MITIGATION_DYNAMIC_CODE_POLICY, PROCESS_MITIGATION_EXTENSION_POINT_DISABLE_POLICY,
        PROCESS_MITIGATION_IMAGE_LOAD_POLICY, PROCESS_MITIGATION_STRICT_HANDLE_CHECK_POLICY,
    };
    use windows::Win32::System::Threading::{
        ProcessDynamicCodePolicy, ProcessExtensionPointDisablePolicy, ProcessImageLoadPolicy,
        ProcessStrictHandleCheckPolicy, SetProcessMitigationPolicy,
    };

    pub fn worker() {
        // SAFETY: as in `apply`: plain flag words, structures that live through the call.
        unsafe {
            let mut extension = PROCESS_MITIGATION_EXTENSION_POINT_DISABLE_POLICY::default();
            extension.Anonymous.Flags = 0b1; // DisableExtensionPoints
            let _ = SetProcessMitigationPolicy(
                ProcessExtensionPointDisablePolicy,
                (&raw const extension).cast(),
                std::mem::size_of::<PROCESS_MITIGATION_EXTENSION_POINT_DISABLE_POLICY>(),
            );
            let mut handles = PROCESS_MITIGATION_STRICT_HANDLE_CHECK_POLICY::default();
            handles.Anonymous.Flags = 0b11; // RaiseExceptionOnInvalidHandleReference | HandleExceptionsPermanentlyEnabled
            let _ = SetProcessMitigationPolicy(
                ProcessStrictHandleCheckPolicy,
                (&raw const handles).cast(),
                std::mem::size_of::<PROCESS_MITIGATION_STRICT_HANDLE_CHECK_POLICY>(),
            );
            let mut code = PROCESS_MITIGATION_DYNAMIC_CODE_POLICY::default();
            code.Anonymous.Flags = 0b1; // ProhibitDynamicCode
            let _ = SetProcessMitigationPolicy(
                ProcessDynamicCodePolicy,
                (&raw const code).cast(),
                std::mem::size_of::<PROCESS_MITIGATION_DYNAMIC_CODE_POLICY>(),
            );
        }
    }

    pub fn apply() {
        // SAFETY: documented process-wide settings taking plain values or pointers to
        // data that lives through the call; none of them keeps a pointer.
        unsafe {
            // Application folder, System32 and explicitly added folders only: never the
            // current directory, never PATH.
            let _ = SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS);
            // An empty string removes the current directory from the legacy search too.
            let _ = SetDllDirectoryW(w!(""));
            // No images from network shares or low-integrity locations; System32 first.
            let mut policy = PROCESS_MITIGATION_IMAGE_LOAD_POLICY::default();
            policy.Anonymous.Flags = 0b111; // NoRemoteImages | NoLowMandatoryLabelImages | PreferSystem32Images
            let _ = SetProcessMitigationPolicy(
                ProcessImageLoadPolicy,
                (&raw const policy).cast(),
                std::mem::size_of::<PROCESS_MITIGATION_IMAGE_LOAD_POLICY>(),
            );
        }
    }
}
