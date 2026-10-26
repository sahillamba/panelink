//! Strict, bounded binary codec for the workspace index metadata.
//!
//! This module only encodes and decodes in-memory metadata. It performs no
//! file access, migration, recovery, checksum, or power-loss guarantees.

use super::{
    validate_name, Project, ProjectId, WorkspaceIndex, WorkspaceIndexError, WorkspaceTab,
    WorkspaceTabId, MAX_DISPLAY_NAME_BYTES, MAX_PROJECTS, MAX_TABS_PER_PROJECT,
};
use std::fmt;

const MAGIC: &[u8; 8] = b"PLWKIDX1";
const VERSION: u16 = 1;
const HEADER_LEN: usize = 56;
const PROJECT_HEADER_LEN: usize = 24;
const TAB_HEADER_LEN: usize = 16;
const LEGACY_TAB_ID: u64 = 1;
const MAX_ENCODED_LEN: usize = HEADER_LEN
    + MAX_PROJECTS
        * (PROJECT_HEADER_LEN
            + MAX_DISPLAY_NAME_BYTES
            + MAX_TABS_PER_PROJECT * (TAB_HEADER_LEN + MAX_DISPLAY_NAME_BYTES));

/// Validated metadata plus the persistent ID allocator state.
#[derive(Clone, PartialEq, Eq)]
pub struct MetadataRecord {
    revision: u64,
    next_id: u64,
    index: WorkspaceIndex,
}

/// A stable, non-name-bearing codec error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodecError {
    InvalidLength,
    UnsupportedVersion,
    ReservedField,
    InvalidRevision,
    InvalidWatermark,
    InvalidLegacyTabId,
    InvalidCount,
    InvalidName,
    DuplicateProjectId,
    DuplicateTabId,
    InvalidSelection,
    InvalidActiveSelection,
}

impl fmt::Debug for MetadataRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MetadataRecord")
            .field("revision", &self.revision)
            .field("next_id", &self.next_id)
            .field("project_count", &self.index.projects.len())
            .finish()
    }
}

impl MetadataRecord {
    /// Creates a validated record. `next_id` must exceed every allocated ID.
    pub fn new(index: WorkspaceIndex, revision: u64, next_id: u64) -> Result<Self, CodecError> {
        if revision == 0 {
            return Err(CodecError::InvalidRevision);
        }
        validate_index(&index, next_id)?;
        Ok(Self {
            revision,
            next_id,
            index,
        })
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub const fn next_id(&self) -> u64 {
        self.next_id
    }

    pub const fn index(&self) -> &WorkspaceIndex {
        &self.index
    }

    /// Encodes this record in the canonical version 1 byte representation.
    pub fn encode(&self) -> Result<Vec<u8>, CodecError> {
        validate_index(&self.index, self.next_id)?;
        let total_len = encoded_len(&self.index)?;
        if total_len > MAX_ENCODED_LEN || total_len > u32::MAX as usize {
            return Err(CodecError::InvalidLength);
        }

        let mut out = Vec::with_capacity(total_len);
        out.extend_from_slice(MAGIC);
        put_u16(&mut out, VERSION);
        put_u16(&mut out, 0);
        put_u32(&mut out, total_len as u32);
        put_u64(&mut out, self.revision);
        put_u64(&mut out, self.next_id);
        put_u64(&mut out, LEGACY_TAB_ID);
        put_u64(
            &mut out,
            self.index.active_project_id.map_or(0, ProjectId::get),
        );
        put_u16(&mut out, self.index.projects.len() as u16);
        out.extend_from_slice(&[0; 6]);

        for project in &self.index.projects {
            put_u64(&mut out, project.id.get());
            put_u64(
                &mut out,
                project.selected_tab_id.map_or(0, WorkspaceTabId::get),
            );
            put_u16(&mut out, project.tabs.len() as u16);
            put_u16(&mut out, project.display_name.len() as u16);
            put_u32(&mut out, 0);
            out.extend_from_slice(project.display_name.as_bytes());
            for tab in &project.tabs {
                put_u64(&mut out, tab.id.get());
                put_u16(&mut out, tab.display_name.len() as u16);
                out.extend_from_slice(&[0; 6]);
                out.extend_from_slice(tab.display_name.as_bytes());
            }
        }
        debug_assert_eq!(out.len(), total_len);
        Ok(out)
    }

    /// Decodes a complete record, rejecting unknown or noncanonical structure.
    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        if bytes.len() < HEADER_LEN || bytes.len() > MAX_ENCODED_LEN {
            return Err(CodecError::InvalidLength);
        }
        if &bytes[..8] != MAGIC {
            return Err(CodecError::InvalidLength);
        }
        if read_u16(bytes, 8)? != VERSION {
            return Err(CodecError::UnsupportedVersion);
        }
        if read_u16(bytes, 10)? != 0 || bytes[50..56].iter().any(|byte| *byte != 0) {
            return Err(CodecError::ReservedField);
        }
        if read_u32(bytes, 12)? as usize != bytes.len() {
            return Err(CodecError::InvalidLength);
        }
        let revision = read_u64(bytes, 16)?;
        if revision == 0 {
            return Err(CodecError::InvalidRevision);
        }
        let next_id = read_u64(bytes, 24)?;
        if next_id < 2 {
            return Err(CodecError::InvalidWatermark);
        }
        if read_u64(bytes, 32)? != LEGACY_TAB_ID {
            return Err(CodecError::InvalidLegacyTabId);
        }
        let active_raw = read_u64(bytes, 40)?;
        let project_count = read_u16(bytes, 48)? as usize;
        if project_count > MAX_PROJECTS {
            return Err(CodecError::InvalidCount);
        }
        // Prove the minimum possible table fits before allocating any vectors.
        let minimum = HEADER_LEN
            .checked_add(
                project_count
                    .checked_mul(PROJECT_HEADER_LEN)
                    .ok_or(CodecError::InvalidLength)?,
            )
            .ok_or(CodecError::InvalidLength)?;
        if minimum > bytes.len() {
            return Err(CodecError::InvalidLength);
        }

        let mut projects = Vec::with_capacity(project_count);
        let mut offset = HEADER_LEN;
        let mut greatest_id = 0;
        for _ in 0..project_count {
            let header = take(bytes, &mut offset, PROJECT_HEADER_LEN)?;
            let project_id = read_u64(header, 0)?;
            let selected_raw = read_u64(header, 8)?;
            let tab_count = read_u16(header, 16)? as usize;
            let name_len = read_u16(header, 18)? as usize;
            if read_u32(header, 20)? != 0 {
                return Err(CodecError::ReservedField);
            }
            if project_id == 0 || tab_count > MAX_TABS_PER_PROJECT {
                return Err(CodecError::InvalidCount);
            }
            if name_len == 0 || name_len > MAX_DISPLAY_NAME_BYTES {
                return Err(CodecError::InvalidName);
            }
            let minimum_project = tab_count
                .checked_mul(TAB_HEADER_LEN)
                .and_then(|size| size.checked_add(name_len))
                .ok_or(CodecError::InvalidLength)?;
            if minimum_project > bytes.len().saturating_sub(offset) {
                return Err(CodecError::InvalidLength);
            }
            let display_name = read_name(bytes, &mut offset, name_len)?;
            let project_id = ProjectId(project_id);
            if projects
                .iter()
                .any(|project: &Project| project.id == project_id)
            {
                return Err(CodecError::DuplicateProjectId);
            }
            greatest_id = greatest_id.max(project_id.get());

            // Bound the vector reservation by the validated per-project limit.
            let mut tabs = Vec::with_capacity(tab_count);
            for _ in 0..tab_count {
                let tab_header = take(bytes, &mut offset, TAB_HEADER_LEN)?;
                let tab_id_raw = read_u64(tab_header, 0)?;
                let tab_name_len = read_u16(tab_header, 8)? as usize;
                if tab_header[10..16].iter().any(|byte| *byte != 0) {
                    return Err(CodecError::ReservedField);
                }
                if tab_id_raw == 0 || tab_name_len == 0 || tab_name_len > MAX_DISPLAY_NAME_BYTES {
                    return Err(CodecError::InvalidName);
                }
                let display_name = read_name(bytes, &mut offset, tab_name_len)?;
                let tab_id = WorkspaceTabId(tab_id_raw);
                if projects
                    .iter()
                    .any(|p| p.tabs.iter().any(|tab| tab.id == tab_id))
                    || tabs.iter().any(|tab: &WorkspaceTab| tab.id == tab_id)
                {
                    return Err(CodecError::DuplicateTabId);
                }
                greatest_id = greatest_id.max(tab_id.get());
                tabs.push(WorkspaceTab {
                    id: tab_id,
                    display_name,
                });
            }
            let selected_tab_id = if selected_raw == 0 {
                None
            } else {
                let selected = WorkspaceTabId(selected_raw);
                if !tabs.iter().any(|tab| tab.id == selected) {
                    return Err(CodecError::InvalidSelection);
                }
                Some(selected)
            };
            if tabs.is_empty() && selected_tab_id.is_some() {
                return Err(CodecError::InvalidSelection);
            }
            projects.push(Project {
                id: project_id,
                display_name,
                tabs,
                selected_tab_id,
            });
        }
        if offset != bytes.len() {
            return Err(CodecError::InvalidLength);
        }
        if next_id <= greatest_id {
            return Err(CodecError::InvalidWatermark);
        }
        let active_project_id = if active_raw == 0 {
            if !projects.is_empty() {
                return Err(CodecError::InvalidActiveSelection);
            }
            None
        } else {
            let active = ProjectId(active_raw);
            let project = projects
                .iter()
                .find(|project| project.id == active)
                .ok_or(CodecError::InvalidActiveSelection)?;
            if project.tabs.is_empty() || project.selected_tab_id.is_none() {
                return Err(CodecError::InvalidActiveSelection);
            }
            Some(active)
        };
        let index = WorkspaceIndex {
            projects,
            active_project_id,
        };
        Self::new(index, revision, next_id)
    }
}

fn validate_index(index: &WorkspaceIndex, next_id: u64) -> Result<(), CodecError> {
    if next_id < 2 {
        return Err(CodecError::InvalidWatermark);
    }
    if index.projects.len() > MAX_PROJECTS {
        return Err(CodecError::InvalidCount);
    }
    let mut greatest_id = 0;
    for (i, project) in index.projects.iter().enumerate() {
        if project.id.get() == 0 || project.tabs.len() > MAX_TABS_PER_PROJECT {
            return Err(CodecError::InvalidCount);
        }
        validate_name(&project.display_name).map_err(map_name_error)?;
        if index.projects[..i]
            .iter()
            .any(|other| other.id == project.id)
        {
            return Err(CodecError::DuplicateProjectId);
        }
        greatest_id = greatest_id.max(project.id.get());
        if project.tabs.is_empty() && project.selected_tab_id.is_some() {
            return Err(CodecError::InvalidSelection);
        }
        if let Some(selected) = project.selected_tab_id {
            if !project.tabs.iter().any(|tab| tab.id == selected) {
                return Err(CodecError::InvalidSelection);
            }
        }
        for (j, tab) in project.tabs.iter().enumerate() {
            validate_name(&tab.display_name).map_err(map_name_error)?;
            if tab.id.get() == 0
                || index.projects[..i]
                    .iter()
                    .any(|other| other.tabs.iter().any(|t| t.id == tab.id))
                || project.tabs[..j].iter().any(|other| other.id == tab.id)
            {
                return Err(CodecError::DuplicateTabId);
            }
            greatest_id = greatest_id.max(tab.id.get());
        }
    }
    if next_id <= greatest_id {
        return Err(CodecError::InvalidWatermark);
    }
    match index.active_project_id {
        None if !index.projects.is_empty() => return Err(CodecError::InvalidActiveSelection),
        Some(active) => {
            let project = index
                .projects
                .iter()
                .find(|project| project.id == active)
                .ok_or(CodecError::InvalidActiveSelection)?;
            if project.tabs.is_empty() || project.selected_tab_id.is_none() {
                return Err(CodecError::InvalidActiveSelection);
            }
        }
        None => {}
    }
    Ok(())
}

fn map_name_error(_: WorkspaceIndexError) -> CodecError {
    CodecError::InvalidName
}

fn encoded_len(index: &WorkspaceIndex) -> Result<usize, CodecError> {
    let mut len = HEADER_LEN;
    for project in &index.projects {
        len = len
            .checked_add(PROJECT_HEADER_LEN + project.display_name.len())
            .ok_or(CodecError::InvalidLength)?;
        for tab in &project.tabs {
            len = len
                .checked_add(TAB_HEADER_LEN + tab.display_name.len())
                .ok_or(CodecError::InvalidLength)?;
        }
    }
    Ok(len)
}

fn read_name(bytes: &[u8], offset: &mut usize, len: usize) -> Result<String, CodecError> {
    let raw = take(bytes, offset, len)?;
    let name = std::str::from_utf8(raw).map_err(|_| CodecError::InvalidName)?;
    validate_name(name).map_err(map_name_error)?;
    Ok(name.to_owned())
}

fn take<'a>(bytes: &'a [u8], offset: &mut usize, len: usize) -> Result<&'a [u8], CodecError> {
    let end = offset.checked_add(len).ok_or(CodecError::InvalidLength)?;
    let slice = bytes.get(*offset..end).ok_or(CodecError::InvalidLength)?;
    *offset = end;
    Ok(slice)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, CodecError> {
    Ok(u16::from_be_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or(CodecError::InvalidLength)?
            .try_into()
            .unwrap(),
    ))
}
fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, CodecError> {
    Ok(u32::from_be_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(CodecError::InvalidLength)?
            .try_into()
            .unwrap(),
    ))
}
fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, CodecError> {
    Ok(u64::from_be_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(CodecError::InvalidLength)?
            .try_into()
            .unwrap(),
    ))
}
fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}
fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}
fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_be_bytes());
}
