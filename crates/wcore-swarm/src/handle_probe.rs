//! TEMPORARY 20-75 workstream C instrumentation. NOT FOR COMMIT.
//!
//! Enumerates every handle THIS PROCESS holds whose final path matches a target
//! directory, reporting each handle's granted access mask. Sysinternals
//! `handle.exe` is not installed on the proof box, so this is the
//! "`NtQueryObject` / equivalent probe" the plan allows instead.

use std::path::Path;

use windows_sys::Wdk::Foundation::NtQueryObject;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;

#[repr(C)]
#[derive(Default)]
struct ObjectBasicInformation {
    attributes: u32,
    granted_access: u32,
    handle_count: u32,
    pointer_count: u32,
    paged_pool_charge: u32,
    non_paged_pool_charge: u32,
    reserved: [u32; 3],
    name_info_size: u32,
    type_info_size: u32,
    security_descriptor_size: u32,
    creation_time: i64,
}

fn final_path(handle: HANDLE) -> Option<String> {
    let mut buffer = vec![0_u16; 32 * 1024];
    let length = unsafe {
        GetFinalPathNameByHandleW(handle, buffer.as_mut_ptr(), buffer.len() as u32 - 1, 0)
    };
    if length == 0 || length as usize >= buffer.len() {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..length as usize]))
}

fn granted_access(handle: HANDLE) -> Option<u32> {
    let mut info = ObjectBasicInformation::default();
    let mut returned = 0_u32;
    let status = unsafe {
        NtQueryObject(
            handle,
            0, // ObjectBasicInformation
            std::ptr::addr_of_mut!(info).cast(),
            std::mem::size_of::<ObjectBasicInformation>() as u32,
            &mut returned,
        )
    };
    if status < 0 {
        return None;
    }
    Some(info.granted_access)
}

fn decode(mask: u32) -> String {
    const BITS: &[(u32, &str)] = &[
        (0x0001_0000, "DELETE"),
        (0x0002_0000, "READ_CONTROL"),
        (0x0004_0000, "WRITE_DAC"),
        (0x0008_0000, "WRITE_OWNER"),
        (0x0010_0000, "SYNCHRONIZE"),
        (0x0000_0001, "FILE_READ_DATA/LIST_DIRECTORY"),
        (0x0000_0002, "FILE_WRITE_DATA/ADD_FILE"),
        (0x0000_0004, "FILE_APPEND_DATA/ADD_SUBDIRECTORY"),
        (0x0000_0008, "FILE_READ_EA"),
        (0x0000_0010, "FILE_WRITE_EA"),
        (0x0000_0020, "FILE_EXECUTE/TRAVERSE"),
        (0x0000_0040, "FILE_DELETE_CHILD"),
        (0x0000_0080, "FILE_READ_ATTRIBUTES"),
        (0x0000_0100, "FILE_WRITE_ATTRIBUTES"),
    ];
    let mut parts = Vec::new();
    for (bit, name) in BITS {
        if mask & bit != 0 {
            parts.push(*name);
        }
    }
    parts.join("|")
}

/// Report every handle this process holds on `target`.
pub(crate) fn dump_handles_on(target: &Path, marker: &str) {
    let wanted = target
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_lowercase();
    eprintln!("PROBE[{marker}] enumerating own-process handles on {wanted}");
    let mut found = 0_usize;
    // Windows handle values are multiples of 4. Scanning our OWN table keeps the
    // probe attributable: every hit is a handle this process opened.
    for raw in (4_usize..=65_532).step_by(4) {
        let handle = raw as HANDLE;
        let Some(path) = final_path(handle) else {
            continue;
        };
        let normalized = path.trim_start_matches(r"\\?\").to_lowercase();
        if normalized != wanted {
            continue;
        }
        found += 1;
        let mask = granted_access(handle).unwrap_or(0);
        eprintln!(
            "PROBE[{marker}]   handle 0x{raw:x} granted=0x{mask:08x} [{}]",
            decode(mask)
        );
    }
    eprintln!("PROBE[{marker}] total handles on target: {found}");
}
