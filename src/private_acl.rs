//! Read-only validation of macOS extended ACLs for private storage.
//!
//! This module deliberately does not repair ACLs. ACLs are fetched from open
//! descriptors so validation applies to the object the caller actually opened.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

type Acl = *mut std::ffi::c_void;
type AclEntry = *mut std::ffi::c_void;

const ACL_TYPE_EXTENDED: i32 = 0x0000_0100;
const ACL_FIRST_ENTRY: i32 = 0;
const ACL_NEXT_ENTRY: i32 = -1;
const ACL_EXTENDED_ALLOW: i32 = 1;
const ACL_EXTENDED_DENY: i32 = 2;
const ACL_MAX_ENTRIES: usize = 128;

const ACL_READ_DATA: u64 = 1 << 1;
const ACL_WRITE_DATA: u64 = 1 << 2;
const ACL_EXECUTE: u64 = 1 << 3;
const ACL_DELETE: u64 = 1 << 4;
const ACL_APPEND_DATA: u64 = 1 << 5;
const ACL_DELETE_CHILD: u64 = 1 << 6;
const ACL_READ_ATTRIBUTES: u64 = 1 << 7;
const ACL_WRITE_ATTRIBUTES: u64 = 1 << 8;
const ACL_READ_EXTATTRIBUTES: u64 = 1 << 9;
const ACL_WRITE_EXTATTRIBUTES: u64 = 1 << 10;
const ACL_READ_SECURITY: u64 = 1 << 11;
const ACL_WRITE_SECURITY: u64 = 1 << 12;
const ACL_CHANGE_OWNER: u64 = 1 << 13;
const ACL_SYNCHRONIZE: u64 = 1 << 20;

const ACL_KNOWN_PERMISSIONS: u64 = ACL_READ_DATA
    | ACL_WRITE_DATA
    | ACL_EXECUTE
    | ACL_DELETE
    | ACL_APPEND_DATA
    | ACL_DELETE_CHILD
    | ACL_READ_ATTRIBUTES
    | ACL_WRITE_ATTRIBUTES
    | ACL_READ_EXTATTRIBUTES
    | ACL_WRITE_EXTATTRIBUTES
    | ACL_READ_SECURITY
    | ACL_WRITE_SECURITY
    | ACL_CHANGE_OWNER
    | ACL_SYNCHRONIZE;

// Directory mutation rights (including metadata and security changes). The
// read/search bits are intentionally omitted so shared ancestor directories
// remain usable while callers can still reject directories outsiders control.
const ACL_ANCESTOR_MUTATING_PERMISSIONS: u64 = ACL_WRITE_DATA
    | ACL_DELETE
    | ACL_APPEND_DATA
    | ACL_DELETE_CHILD
    | ACL_WRITE_ATTRIBUTES
    | ACL_WRITE_EXTATTRIBUTES
    | ACL_WRITE_SECURITY
    | ACL_CHANGE_OWNER;

#[link(name = "System")]
unsafe extern "C" {
    fn acl_get_fd_np(fd: i32, ty: i32) -> Acl;
    fn acl_free(object: *mut std::ffi::c_void) -> i32;
    fn acl_get_entry(acl: Acl, entry_id: i32, entry: *mut AclEntry) -> i32;
    fn acl_valid(acl: Acl) -> i32;
    fn acl_get_tag_type(entry: AclEntry, tag: *mut i32) -> i32;
    fn acl_get_permset_mask_np(entry: AclEntry, mask: *mut u64) -> i32;
    fn acl_get_qualifier(entry: AclEntry) -> *mut std::ffi::c_void;
    fn mbr_uid_to_uuid(uid: u32, uuid: *mut u8) -> i32;
}

#[derive(Clone, Copy)]
struct Entry {
    tag: i32,
    permissions: u64,
    qualifier: Option<[u8; 16]>,
}

struct OwnedAcl(Acl);

impl Drop for OwnedAcl {
    fn drop(&mut self) {
        // SAFETY: `acl_get_fd_np` returns an owned ACL object; `acl_free` releases it.
        // Drop runs exactly once for each non-null result.
        unsafe {
            let _ = acl_free(self.0);
        }
    }
}

struct OwnedQualifier(*mut std::ffi::c_void);

impl Drop for OwnedQualifier {
    fn drop(&mut self) {
        // SAFETY: `acl_get_qualifier` allocates a qualifier that the caller must free
        // with `acl_free`, including when UUID comparison later fails.
        unsafe {
            let _ = acl_free(self.0);
        }
    }
}

fn acl_error() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "ACL inspection failed")
}

fn unsafe_path_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "ancestor directory could not be opened safely",
    )
}

/// Require that an opened file has no allow entries for principals other than
/// its owner. Empty ACLs and deny-only ACLs are permitted.
pub fn require_private(file: &File) -> io::Result<()> {
    let metadata = file.metadata().map_err(|_| acl_error())?;
    let entries = read_acl(file)?;
    let mut owner_uuid = None;

    for entry in entries {
        if entry.tag != ACL_EXTENDED_ALLOW {
            continue;
        }

        let qualifier = entry.qualifier.ok_or_else(acl_error)?;
        if owner_uuid.is_none() {
            owner_uuid = Some(uid_to_uuid(metadata.uid())?);
        }

        if Some(qualifier) != owner_uuid {
            return Err(acl_error());
        }
    }

    Ok(())
}

/// Require that an opened directory grants no mutating ACL permissions to
/// other principals. Read, search, and metadata-read grants are allowed.
pub fn require_safe_ancestor(file: &File) -> io::Result<()> {
    let metadata = file.metadata().map_err(|_| acl_error())?;
    if !metadata.is_dir() {
        return Err(acl_error());
    }

    let entries = read_acl(file)?;
    let mut owner_uuid = None;
    for entry in entries {
        if entry.tag != ACL_EXTENDED_ALLOW
            || entry.permissions & ACL_ANCESTOR_MUTATING_PERMISSIONS == 0
        {
            continue;
        }
        let qualifier = entry.qualifier.ok_or_else(acl_error)?;
        if owner_uuid.is_none() {
            owner_uuid = Some(uid_to_uuid(metadata.uid())?);
        }
        if Some(qualifier) != owner_uuid {
            return Err(acl_error());
        }
    }

    Ok(())
}

fn uid_to_uuid(uid: u32) -> io::Result<[u8; 16]> {
    let mut uuid = [0u8; 16];
    // SAFETY: `mbr_uid_to_uuid` writes exactly 16 UUID bytes on success and, unlike
    // most POSIX calls, returns an errno value directly.
    let result = unsafe { mbr_uid_to_uuid(uid, uuid.as_mut_ptr()) };
    if result != 0 {
        return Err(acl_error());
    }
    Ok(uuid)
}

/// Open an existing directory without following a final symlink, then check
/// its ACL. This performs no path mutation.
pub fn check_ancestor_path(path: &Path) -> io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY);
    let file = options.open(path).map_err(|_| unsafe_path_error())?;
    require_safe_ancestor(&file)
}

fn read_acl(file: &File) -> io::Result<Vec<Entry>> {
    // SAFETY: `acl_get_fd_np` borrows the descriptor and returns an independently
    // owned ACL object. The File remains open and its ownership is unchanged.
    let raw_acl = unsafe { acl_get_fd_np(file.as_raw_fd(), ACL_TYPE_EXTENDED) };
    if raw_acl.is_null() {
        // Darwin reports an absent extended ACL as ENOENT or ENOATTR on a
        // live descriptor. No pathname lookup occurs in this call.
        // Other errors (including EACCES and filesystem lookup failures) fail
        // closed rather than being treated as an empty ACL.
        let error = io::Error::last_os_error();
        if matches!(error.raw_os_error(), Some(libc::ENOENT | libc::ENOATTR)) {
            return Ok(Vec::new());
        }
        return Err(acl_error());
    }

    let acl = OwnedAcl(raw_acl);
    // SAFETY: the non-null ACL is an owned object returned by acl_get_fd_np.
    if unsafe { acl_valid(acl.0) } != 0 {
        return Err(acl_error());
    }

    let mut entries = Vec::with_capacity(8);
    let mut entry = std::ptr::null_mut();
    let mut entry_id = ACL_FIRST_ENTRY;

    loop {
        // SAFETY: the owned ACL remains valid and the output slot is writable.
        // Darwin returns 0 for an entry and -1/EINVAL at end-of-list.
        let result = unsafe { acl_get_entry(acl.0, entry_id, &mut entry) };
        if result == -1 {
            if io::Error::last_os_error().raw_os_error() == Some(libc::EINVAL) {
                break;
            }
            return Err(acl_error());
        }
        if result != 0 || entry.is_null() {
            return Err(acl_error());
        }
        if entries.len() == ACL_MAX_ENTRIES {
            return Err(acl_error());
        }

        let mut tag = 0;
        let mut permissions = 0u64;
        // SAFETY: entry is borrowed from the still-live ACL. Both output
        // pointers have the exact SDK ABI types and remain live for the call.
        let tag_result = unsafe { acl_get_tag_type(entry, &mut tag) };
        // SAFETY: same entry lifetime; permissions is one writable uint64_t.
        let permissions_result = unsafe { acl_get_permset_mask_np(entry, &mut permissions) };
        if tag_result != 0 || permissions_result != 0 || permissions & !ACL_KNOWN_PERMISSIONS != 0 {
            return Err(acl_error());
        }

        let qualifier = match tag {
            ACL_EXTENDED_ALLOW => Some(read_qualifier(entry)?),
            ACL_EXTENDED_DENY => None,
            _ => return Err(acl_error()),
        };
        entries.push(Entry {
            tag,
            permissions,
            qualifier,
        });
        entry_id = ACL_NEXT_ENTRY;
    }

    Ok(entries)
}

fn read_qualifier(entry: AclEntry) -> io::Result<[u8; 16]> {
    // SAFETY: entry is borrowed from a live extended ACL and has ALLOW tag.
    let raw = unsafe { acl_get_qualifier(entry) };
    if raw.is_null() {
        return Err(acl_error());
    }
    let qualifier = OwnedQualifier(raw);
    // SAFETY: Darwin extended ACL qualifiers are allocated 16-byte UUIDs;
    // [u8; 16] has byte alignment and is copied before its allocation is freed.
    let uuid = unsafe { *(qualifier.0.cast::<[u8; 16]>()) };
    Ok(uuid)
}
