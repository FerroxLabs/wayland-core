//! Integrity-level probe — a tiny test helper that prints the current
//! process's token integrity level via direct Win32 calls (no LSA lookup,
//! no group enumeration, nothing that requires Administrators / Users SID
//! membership). Spawned through the AppContainer sandbox by the live
//! integrity-verification test in `tests/live_integrity.rs`.
//!
//! Output (one line, then `\n`):
//!   `IL=Low` / `IL=Medium` / `IL=High` / `IL=System` / `IL=Untrusted` /
//!   `IL_RID=0x<hex>`
//!
//! Exit codes:
//!   0 on success (line printed)
//!   2 OpenProcessToken failed
//!   3 GetTokenInformation failed
//!   4 not Windows

#[cfg(windows)]
fn main() {
    use std::ptr;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_MANDATORY_LABEL,
        TOKEN_QUERY, TokenIntegrityLevel,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token: HANDLE = ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            eprintln!("il_probe: OpenProcessToken failed");
            std::process::exit(2);
        }

        let mut needed: u32 = 0;
        // First call to learn required buffer size.
        let _ = GetTokenInformation(token, TokenIntegrityLevel, ptr::null_mut(), 0, &mut needed);
        let mut buf: Vec<u8> = vec![0u8; needed as usize];
        if GetTokenInformation(
            token,
            TokenIntegrityLevel,
            buf.as_mut_ptr() as _,
            needed,
            &mut needed,
        ) == 0
        {
            eprintln!("il_probe: GetTokenInformation failed");
            CloseHandle(token);
            std::process::exit(3);
        }

        let label = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        let sid = label.Label.Sid;
        let count = *GetSidSubAuthorityCount(sid as _);
        let rid = *GetSidSubAuthority(sid as _, (count - 1) as u32);

        let label = match rid {
            0x0000 => "Untrusted",
            0x1000 => "Low",
            0x2000 => "Medium",
            0x2100 => "Medium-Plus",
            0x3000 => "High",
            0x4000 => "System",
            _ => {
                println!("IL_RID=0x{rid:x}");
                CloseHandle(token);
                return;
            }
        };
        println!("IL={label}");
        CloseHandle(token);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("il_probe is Windows-only");
    std::process::exit(4);
}
