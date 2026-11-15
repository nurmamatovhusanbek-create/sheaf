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

#[cfg(windows)]
#[allow(unsafe_code)]
mod win {
    use windows::core::w;
    use windows::Win32::System::LibraryLoader::{
        SetDefaultDllDirectories, SetDllDirectoryW, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
    };
    use windows::Win32::System::SystemServices::PROCESS_MITIGATION_IMAGE_LOAD_POLICY;
    use windows::Win32::System::Threading::{ProcessImageLoadPolicy, SetProcessMitigationPolicy};

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
