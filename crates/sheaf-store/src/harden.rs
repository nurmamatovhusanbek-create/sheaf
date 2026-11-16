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

/// File in a session folder that says "this folder was labelled low integrity, a worker may lower itself".
#[cfg(windows)]
const LOW_MARK: &str = ".low-integrity";

/// Make a freshly created session folder writable by a low-integrity process (everything created in it
/// afterwards inherits that), and leave the marker that tells the worker it may lower itself. Without the
/// marker (a file system with no labels) the worker stays at the user's normal level rather than being unable
/// to write. A no-op elsewhere.
pub(crate) fn label_session_low(dir: &std::path::Path) {
    #[cfg(windows)]
    if win::label_dir_low(dir) {
        let _ = std::fs::write(dir.join(LOW_MARK), b"");
    }
    #[cfg(not(windows))]
    let _ = dir;
}

/// For a worker, once it knows its session folder: drop to low integrity and give up every privilege but the
/// one that lets it reach files under folders it cannot list. A low-integrity process cannot write to
/// anything the user's normal programs own (documents, settings, the registry, other programs), only to
/// objects labelled low, and the session folder is. Temporary files go there too. Does nothing unless the
/// session folder carries the marker from [`label_session_low`]. Failures are ignored. A no-op elsewhere.
pub fn lower_worker(session: &std::path::Path) {
    #[cfg(windows)]
    if session.join(LOW_MARK).is_file() {
        std::env::set_var("TEMP", session);
        std::env::set_var("TMP", session);
        win::lower();
    }
    #[cfg(not(windows))]
    let _ = session;
}

/// The integrity level (the last number of the level's identifier: 0x1000 low, 0x2000 medium, 0x3000 high) of
/// a running process of this user, for tests and diagnostics. `None` if it cannot be read, and elsewhere.
pub fn integrity_level_of(pid: u32) -> Option<u32> {
    #[cfg(windows)]
    return win::integrity_of(pid);
    #[cfg(not(windows))]
    {
        let _ = pid;
        None
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod win {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, LocalFree, ERROR_SUCCESS, HANDLE, HLOCAL, LUID};
    use windows::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, ConvertStringSidToSidW, SetNamedSecurityInfoW,
        SDDL_REVISION_1, SE_FILE_OBJECT,
    };
    use windows::Win32::Security::{
        AdjustTokenPrivileges, GetLengthSid, GetSecurityDescriptorSacl, GetSidSubAuthority, GetSidSubAuthorityCount,
        GetTokenInformation, LookupPrivilegeValueW, SetTokenInformation, TokenIntegrityLevel, TokenPrivileges, ACL,
        LABEL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_PRIVILEGE_REMOVED, SID_AND_ATTRIBUTES,
        TOKEN_ADJUST_DEFAULT, TOKEN_ADJUST_PRIVILEGES, TOKEN_MANDATORY_LABEL, TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows::Win32::System::LibraryLoader::{
        SetDefaultDllDirectories, SetDllDirectoryW, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
    };
    use windows::Win32::System::SystemServices::{
        PROCESS_MITIGATION_DYNAMIC_CODE_POLICY, PROCESS_MITIGATION_EXTENSION_POINT_DISABLE_POLICY,
        PROCESS_MITIGATION_IMAGE_LOAD_POLICY, PROCESS_MITIGATION_STRICT_HANDLE_CHECK_POLICY, SE_GROUP_INTEGRITY,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, ProcessDynamicCodePolicy, ProcessExtensionPointDisablePolicy,
        ProcessImageLoadPolicy, ProcessStrictHandleCheckPolicy, SetProcessMitigationPolicy,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    /// "Mandatory label, inheritable, no write-up, low": anything below low cannot write here, low can.
    const LOW_LABEL: windows::core::PCWSTR = w!("S:(ML;OICI;NW;;;LW)");

    pub fn label_dir_low(dir: &std::path::Path) -> bool {
        let path: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        // SAFETY: the descriptor is made and freed here; the path buffer is NUL-terminated and outlives the call.
        unsafe {
            let mut descriptor = PSECURITY_DESCRIPTOR::default();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(LOW_LABEL, SDDL_REVISION_1, &mut descriptor, None)
                .is_err()
            {
                return false;
            }
            let mut present = windows::core::BOOL::default();
            let mut defaulted = windows::core::BOOL::default();
            let mut sacl: *mut ACL = std::ptr::null_mut();
            let got = GetSecurityDescriptorSacl(descriptor, &mut present, &mut sacl, &mut defaulted).is_ok()
                && present.as_bool()
                && !sacl.is_null();
            let done = got
                && SetNamedSecurityInfoW(
                    PCWSTR(path.as_ptr()),
                    SE_FILE_OBJECT,
                    LABEL_SECURITY_INFORMATION,
                    None,
                    None,
                    None,
                    Some(sacl),
                ) == ERROR_SUCCESS;
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
            done
        }
    }

    pub fn lower() {
        // SAFETY: a handle to our own token, closed at the end; the label structure and the SID it points to
        // live through the call; the privilege buffer is aligned for the structure and sized by the system.
        unsafe {
            let mut token = HANDLE::default();
            if OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_ADJUST_DEFAULT | TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
                &mut token,
            )
            .is_err()
            {
                return;
            }
            let mut sid = PSID::default();
            if ConvertStringSidToSidW(w!("S-1-16-4096"), &mut sid).is_ok() {
                let label = TOKEN_MANDATORY_LABEL {
                    Label: SID_AND_ATTRIBUTES {
                        Sid: sid,
                        Attributes: SE_GROUP_INTEGRITY as u32,
                    },
                };
                let _ = SetTokenInformation(
                    token,
                    TokenIntegrityLevel,
                    (&raw const label).cast(),
                    std::mem::size_of::<TOKEN_MANDATORY_LABEL>() as u32 + GetLengthSid(sid),
                );
                let _ = LocalFree(Some(HLOCAL(sid.0)));
            }
            remove_privileges(token);
            let _ = CloseHandle(token);
        }
    }

    /// Remove every privilege of the token except "bypass traverse checking", which ordinary file access under
    /// a folder the user cannot list depends on.
    unsafe fn remove_privileges(token: HANDLE) {
        let mut need = 0u32;
        let _ = GetTokenInformation(token, TokenPrivileges, None, 0, &mut need);
        if need == 0 {
            return;
        }
        let mut buffer = vec![0u32; (need as usize).div_ceil(4)];
        let at = buffer.as_mut_ptr().cast::<TOKEN_PRIVILEGES>();
        if GetTokenInformation(token, TokenPrivileges, Some(at.cast()), need, &mut need).is_err() {
            return;
        }
        let mut keep = LUID::default();
        let _ = LookupPrivilegeValueW(None, w!("SeChangeNotifyPrivilege"), &mut keep);
        let list = std::slice::from_raw_parts_mut((*at).Privileges.as_mut_ptr(), (*at).PrivilegeCount as usize);
        for entry in list.iter_mut() {
            if entry.Luid.LowPart != keep.LowPart || entry.Luid.HighPart != keep.HighPart {
                entry.Attributes = SE_PRIVILEGE_REMOVED;
            }
        }
        let _ = AdjustTokenPrivileges(token, false, Some(at), 0, None, None);
    }

    pub fn integrity_of(pid: u32) -> Option<u32> {
        // SAFETY: handles opened here are closed here; the buffer is aligned for the structure and sized by the
        // system; the SID it holds is read while the buffer is alive.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut token = HANDLE::default();
            let opened = OpenProcessToken(process, TOKEN_QUERY, &mut token).is_ok();
            let _ = CloseHandle(process);
            if !opened {
                return None;
            }
            let mut need = 0u32;
            let _ = GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut need);
            let mut buffer = vec![0u64; (need as usize).div_ceil(8).max(1)];
            let level = if need > 0
                && GetTokenInformation(
                    token,
                    TokenIntegrityLevel,
                    Some(buffer.as_mut_ptr().cast()),
                    need,
                    &mut need,
                )
                .is_ok()
            {
                let label = &*buffer.as_ptr().cast::<TOKEN_MANDATORY_LABEL>();
                let count = *GetSidSubAuthorityCount(label.Label.Sid);
                (count > 0).then(|| *GetSidSubAuthority(label.Label.Sid, u32::from(count) - 1))
            } else {
                None
            };
            let _ = CloseHandle(token);
            level
        }
    }

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
