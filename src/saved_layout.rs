//! Bounded historical saved-layout record (pure codec/model).
//!
//! Not a persistence, runtime or restore facility.

use crate::control_protocol::TargetRef;
use crate::pane_layout::{
    LayoutSnapshot, PaneGeometry, Rect, MAX_PANES, MAX_PANE_COLUMNS, MAX_PANE_ROWS,
};
use std::fmt;

/// Exact encoded record size.
/// Current four-pane record size.
pub const SAVED_LAYOUT_BYTES: usize = 280;
/// Historical schema-1 record size, accepted on read only.
pub const LEGACY_SAVED_LAYOUT_BYTES: usize = 160;
const MAX_SAVED_PANES: u8 = 4;
// Saved-layout schemas predate the current live pane inset. Keep their
// historical geometry domain stable so existing records remain readable and
// canonical even though new live layouts require 7x22 outer rectangles.
const MIN_SAVED_ROWS: u16 = 5;
const MIN_SAVED_COLUMNS: u16 = 20;

/// Canonical saved split orientation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutAxis {
    /// Zero or one pane; no divider.
    Single,
    /// Two panes left/right.
    SideBySide,
    /// Two panes top/bottom.
    Stacked,
}

/// Sanitized record validation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutRecordError {
    /// Wrong record length.
    InvalidLength,
    /// Unknown magic/version.
    UnsupportedVersion,
    /// Zero or invalid header metadata.
    InvalidMetadata,
    /// Invalid dimensions or geometry.
    InvalidGeometry,
    /// Invalid, conflicting or out-of-scope targets.
    InvalidTargets,
    /// Any other structural violation.
    InvalidRecord,
}

impl fmt::Display for LayoutRecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid saved layout record")
    }
}

impl std::error::Error for LayoutRecordError {}

/// Nonzero historical header fields.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct LayoutMetadata {
    generation: u64,
    workspace: u64,
    snapshot_sequence: u64,
    layout_revision: u64,
    observed_at_unix_ms: u64,
}

impl fmt::Debug for LayoutMetadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LayoutMetadata { <redacted> }")
    }
}

impl LayoutMetadata {
    /// Validate nonzero header fields.
    pub fn new(
        generation: u64,
        workspace: u64,
        snapshot_sequence: u64,
        layout_revision: u64,
        observed_at_unix_ms: u64,
    ) -> Result<Self, LayoutRecordError> {
        if generation == 0
            || workspace == 0
            || snapshot_sequence == 0
            || layout_revision == 0
            || observed_at_unix_ms == 0
        {
            return Err(LayoutRecordError::InvalidMetadata);
        }
        Ok(Self {
            generation,
            workspace,
            snapshot_sequence,
            layout_revision,
            observed_at_unix_ms,
        })
    }
    /// Historical server generation.
    pub const fn generation(&self) -> u64 {
        self.generation
    }
    /// Historical workspace.
    pub const fn workspace(&self) -> u64 {
        self.workspace
    }
    /// Snapshot sequence (orders records).
    pub const fn snapshot_sequence(&self) -> u64 {
        self.snapshot_sequence
    }
    /// Last accepted layout revision.
    pub const fn layout_revision(&self) -> u64 {
        self.layout_revision
    }
    /// Observation wall-clock time; never orders records.
    pub const fn observed_at_unix_ms(&self) -> u64 {
        self.observed_at_unix_ms
    }
}

/// Validated historical layout record.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct SavedLayout {
    metadata: LayoutMetadata,
    rows: u16,
    columns: u16,
    axis: LayoutAxis,
    panes: [Option<TargetRef>; MAX_PANES],
    geometry: [Option<PaneGeometry>; MAX_PANES],
    dividers: [Option<Rect>; MAX_PANES - 1],
}

impl fmt::Debug for SavedLayout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SavedLayout { <redacted> }")
    }
}

impl SavedLayout {
    /// Validate and capture a live snapshot.
    pub fn from_snapshot(
        metadata: LayoutMetadata,
        snapshot: &LayoutSnapshot,
    ) -> Result<Self, LayoutRecordError> {
        if snapshot.revision != metadata.layout_revision {
            return Err(LayoutRecordError::InvalidRecord);
        }

        let count = snapshot.pane_count();
        match count {
            1 => {
                if !(MIN_SAVED_ROWS..=MAX_PANE_ROWS).contains(&snapshot.rows)
                    || !(MIN_SAVED_COLUMNS..=MAX_PANE_COLUMNS).contains(&snapshot.columns)
                {
                    return Err(LayoutRecordError::InvalidGeometry);
                }
                if snapshot.dividers.iter().any(Option::is_some) || snapshot.divider.is_some() {
                    return Err(LayoutRecordError::InvalidGeometry);
                }
                let pane = snapshot.panes[0].ok_or(LayoutRecordError::InvalidRecord)?;
                if snapshot.panes[1..].iter().any(Option::is_some) {
                    return Err(LayoutRecordError::InvalidRecord);
                }
                if pane.rect != root_rect(snapshot.rows, snapshot.columns) {
                    return Err(LayoutRecordError::InvalidGeometry);
                }
                if pane.target.generation != metadata.generation
                    || pane.target.workspace != metadata.workspace
                {
                    return Err(LayoutRecordError::InvalidTargets);
                }
                if pane.target.pane == 0
                    || pane.target.session == 0
                    || pane.target.occupant_revision == 0
                {
                    return Err(LayoutRecordError::InvalidTargets);
                }
                Ok(Self {
                    metadata,
                    rows: snapshot.rows,
                    columns: snapshot.columns,
                    axis: LayoutAxis::Single,
                    panes: [Some(pane.target), None, None, None],
                    geometry: [Some(pane), None, None, None],
                    dividers: [None; MAX_PANES - 1],
                })
            }
            2..=MAX_PANES => {
                if !(MIN_SAVED_ROWS..=MAX_PANE_ROWS).contains(&snapshot.rows)
                    || !(MIN_SAVED_COLUMNS..=MAX_PANE_COLUMNS).contains(&snapshot.columns)
                {
                    return Err(LayoutRecordError::InvalidGeometry);
                }
                let panes: Vec<_> = snapshot.panes.iter().flatten().copied().collect();
                if panes.len() != count {
                    return Err(LayoutRecordError::InvalidGeometry);
                }
                let axis = if count == 2 {
                    let divider = snapshot.divider.ok_or(LayoutRecordError::InvalidGeometry)?;
                    let axis = if divider.columns == 1 && divider.rows == snapshot.rows {
                        LayoutAxis::SideBySide
                    } else if divider.rows == 1 && divider.columns == snapshot.columns {
                        LayoutAxis::Stacked
                    } else {
                        return Err(LayoutRecordError::InvalidGeometry);
                    };
                    let (first, second, expected_divider) =
                        derive_split_rects(snapshot.rows, snapshot.columns, axis)?;
                    if panes[0].rect != first
                        || panes[1].rect != second
                        || divider != expected_divider
                    {
                        return Err(LayoutRecordError::InvalidGeometry);
                    }
                    axis
                } else {
                    if snapshot.dividers.iter().flatten().count() != count - 1 {
                        return Err(LayoutRecordError::InvalidGeometry);
                    }
                    validate_saved_partition(
                        snapshot.rows,
                        snapshot.columns,
                        &snapshot.panes,
                        &snapshot.dividers,
                        count,
                    )?;
                    LayoutAxis::Single
                };

                for (index, pane) in panes.iter().enumerate() {
                    if pane.target.generation != metadata.generation
                        || pane.target.workspace != metadata.workspace
                    {
                        return Err(LayoutRecordError::InvalidTargets);
                    }
                    if pane.target.pane == 0
                        || pane.target.session == 0
                        || pane.target.occupant_revision == 0
                        || panes[..index].iter().any(|other| {
                            other.target.pane == pane.target.pane
                                || other.target.session == pane.target.session
                        })
                    {
                        return Err(LayoutRecordError::InvalidTargets);
                    }
                }

                let targets = std::array::from_fn(|index| panes.get(index).map(|pane| pane.target));
                let geometry = std::array::from_fn(|index| panes.get(index).copied());
                let dividers = if count == 2 {
                    [snapshot.divider, None, None]
                } else {
                    snapshot.dividers
                };
                Ok(Self {
                    metadata,
                    rows: snapshot.rows,
                    columns: snapshot.columns,
                    axis,
                    panes: targets,
                    geometry,
                    dividers,
                })
            }
            _ => Err(LayoutRecordError::InvalidRecord),
        }
    }
    /// Empty tombstone.
    pub fn empty(metadata: LayoutMetadata) -> Self {
        Self {
            metadata,
            rows: 0,
            columns: 0,
            axis: LayoutAxis::Single,
            panes: [None; MAX_PANES],
            geometry: [None; MAX_PANES],
            dividers: [None; MAX_PANES - 1],
        }
    }
    /// Header metadata.
    pub const fn metadata(&self) -> LayoutMetadata {
        self.metadata
    }
    /// Desired root rows.
    pub const fn rows(&self) -> u16 {
        self.rows
    }
    /// Desired root columns.
    pub const fn columns(&self) -> u16 {
        self.columns
    }
    /// Orientation.
    pub const fn axis(&self) -> LayoutAxis {
        self.axis
    }
    /// Number of panes (0..=2).
    pub fn pane_count(&self) -> usize {
        self.panes.iter().filter(|p| p.is_some()).count()
    }
    /// Historical spatial targets.
    pub const fn panes(&self) -> [Option<TargetRef>; MAX_PANES] {
        self.panes
    }
    /// Derived historical display rectangles.
    pub fn geometry(&self) -> [Option<PaneGeometry>; MAX_PANES] {
        self.geometry
    }
    /// Historical split dividers in tree traversal order.
    pub const fn dividers(&self) -> [Option<Rect>; MAX_PANES - 1] {
        self.dividers
    }
}

/// Encode a validated record.
pub fn encode(layout: &SavedLayout) -> [u8; SAVED_LAYOUT_BYTES] {
    let mut buf = [0u8; SAVED_LAYOUT_BYTES];
    buf[0..8].copy_from_slice(b"PLLAY001");
    buf[8..16].copy_from_slice(&layout.metadata.generation.to_be_bytes());
    buf[16..24].copy_from_slice(&layout.metadata.workspace.to_be_bytes());
    buf[24..32].copy_from_slice(&layout.metadata.snapshot_sequence.to_be_bytes());
    buf[32..40].copy_from_slice(&layout.metadata.layout_revision.to_be_bytes());
    buf[40..48].copy_from_slice(&layout.metadata.observed_at_unix_ms.to_be_bytes());
    buf[48..50].copy_from_slice(&layout.rows.to_be_bytes());
    buf[50..52].copy_from_slice(&layout.columns.to_be_bytes());
    buf[52] = layout.pane_count() as u8;
    // Three or more panes have no single axis; decode requires marker 3 for
    // them (byte 0 is reserved for zero/one-pane records).
    buf[53] = if layout.pane_count() > 2 {
        3
    } else {
        axis_byte(layout.axis)
    };
    // bytes 54..64 remain zero
    if let Some(target) = layout.panes[0] {
        write_target(&mut buf[64..104], target);
    }
    if let Some(target) = layout.panes[1] {
        write_target(&mut buf[104..144], target);
    }
    if let Some(target) = layout.panes[2] {
        write_target(&mut buf[144..184], target);
    }
    if let Some(target) = layout.panes[3] {
        write_target(&mut buf[184..224], target);
    }
    for (index, pane) in layout.geometry.iter().enumerate() {
        if let Some(pane) = pane {
            write_rect(&mut buf[224 + index * 8..232 + index * 8], pane.rect);
        }
    }
    for (index, divider) in layout.dividers.iter().enumerate() {
        if let Some(divider) = divider {
            write_rect(&mut buf[256 + index * 8..264 + index * 8], *divider);
        }
    }
    buf
}

/// Decode and fully validate a record.
pub fn decode(bytes: &[u8]) -> Result<SavedLayout, LayoutRecordError> {
    if bytes.len() != SAVED_LAYOUT_BYTES && bytes.len() != LEGACY_SAVED_LAYOUT_BYTES {
        return Err(LayoutRecordError::InvalidLength);
    }
    let legacy = bytes.len() == LEGACY_SAVED_LAYOUT_BYTES;
    if &bytes[0..8] != b"PLLAY001" {
        return Err(LayoutRecordError::UnsupportedVersion);
    }

    let generation = read_u64(bytes, 8);
    let workspace = read_u64(bytes, 16);
    let snapshot_sequence = read_u64(bytes, 24);
    let layout_revision = read_u64(bytes, 32);
    let observed_at_unix_ms = read_u64(bytes, 40);
    if generation == 0
        || workspace == 0
        || snapshot_sequence == 0
        || layout_revision == 0
        || observed_at_unix_ms == 0
    {
        return Err(LayoutRecordError::InvalidMetadata);
    }
    let metadata = LayoutMetadata {
        generation,
        workspace,
        snapshot_sequence,
        layout_revision,
        observed_at_unix_ms,
    };

    let rows = read_u16(bytes, 48);
    let columns = read_u16(bytes, 50);
    let count = bytes[52];
    let axis = bytes[53];

    if bytes[54..64].iter().any(|&b| b != 0) || (legacy && bytes[144..160].iter().any(|&b| b != 0))
    {
        return Err(LayoutRecordError::InvalidRecord);
    }

    let first_target = decode_target(bytes, 64)?;
    let second_target = decode_target(bytes, 104)?;
    let third_target = if legacy {
        None
    } else {
        decode_target(bytes, 144)?
    };
    let fourth_target = if legacy {
        None
    } else {
        decode_target(bytes, 184)?
    };

    match count {
        0 => {
            if rows != 0 || columns != 0 || axis != 0 {
                return Err(LayoutRecordError::InvalidRecord);
            }
            if first_target.is_some()
                || second_target.is_some()
                || third_target.is_some()
                || fourth_target.is_some()
            {
                return Err(LayoutRecordError::InvalidTargets);
            }
            if !legacy && bytes[224..280].iter().any(|&byte| byte != 0) {
                return Err(LayoutRecordError::InvalidRecord);
            }
            Ok(SavedLayout {
                metadata,
                rows: 0,
                columns: 0,
                axis: LayoutAxis::Single,
                panes: [None; MAX_PANES],
                geometry: [None; MAX_PANES],
                dividers: [None; MAX_PANES - 1],
            })
        }
        1 => {
            if axis != 0 {
                return Err(LayoutRecordError::InvalidRecord);
            }
            if !(MIN_SAVED_ROWS..=MAX_PANE_ROWS).contains(&rows)
                || !(MIN_SAVED_COLUMNS..=MAX_PANE_COLUMNS).contains(&columns)
            {
                return Err(LayoutRecordError::InvalidGeometry);
            }
            if axis != 0 {
                return Err(LayoutRecordError::InvalidRecord);
            }
            let target = first_target.ok_or(LayoutRecordError::InvalidTargets)?;
            if second_target.is_some() || third_target.is_some() || fourth_target.is_some() {
                return Err(LayoutRecordError::InvalidTargets);
            }
            if target.generation != metadata.generation || target.workspace != metadata.workspace {
                return Err(LayoutRecordError::InvalidTargets);
            }
            // The current form stores the derived root rect explicitly; it
            // must match exactly and every other geometry byte must be zero,
            // so decode/encode stays canonical.
            if !legacy
                && (read_rect(bytes, 224) != root_rect(rows, columns)
                    || bytes[232..280].iter().any(|&byte| byte != 0))
            {
                return Err(LayoutRecordError::InvalidRecord);
            }
            Ok(SavedLayout {
                metadata,
                rows,
                columns,
                axis: LayoutAxis::Single,
                panes: [Some(target), None, None, None],
                geometry: [
                    Some(PaneGeometry {
                        target,
                        rect: root_rect(rows, columns),
                    }),
                    None,
                    None,
                    None,
                ],
                dividers: [None; MAX_PANES - 1],
            })
        }
        2..=MAX_SAVED_PANES => {
            let axis = match axis {
                1 => LayoutAxis::SideBySide,
                2 => LayoutAxis::Stacked,
                3 => LayoutAxis::Single,
                _ => return Err(LayoutRecordError::InvalidRecord),
            };
            if count == 2 && axis == LayoutAxis::Single {
                return Err(LayoutRecordError::InvalidRecord);
            }
            if !(MIN_SAVED_ROWS..=MAX_PANE_ROWS).contains(&rows)
                || !(MIN_SAVED_COLUMNS..=MAX_PANE_COLUMNS).contains(&columns)
            {
                return Err(LayoutRecordError::InvalidGeometry);
            }
            let targets = [first_target, second_target, third_target, fourth_target];
            if targets.iter().take(usize::from(count)).any(Option::is_none)
                || targets.iter().skip(usize::from(count)).any(Option::is_some)
            {
                return Err(LayoutRecordError::InvalidTargets);
            }
            for (index, target) in targets.iter().flatten().enumerate() {
                if target.generation != metadata.generation
                    || target.workspace != metadata.workspace
                    || targets[..index]
                        .iter()
                        .flatten()
                        .any(|other| other.pane == target.pane || other.session == target.session)
                {
                    return Err(LayoutRecordError::InvalidTargets);
                }
            }
            let mut geometry = [None; MAX_PANES];
            let mut dividers = [None; MAX_PANES - 1];
            if legacy {
                if count != 2 {
                    return Err(LayoutRecordError::InvalidRecord);
                }
                validate_derived_geometry(rows, columns, axis)?;
                let (first_rect, second_rect, divider) = derive_split_rects(rows, columns, axis)?;
                geometry[0] = Some(PaneGeometry {
                    target: targets[0].unwrap(),
                    rect: first_rect,
                });
                geometry[1] = Some(PaneGeometry {
                    target: targets[1].unwrap(),
                    rect: second_rect,
                });
                dividers[0] = Some(divider);
            } else {
                if count > 2 && axis != LayoutAxis::Single {
                    return Err(LayoutRecordError::InvalidRecord);
                }
                if (usize::from(count)..MAX_PANES).any(|index| {
                    bytes[224 + index * 8..232 + index * 8]
                        .iter()
                        .any(|&byte| byte != 0)
                }) || (usize::from(count - 1)..MAX_PANES - 1).any(|index| {
                    bytes[256 + index * 8..264 + index * 8]
                        .iter()
                        .any(|&byte| byte != 0)
                }) {
                    return Err(LayoutRecordError::InvalidRecord);
                }
                for index in 0..usize::from(count) {
                    let offset = 224 + index * 8;
                    let rect = read_rect(bytes, offset);
                    validate_rect(rows, columns, rect)?;
                    geometry[index] = Some(PaneGeometry {
                        target: targets[index].unwrap(),
                        rect,
                    });
                }
                for (index, divider_slot) in
                    dividers.iter_mut().enumerate().take(usize::from(count - 1))
                {
                    let divider = read_rect(bytes, 256 + index * 8);
                    validate_divider(rows, columns, divider)?;
                    *divider_slot = Some(divider);
                }
                validate_saved_partition(rows, columns, &geometry, &dividers, usize::from(count))?;
            }
            Ok(SavedLayout {
                metadata,
                rows,
                columns,
                axis,
                panes: std::array::from_fn(|index| targets[index]),
                geometry,
                dividers,
            })
        }
        _ => Err(LayoutRecordError::InvalidRecord),
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn root_rect(rows: u16, columns: u16) -> Rect {
    Rect {
        x: 0,
        y: 0,
        rows,
        columns,
    }
}

fn axis_byte(axis: LayoutAxis) -> u8 {
    match axis {
        LayoutAxis::Single => 0,
        LayoutAxis::SideBySide => 1,
        LayoutAxis::Stacked => 2,
    }
}

fn write_rect(bytes: &mut [u8], rect: Rect) {
    bytes[0..2].copy_from_slice(&rect.x.to_be_bytes());
    bytes[2..4].copy_from_slice(&rect.y.to_be_bytes());
    bytes[4..6].copy_from_slice(&rect.rows.to_be_bytes());
    bytes[6..8].copy_from_slice(&rect.columns.to_be_bytes());
}

fn read_rect(bytes: &[u8], offset: usize) -> Rect {
    Rect {
        x: read_u16(bytes, offset),
        y: read_u16(bytes, offset + 2),
        rows: read_u16(bytes, offset + 4),
        columns: read_u16(bytes, offset + 6),
    }
}

fn validate_rect(rows: u16, columns: u16, rect: Rect) -> Result<(), LayoutRecordError> {
    if rect.rows < MIN_SAVED_ROWS
        || rect.columns < MIN_SAVED_COLUMNS
        || u32::from(rect.x) + u32::from(rect.columns) > u32::from(columns)
        || u32::from(rect.y) + u32::from(rect.rows) > u32::from(rows)
    {
        return Err(LayoutRecordError::InvalidGeometry);
    }
    Ok(())
}

fn validate_divider(rows: u16, columns: u16, rect: Rect) -> Result<(), LayoutRecordError> {
    if rect.rows == 0
        || rect.columns == 0
        || (rect.rows != 1 && rect.columns != 1)
        || u32::from(rect.x) + u32::from(rect.columns) > u32::from(columns)
        || u32::from(rect.y) + u32::from(rect.rows) > u32::from(rows)
    {
        return Err(LayoutRecordError::InvalidGeometry);
    }
    Ok(())
}

fn validate_saved_partition(
    rows: u16,
    columns: u16,
    panes: &[Option<PaneGeometry>; MAX_PANES],
    dividers: &[Option<Rect>; MAX_PANES - 1],
    count: usize,
) -> Result<(), LayoutRecordError> {
    let mut covered = [false; 80 * 240];
    for pane in panes.iter().take(count).flatten() {
        for y in pane.rect.y..pane.rect.y + pane.rect.rows {
            for x in pane.rect.x..pane.rect.x + pane.rect.columns {
                let index = usize::from(y) * 240 + usize::from(x);
                if std::mem::replace(&mut covered[index], true) {
                    return Err(LayoutRecordError::InvalidGeometry);
                }
            }
        }
    }
    for divider in dividers.iter().take(count - 1).flatten() {
        if divider.rows == 0 || divider.columns == 0 || (divider.rows != 1 && divider.columns != 1)
        {
            return Err(LayoutRecordError::InvalidGeometry);
        }
        for y in divider.y..divider.y + divider.rows {
            for x in divider.x..divider.x + divider.columns {
                let index = usize::from(y) * 240 + usize::from(x);
                if std::mem::replace(&mut covered[index], true) {
                    return Err(LayoutRecordError::InvalidGeometry);
                }
            }
        }
    }
    for y in 0..rows {
        for x in 0..columns {
            if !covered[usize::from(y) * 240 + usize::from(x)] {
                return Err(LayoutRecordError::InvalidGeometry);
            }
        }
    }
    if dividers.iter().take(count - 1).flatten().any(|divider| {
        let wide = divider.columns == 1 && divider.rows == rows;
        let tall = divider.rows == 1 && divider.columns == columns;
        !wide && !tall
    }) {
        // A nested split's divider spans only its parent region. Disjoint full
        // coverage and one-cell thickness validate its bounded footprint.
    }
    Ok(())
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_be_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ])
}

fn write_target(buf: &mut [u8], target: TargetRef) {
    buf[0..8].copy_from_slice(&target.generation.to_be_bytes());
    buf[8..16].copy_from_slice(&target.workspace.to_be_bytes());
    buf[16..24].copy_from_slice(&target.pane.to_be_bytes());
    buf[24..32].copy_from_slice(&target.session.to_be_bytes());
    buf[32..40].copy_from_slice(&target.occupant_revision.to_be_bytes());
}

fn decode_target(bytes: &[u8], offset: usize) -> Result<Option<TargetRef>, LayoutRecordError> {
    let generation = read_u64(bytes, offset);
    let workspace = read_u64(bytes, offset + 8);
    let pane = read_u64(bytes, offset + 16);
    let session = read_u64(bytes, offset + 24);
    let occupant_revision = read_u64(bytes, offset + 32);
    if generation == 0 && workspace == 0 && pane == 0 && session == 0 && occupant_revision == 0 {
        Ok(None)
    } else if generation == 0
        || workspace == 0
        || pane == 0
        || session == 0
        || occupant_revision == 0
    {
        Err(LayoutRecordError::InvalidTargets)
    } else {
        Ok(Some(TargetRef {
            generation,
            workspace,
            pane,
            session,
            occupant_revision,
        }))
    }
}

fn validate_derived_geometry(
    rows: u16,
    columns: u16,
    axis: LayoutAxis,
) -> Result<(), LayoutRecordError> {
    match axis {
        LayoutAxis::Single => {
            if !(MIN_SAVED_ROWS..=MAX_PANE_ROWS).contains(&rows)
                || !(MIN_SAVED_COLUMNS..=MAX_PANE_COLUMNS).contains(&columns)
            {
                return Err(LayoutRecordError::InvalidGeometry);
            }
        }
        LayoutAxis::SideBySide => {
            if columns < 2 * MIN_SAVED_COLUMNS + 1 {
                return Err(LayoutRecordError::InvalidGeometry);
            }
            let usable = columns
                .checked_sub(1)
                .ok_or(LayoutRecordError::InvalidGeometry)?;
            let first = usable / 2;
            let second = usable - first;
            if first < MIN_SAVED_COLUMNS || second < MIN_SAVED_COLUMNS {
                return Err(LayoutRecordError::InvalidGeometry);
            }
            if !(MIN_SAVED_ROWS..=MAX_PANE_ROWS).contains(&rows) {
                return Err(LayoutRecordError::InvalidGeometry);
            }
        }
        LayoutAxis::Stacked => {
            if rows < 2 * MIN_SAVED_ROWS + 1 {
                return Err(LayoutRecordError::InvalidGeometry);
            }
            let usable = rows
                .checked_sub(1)
                .ok_or(LayoutRecordError::InvalidGeometry)?;
            let first = usable / 2;
            let second = usable - first;
            if first < MIN_SAVED_ROWS || second < MIN_SAVED_ROWS {
                return Err(LayoutRecordError::InvalidGeometry);
            }
            if !(MIN_SAVED_COLUMNS..=MAX_PANE_COLUMNS).contains(&columns) {
                return Err(LayoutRecordError::InvalidGeometry);
            }
        }
    }
    Ok(())
}

fn derive_split_rects(
    rows: u16,
    columns: u16,
    axis: LayoutAxis,
) -> Result<(Rect, Rect, Rect), LayoutRecordError> {
    match axis {
        LayoutAxis::SideBySide => {
            if columns < 2 * MIN_SAVED_COLUMNS + 1 {
                return Err(LayoutRecordError::InvalidGeometry);
            }
            let usable = columns
                .checked_sub(1)
                .ok_or(LayoutRecordError::InvalidGeometry)?;
            let first_columns = usable / 2;
            let second_columns = usable - first_columns;
            let divider_x = first_columns;
            let second_x = divider_x
                .checked_add(1)
                .ok_or(LayoutRecordError::InvalidGeometry)?;
            let first = Rect {
                x: 0,
                y: 0,
                rows,
                columns: first_columns,
            };
            let second = Rect {
                x: second_x,
                y: 0,
                rows,
                columns: second_columns,
            };
            let divider = Rect {
                x: divider_x,
                y: 0,
                rows,
                columns: 1,
            };
            validate_leaf_rect(first)?;
            validate_leaf_rect(second)?;
            Ok((first, second, divider))
        }
        LayoutAxis::Stacked => {
            if rows < 2 * MIN_SAVED_ROWS + 1 {
                return Err(LayoutRecordError::InvalidGeometry);
            }
            let usable = rows
                .checked_sub(1)
                .ok_or(LayoutRecordError::InvalidGeometry)?;
            let first_rows = usable / 2;
            let second_rows = usable - first_rows;
            let divider_y = first_rows;
            let second_y = divider_y
                .checked_add(1)
                .ok_or(LayoutRecordError::InvalidGeometry)?;
            let first = Rect {
                x: 0,
                y: 0,
                rows: first_rows,
                columns,
            };
            let second = Rect {
                x: 0,
                y: second_y,
                rows: second_rows,
                columns,
            };
            let divider = Rect {
                x: 0,
                y: divider_y,
                rows: 1,
                columns,
            };
            validate_leaf_rect(first)?;
            validate_leaf_rect(second)?;
            Ok((first, second, divider))
        }
        LayoutAxis::Single => Err(LayoutRecordError::InvalidRecord),
    }
}

fn validate_leaf_rect(rect: Rect) -> Result<(), LayoutRecordError> {
    if rect.rows < MIN_SAVED_ROWS || rect.columns < MIN_SAVED_COLUMNS {
        return Err(LayoutRecordError::InvalidGeometry);
    }
    if rect.rows > MAX_PANE_ROWS || rect.columns > MAX_PANE_COLUMNS {
        return Err(LayoutRecordError::InvalidGeometry);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane_layout::{PaneLayout, SplitDirection};

    fn sample_meta() -> LayoutMetadata {
        LayoutMetadata::new(7, 8, 9, 10, 11).expect("valid test metadata")
    }

    fn target(pane: u64, session: u64) -> TargetRef {
        TargetRef {
            generation: 7,
            workspace: 8,
            pane,
            session,
            occupant_revision: 1,
        }
    }

    #[test]
    fn empty_pane_count_and_geometry() {
        let meta = sample_meta();
        let empty = SavedLayout::empty(meta);
        assert_eq!(empty.pane_count(), 0);
        assert_eq!(empty.geometry(), [None; MAX_PANES]);
        assert_eq!(empty.rows(), 0);
        assert_eq!(empty.columns(), 0);
        assert_eq!(empty.axis(), LayoutAxis::Single);
        assert_eq!(empty.panes(), [None; MAX_PANES]);
    }

    #[test]
    fn empty_roundtrip() {
        let meta = sample_meta();
        let empty = SavedLayout::empty(meta);
        let bytes = encode(&empty);
        assert_eq!(bytes.len(), SAVED_LAYOUT_BYTES);
        let decoded = decode(&bytes).expect("decode empty");
        assert_eq!(decoded, empty);
        assert_eq!(decoded.metadata(), meta);
    }

    #[test]
    fn one_pane_roundtrip() {
        let meta = LayoutMetadata::new(7, 8, 1, 1, 1).unwrap();
        let t = target(1, 1);
        let snapshot = LayoutSnapshot {
            revision: 1,
            rows: 10,
            columns: 30,
            panes: [
                Some(PaneGeometry {
                    target: t,
                    rect: root_rect(10, 30),
                }),
                None,
                None,
                None,
            ],
            divider: None,
            dividers: [None; MAX_PANES - 1],
        };
        let layout = SavedLayout::from_snapshot(meta, &snapshot).expect("from_snapshot one pane");
        assert_eq!(layout.pane_count(), 1);
        let bytes = encode(&layout);
        let decoded = decode(&bytes).expect("decode one pane");
        assert_eq!(decoded, layout);
    }

    #[test]
    fn historical_minimum_snapshot_remains_canonical() {
        let meta = LayoutMetadata::new(7, 8, 1, 1, 1).unwrap();
        let pane = PaneGeometry {
            target: target(1, 1),
            rect: root_rect(MIN_SAVED_ROWS, MIN_SAVED_COLUMNS),
        };
        let snapshot = LayoutSnapshot {
            revision: 1,
            rows: MIN_SAVED_ROWS,
            columns: MIN_SAVED_COLUMNS,
            panes: [Some(pane), None, None, None],
            divider: None,
            dividers: [None; MAX_PANES - 1],
        };

        let saved = SavedLayout::from_snapshot(meta, &snapshot).expect("historical minimum");
        let bytes = encode(&saved);
        assert_eq!(decode(&bytes), Ok(saved));
        assert_eq!(read_u16(&bytes, 48), MIN_SAVED_ROWS);
        assert_eq!(read_u16(&bytes, 50), MIN_SAVED_COLUMNS);
    }

    #[test]
    fn encode_uses_exact_offsets_and_zero_padding() {
        let meta = LayoutMetadata::new(1, 2, 3, 4, 5).unwrap();
        let layout = SavedLayout::empty(meta);
        let bytes = encode(&layout);
        assert_eq!(&bytes[0..8], b"PLLAY001");
        assert_eq!(read_u64(&bytes, 8), 1);
        assert_eq!(read_u64(&bytes, 16), 2);
        assert_eq!(read_u64(&bytes, 24), 3);
        assert_eq!(read_u64(&bytes, 32), 4);
        assert_eq!(read_u64(&bytes, 40), 5);
        assert_eq!(bytes[48..52], [0, 0, 0, 0]);
        assert_eq!(bytes[52], 0); // count
        assert_eq!(bytes[53], 0); // axis
        assert!(bytes[54..64].iter().all(|&b| b == 0));
        assert!(bytes[64..144].iter().all(|&b| b == 0));
        assert!(bytes[144..280].iter().all(|&b| b == 0));
    }

    #[test]
    fn four_directions_from_pane_layout_roundtrip() {
        let cases = [
            (SplitDirection::Left, 7, 83),
            (SplitDirection::Right, 7, 83),
            (SplitDirection::Up, 15, 82),
            (SplitDirection::Down, 16, 82),
        ];
        for &(direction, rows, columns) in &cases {
            let existing = target(1, 1);
            let new = target(2, 2);
            let mut layout = PaneLayout::new(existing, rows, columns).expect("valid root");
            let preview = layout
                .preview_split(existing, 1, direction, new)
                .expect("split preview");
            layout.commit_split(preview).expect("commit");
            let snapshot = layout.snapshot();
            let meta = LayoutMetadata::new(7, 8, 1, snapshot.revision, 100).unwrap();
            let saved = SavedLayout::from_snapshot(meta, &snapshot).expect("valid from_snapshot");
            assert_eq!(saved.pane_count(), 2);
            let bytes = encode(&saved);
            let decoded = decode(&bytes).expect("decode");
            assert_eq!(decoded, saved);
            assert_eq!(decoded.axis(), saved.axis());
            assert_eq!(decoded.rows(), rows);
            assert_eq!(decoded.columns(), columns);
        }
    }

    #[test]
    fn reads_historical_two_pane_record_and_reencodes_current_form() {
        let meta = LayoutMetadata::new(7, 8, 1, 2, 100).unwrap();
        let mut legacy = [0u8; LEGACY_SAVED_LAYOUT_BYTES];
        legacy[0..8].copy_from_slice(b"PLLAY001");
        for (index, field) in [7_u64, 8, 1, 2, 100].into_iter().enumerate() {
            legacy[8 + index * 8..16 + index * 8].copy_from_slice(&field.to_be_bytes());
        }
        legacy[48..50].copy_from_slice(&7_u16.to_be_bytes());
        legacy[50..52].copy_from_slice(&82_u16.to_be_bytes());
        legacy[52] = 2;
        legacy[53] = 1;
        write_target(&mut legacy[64..104], target(1, 1));
        write_target(&mut legacy[104..144], target(2, 2));
        let saved = decode(&legacy).expect("historical two-pane form");
        assert_eq!(saved.metadata(), meta);
        assert_eq!(saved.pane_count(), 2);
        assert_eq!(decode(&encode(&saved)).unwrap(), saved);
    }

    #[test]
    fn odd_and_even_root_dimensions() {
        // SideBySide odd (83) and even (82)
        for columns in [83, 82] {
            let mut layout = PaneLayout::new(target(1, 1), 7, columns).expect("root");
            let preview = layout
                .preview_split(target(1, 1), 1, SplitDirection::Left, target(2, 2))
                .expect("split");
            layout.commit_split(preview).expect("commit");
            let snapshot = layout.snapshot();
            let meta = LayoutMetadata::new(7, 8, 1, snapshot.revision, 1).unwrap();
            SavedLayout::from_snapshot(meta, &snapshot).expect("odd/even side-by-side ok");
        }
        // Stacked odd (15) and even (16)
        for rows in [15, 16] {
            let mut layout = PaneLayout::new(target(1, 1), rows, 82).expect("root");
            let preview = layout
                .preview_split(target(1, 1), 1, SplitDirection::Up, target(2, 2))
                .expect("split");
            layout.commit_split(preview).expect("commit");
            let snapshot = layout.snapshot();
            let meta = LayoutMetadata::new(7, 8, 1, snapshot.revision, 1).unwrap();
            SavedLayout::from_snapshot(meta, &snapshot).expect("odd/even stacked ok");
        }
    }

    #[test]
    fn minimum_and_maximum_roots() {
        // Count1 min/max
        let t = target(1, 1);
        for (rows, columns) in [(7, 22), (80, 240)] {
            let layout = PaneLayout::new(t, rows, columns).expect("root");
            let snapshot = layout.snapshot();
            let meta = LayoutMetadata::new(7, 8, 1, snapshot.revision, 1).unwrap();
            SavedLayout::from_snapshot(meta, &snapshot).expect("count1 min/max ok");
        }

        // Count2 SideBySide min/max
        for columns in [45, 240] {
            let mut layout = PaneLayout::new(target(1, 1), 7, columns).expect("root");
            let preview = layout
                .preview_split(target(1, 1), 1, SplitDirection::Right, target(2, 2))
                .expect("split");
            layout.commit_split(preview).expect("commit");
            let snapshot = layout.snapshot();
            let meta = LayoutMetadata::new(7, 8, 1, snapshot.revision, 1).unwrap();
            SavedLayout::from_snapshot(meta, &snapshot).expect("side-by-side min/max ok");
        }

        // Count2 Stacked min/max
        for rows in [15, 80] {
            let mut layout = PaneLayout::new(target(1, 1), rows, 22).expect("root");
            let preview = layout
                .preview_split(target(1, 1), 1, SplitDirection::Down, target(2, 2))
                .expect("split");
            layout.commit_split(preview).expect("commit");
            let snapshot = layout.snapshot();
            let meta = LayoutMetadata::new(7, 8, 1, snapshot.revision, 1).unwrap();
            SavedLayout::from_snapshot(meta, &snapshot).expect("stacked min/max ok");
        }
    }

    #[test]
    fn decode_rejects_short_records() {
        let valid = encode(&SavedLayout::empty(sample_meta()));
        for len in 0..SAVED_LAYOUT_BYTES {
            if len == LEGACY_SAVED_LAYOUT_BYTES {
                continue;
            }
            assert_eq!(
                decode(&valid[..len]),
                Err(LayoutRecordError::InvalidLength),
                "accepted truncated length {len}"
            );
        }
    }

    #[test]
    fn decode_rejects_oversize_and_trailing() {
        let valid = encode(&SavedLayout::empty(sample_meta()));
        let mut oversize = valid.to_vec();
        oversize.push(0);
        assert_eq!(decode(&oversize), Err(LayoutRecordError::InvalidLength));
    }

    #[test]
    fn decode_rejects_bad_magic() {
        let mut bytes = encode(&SavedLayout::empty(sample_meta()));
        bytes[0] = b'X';
        assert_eq!(decode(&bytes), Err(LayoutRecordError::UnsupportedVersion));
        bytes[0] = b'P';
        bytes[7] = b'2';
        assert_eq!(decode(&bytes), Err(LayoutRecordError::UnsupportedVersion));
    }

    #[test]
    fn decode_rejects_zero_metadata_fields() {
        let base = encode(&SavedLayout::empty(sample_meta()));
        let offsets = [8_usize, 16, 24, 32, 40];
        for offset in offsets {
            let mut bytes = base;
            bytes[offset..offset + 8].fill(0);
            assert_eq!(
                decode(&bytes),
                Err(LayoutRecordError::InvalidMetadata),
                "zero at offset {offset}"
            );
        }
    }

    #[test]
    fn decode_rejects_nonzero_reserved() {
        let base = encode(&SavedLayout::empty(sample_meta()));
        for offset in [54_usize, 63, 248, 279] {
            let mut bytes = base;
            bytes[offset] = 1;
            assert_eq!(
                decode(&bytes),
                Err(LayoutRecordError::InvalidRecord),
                "nonzero reserved at offset {offset}"
            );
        }
    }

    #[test]
    fn decode_rejects_count_axis_mismatch() {
        let meta = sample_meta();
        // count 0 with nonzero axis
        let mut bytes = encode(&SavedLayout::empty(meta));
        bytes[53] = 1;
        assert_eq!(decode(&bytes), Err(LayoutRecordError::InvalidRecord));

        // count 1 with axis side-by-side
        let t = target(1, 1);
        let one_pane = SavedLayout::from_snapshot(
            meta,
            &LayoutSnapshot {
                revision: meta.layout_revision(),
                rows: 10,
                columns: 30,
                panes: [
                    Some(PaneGeometry {
                        target: t,
                        rect: root_rect(10, 30),
                    }),
                    None,
                    None,
                    None,
                ],
                divider: None,
                dividers: [None; MAX_PANES - 1],
            },
        )
        .expect("one pane");
        let mut bytes = encode(&one_pane);
        bytes[53] = 1;
        assert_eq!(decode(&bytes), Err(LayoutRecordError::InvalidRecord));

        // count 2 with axis single
        let mut layout = PaneLayout::new(t, 15, 82).expect("root");
        let preview = layout
            .preview_split(t, 1, SplitDirection::Down, target(2, 2))
            .expect("split");
        layout.commit_split(preview).expect("commit");
        let snapshot = layout.snapshot();
        let two_pane = SavedLayout::from_snapshot(
            LayoutMetadata::new(7, 8, 1, snapshot.revision, 1).unwrap(),
            &snapshot,
        )
        .expect("two pane");
        let mut bytes = encode(&two_pane);
        bytes[53] = 0;
        assert_eq!(decode(&bytes), Err(LayoutRecordError::InvalidRecord));

        // count 2 with axis 3
        let mut bytes = encode(&two_pane);
        bytes[53] = 3;
        assert_eq!(decode(&bytes), Err(LayoutRecordError::InvalidRecord));
    }

    #[test]
    fn decode_rejects_hole_in_pane_array() {
        // count 2 but first target all-zero, second nonzero -> hole
        let mut bytes = [0u8; SAVED_LAYOUT_BYTES];
        bytes[0..8].copy_from_slice(b"PLLAY001");
        bytes[8..16].copy_from_slice(&7_u64.to_be_bytes());
        bytes[16..24].copy_from_slice(&8_u64.to_be_bytes());
        bytes[24..32].copy_from_slice(&9_u64.to_be_bytes());
        bytes[32..40].copy_from_slice(&10_u64.to_be_bytes());
        bytes[40..48].copy_from_slice(&11_u64.to_be_bytes());
        bytes[48..50].copy_from_slice(&11_u16.to_be_bytes());
        bytes[50..52].copy_from_slice(&82_u16.to_be_bytes());
        bytes[52] = 2;
        bytes[53] = 2; // stacked
                       // second target nonzero
        bytes[104..112].copy_from_slice(&7_u64.to_be_bytes());
        bytes[112..120].copy_from_slice(&8_u64.to_be_bytes());
        bytes[120..128].copy_from_slice(&2_u64.to_be_bytes());
        bytes[128..136].copy_from_slice(&2_u64.to_be_bytes());
        bytes[136..144].copy_from_slice(&1_u64.to_be_bytes());
        assert_eq!(decode(&bytes), Err(LayoutRecordError::InvalidTargets));
    }

    #[test]
    fn decode_rejects_nonzero_unused_slot() {
        let meta = sample_meta();
        let t = target(1, 1);
        let one_pane = SavedLayout::from_snapshot(
            meta,
            &LayoutSnapshot {
                revision: meta.layout_revision(),
                rows: 10,
                columns: 30,
                panes: [
                    Some(PaneGeometry {
                        target: t,
                        rect: root_rect(10, 30),
                    }),
                    None,
                    None,
                    None,
                ],
                divider: None,
                dividers: [None; MAX_PANES - 1],
            },
        )
        .expect("one pane");
        let mut bytes = encode(&one_pane);
        // set second target nonzero
        bytes[104..112].copy_from_slice(&7_u64.to_be_bytes());
        bytes[112..120].copy_from_slice(&8_u64.to_be_bytes());
        bytes[120..128].copy_from_slice(&2_u64.to_be_bytes());
        bytes[128..136].copy_from_slice(&2_u64.to_be_bytes());
        bytes[136..144].copy_from_slice(&1_u64.to_be_bytes());
        assert_eq!(decode(&bytes), Err(LayoutRecordError::InvalidTargets));
    }

    #[test]
    fn decode_rejects_target_scope_mismatch() {
        let mut bytes = [0u8; SAVED_LAYOUT_BYTES];
        bytes[0..8].copy_from_slice(b"PLLAY001");
        bytes[8..16].copy_from_slice(&7_u64.to_be_bytes());
        bytes[16..24].copy_from_slice(&8_u64.to_be_bytes());
        bytes[24..32].copy_from_slice(&9_u64.to_be_bytes());
        bytes[32..40].copy_from_slice(&10_u64.to_be_bytes());
        bytes[40..48].copy_from_slice(&11_u64.to_be_bytes());
        bytes[48..50].copy_from_slice(&11_u16.to_be_bytes());
        bytes[50..52].copy_from_slice(&82_u16.to_be_bytes());
        bytes[52] = 2;
        bytes[53] = 2;
        // first target with wrong generation
        bytes[64..72].copy_from_slice(&99_u64.to_be_bytes());
        bytes[72..80].copy_from_slice(&8_u64.to_be_bytes());
        bytes[80..88].copy_from_slice(&1_u64.to_be_bytes());
        bytes[88..96].copy_from_slice(&1_u64.to_be_bytes());
        bytes[96..104].copy_from_slice(&1_u64.to_be_bytes());
        // second target ok
        bytes[104..112].copy_from_slice(&7_u64.to_be_bytes());
        bytes[112..120].copy_from_slice(&8_u64.to_be_bytes());
        bytes[120..128].copy_from_slice(&2_u64.to_be_bytes());
        bytes[128..136].copy_from_slice(&2_u64.to_be_bytes());
        bytes[136..144].copy_from_slice(&1_u64.to_be_bytes());
        assert_eq!(decode(&bytes), Err(LayoutRecordError::InvalidTargets));
    }

    #[test]
    fn decode_rejects_one_pane_target_scope_mismatch() {
        // Parent review regression: a single pane must share header scope too.
        for (offset, value) in [(64, 99_u64), (72, 98_u64)] {
            let mut bytes = [0u8; SAVED_LAYOUT_BYTES];
            bytes[0..8].copy_from_slice(b"PLLAY001");
            for (index, field) in [7_u64, 8, 9, 10, 11].iter().enumerate() {
                bytes[8 + index * 8..16 + index * 8].copy_from_slice(&field.to_be_bytes());
            }
            bytes[48..50].copy_from_slice(&24_u16.to_be_bytes());
            bytes[50..52].copy_from_slice(&80_u16.to_be_bytes());
            bytes[52] = 1;
            for (index, field) in [7_u64, 8, 1, 1, 1].iter().enumerate() {
                bytes[64 + index * 8..72 + index * 8].copy_from_slice(&field.to_be_bytes());
            }
            // Current form: the single pane's root rect is stored explicitly.
            bytes[228..230].copy_from_slice(&24_u16.to_be_bytes());
            bytes[230..232].copy_from_slice(&80_u16.to_be_bytes());
            assert!(decode(&bytes).is_ok());
            bytes[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
            assert_eq!(decode(&bytes), Err(LayoutRecordError::InvalidTargets));
        }
    }

    #[test]
    fn from_snapshot_rejects_oversized_two_pane_root() {
        // Leaves fit, but the root exceeds the 80x240 bound.
        let metadata = LayoutMetadata::new(7, 8, 1, 1, 1).unwrap();
        let snapshot = LayoutSnapshot {
            revision: 1,
            rows: 81,
            columns: 80,
            panes: [
                Some(PaneGeometry {
                    target: target(1, 1),
                    rect: Rect {
                        x: 0,
                        y: 0,
                        rows: 40,
                        columns: 80,
                    },
                }),
                Some(PaneGeometry {
                    target: target(2, 2),
                    rect: Rect {
                        x: 0,
                        y: 41,
                        rows: 40,
                        columns: 80,
                    },
                }),
                None,
                None,
            ],
            divider: Some(Rect {
                x: 0,
                y: 40,
                rows: 1,
                columns: 80,
            }),
            dividers: [
                Some(Rect {
                    x: 0,
                    y: 40,
                    rows: 1,
                    columns: 80,
                }),
                None,
                None,
            ],
        };
        assert_eq!(
            SavedLayout::from_snapshot(metadata, &snapshot),
            Err(LayoutRecordError::InvalidGeometry)
        );
    }

    #[test]
    fn decode_rejects_duplicate_pane_or_session() {
        let mut bytes = [0u8; SAVED_LAYOUT_BYTES];
        bytes[0..8].copy_from_slice(b"PLLAY001");
        bytes[8..16].copy_from_slice(&7_u64.to_be_bytes());
        bytes[16..24].copy_from_slice(&8_u64.to_be_bytes());
        bytes[24..32].copy_from_slice(&9_u64.to_be_bytes());
        bytes[32..40].copy_from_slice(&10_u64.to_be_bytes());
        bytes[40..48].copy_from_slice(&11_u64.to_be_bytes());
        bytes[48..50].copy_from_slice(&11_u16.to_be_bytes());
        bytes[50..52].copy_from_slice(&82_u16.to_be_bytes());
        bytes[52] = 2;
        bytes[53] = 2;
        // first and second share pane id
        for offset in [64_usize, 104] {
            bytes[offset..offset + 8].copy_from_slice(&7_u64.to_be_bytes());
            bytes[offset + 8..offset + 16].copy_from_slice(&8_u64.to_be_bytes());
            bytes[offset + 16..offset + 24].copy_from_slice(&1_u64.to_be_bytes());
            bytes[offset + 24..offset + 32].copy_from_slice(&2_u64.to_be_bytes());
            bytes[offset + 32..offset + 40].copy_from_slice(&1_u64.to_be_bytes());
        }
        // duplicate pane
        assert_eq!(decode(&bytes), Err(LayoutRecordError::InvalidTargets));
        // duplicate session
        bytes[80..88].copy_from_slice(&3_u64.to_be_bytes());
        bytes[120..128].copy_from_slice(&3_u64.to_be_bytes());
        assert_eq!(decode(&bytes), Err(LayoutRecordError::InvalidTargets));
    }

    #[test]
    fn decode_rejects_zero_target_field() {
        let mut bytes = [0u8; SAVED_LAYOUT_BYTES];
        bytes[0..8].copy_from_slice(b"PLLAY001");
        bytes[8..16].copy_from_slice(&7_u64.to_be_bytes());
        bytes[16..24].copy_from_slice(&8_u64.to_be_bytes());
        bytes[24..32].copy_from_slice(&9_u64.to_be_bytes());
        bytes[32..40].copy_from_slice(&10_u64.to_be_bytes());
        bytes[40..48].copy_from_slice(&11_u64.to_be_bytes());
        bytes[48..50].copy_from_slice(&5_u16.to_be_bytes());
        bytes[50..52].copy_from_slice(&20_u16.to_be_bytes());
        bytes[52] = 1;
        bytes[53] = 0;
        // first target with pane=0
        bytes[64..72].copy_from_slice(&7_u64.to_be_bytes());
        bytes[72..80].copy_from_slice(&8_u64.to_be_bytes());
        bytes[80..88].copy_from_slice(&0_u64.to_be_bytes());
        bytes[88..96].copy_from_slice(&1_u64.to_be_bytes());
        bytes[96..104].copy_from_slice(&1_u64.to_be_bytes());
        assert_eq!(decode(&bytes), Err(LayoutRecordError::InvalidTargets));
    }

    #[test]
    fn decode_rejects_too_small_root() {
        for (rows, columns, count, axis) in [
            (4u16, 20u16, 1, 0),
            (5u16, 19u16, 1, 0),
            (5u16, 40u16, 2, 1),  // side-by-side too narrow
            (10u16, 20u16, 2, 2), // stacked too short
        ] {
            let mut bytes = [0u8; SAVED_LAYOUT_BYTES];
            bytes[0..8].copy_from_slice(b"PLLAY001");
            bytes[8..16].copy_from_slice(&7_u64.to_be_bytes());
            bytes[16..24].copy_from_slice(&8_u64.to_be_bytes());
            bytes[24..32].copy_from_slice(&9_u64.to_be_bytes());
            bytes[32..40].copy_from_slice(&10_u64.to_be_bytes());
            bytes[40..48].copy_from_slice(&11_u64.to_be_bytes());
            bytes[48..50].copy_from_slice(&rows.to_be_bytes());
            bytes[50..52].copy_from_slice(&columns.to_be_bytes());
            bytes[52] = count;
            bytes[53] = axis;
            // fill valid targets when needed
            if count >= 1 {
                bytes[64..72].copy_from_slice(&7_u64.to_be_bytes());
                bytes[72..80].copy_from_slice(&8_u64.to_be_bytes());
                bytes[80..88].copy_from_slice(&1_u64.to_be_bytes());
                bytes[88..96].copy_from_slice(&1_u64.to_be_bytes());
                bytes[96..104].copy_from_slice(&1_u64.to_be_bytes());
            }
            if count == 2 {
                bytes[104..112].copy_from_slice(&7_u64.to_be_bytes());
                bytes[112..120].copy_from_slice(&8_u64.to_be_bytes());
                bytes[120..128].copy_from_slice(&2_u64.to_be_bytes());
                bytes[128..136].copy_from_slice(&2_u64.to_be_bytes());
                bytes[136..144].copy_from_slice(&1_u64.to_be_bytes());
            }
            assert_eq!(
                decode(&bytes),
                Err(LayoutRecordError::InvalidGeometry),
                "rows={rows}, columns={columns}, count={count}, axis={axis}"
            );
        }
    }

    #[test]
    fn decode_rejects_too_large_root() {
        for (rows, columns, count, axis) in [
            (81u16, 240u16, 1, 0),
            (80u16, 241u16, 1, 0),
            (81u16, 240u16, 2, 1),
            (80u16, 241u16, 2, 2),
        ] {
            let mut bytes = [0u8; SAVED_LAYOUT_BYTES];
            bytes[0..8].copy_from_slice(b"PLLAY001");
            bytes[8..16].copy_from_slice(&7_u64.to_be_bytes());
            bytes[16..24].copy_from_slice(&8_u64.to_be_bytes());
            bytes[24..32].copy_from_slice(&9_u64.to_be_bytes());
            bytes[32..40].copy_from_slice(&10_u64.to_be_bytes());
            bytes[40..48].copy_from_slice(&11_u64.to_be_bytes());
            bytes[48..50].copy_from_slice(&rows.to_be_bytes());
            bytes[50..52].copy_from_slice(&columns.to_be_bytes());
            bytes[52] = count;
            bytes[53] = axis;
            if count >= 1 {
                bytes[64..72].copy_from_slice(&7_u64.to_be_bytes());
                bytes[72..80].copy_from_slice(&8_u64.to_be_bytes());
                bytes[80..88].copy_from_slice(&1_u64.to_be_bytes());
                bytes[88..96].copy_from_slice(&1_u64.to_be_bytes());
                bytes[96..104].copy_from_slice(&1_u64.to_be_bytes());
            }
            if count == 2 {
                bytes[104..112].copy_from_slice(&7_u64.to_be_bytes());
                bytes[112..120].copy_from_slice(&8_u64.to_be_bytes());
                bytes[120..128].copy_from_slice(&2_u64.to_be_bytes());
                bytes[128..136].copy_from_slice(&2_u64.to_be_bytes());
                bytes[136..144].copy_from_slice(&1_u64.to_be_bytes());
            }
            assert_eq!(
                decode(&bytes),
                Err(LayoutRecordError::InvalidGeometry),
                "rows={rows}, columns={columns}"
            );
        }
    }

    #[test]
    fn from_snapshot_rejects_inconsistent_revision() {
        let meta = LayoutMetadata::new(1, 1, 1, 5, 1).unwrap();
        let snapshot = LayoutSnapshot {
            revision: 4,
            rows: 10,
            columns: 30,
            panes: [
                Some(PaneGeometry {
                    target: target(1, 1),
                    rect: root_rect(10, 30),
                }),
                None,
                None,
                None,
            ],
            divider: None,
            dividers: [None; MAX_PANES - 1],
        };
        assert_eq!(
            SavedLayout::from_snapshot(meta, &snapshot),
            Err(LayoutRecordError::InvalidRecord)
        );
    }

    #[test]
    fn from_snapshot_rejects_wrong_rect_for_one_pane() {
        let meta = LayoutMetadata::new(1, 1, 1, 1, 1).unwrap();
        let snapshot = LayoutSnapshot {
            revision: 1,
            rows: 10,
            columns: 30,
            panes: [
                Some(PaneGeometry {
                    target: target(1, 1),
                    rect: Rect {
                        x: 1,
                        y: 0,
                        rows: 10,
                        columns: 30,
                    },
                }),
                None,
                None,
                None,
            ],
            divider: None,
            dividers: [None; MAX_PANES - 1],
        };
        assert_eq!(
            SavedLayout::from_snapshot(meta, &snapshot),
            Err(LayoutRecordError::InvalidGeometry)
        );
    }

    #[test]
    fn from_snapshot_rejects_wrong_divider_for_two_panes() {
        let mut layout = PaneLayout::new(target(1, 1), 15, 82).expect("root");
        let preview = layout
            .preview_split(target(1, 1), 1, SplitDirection::Down, target(2, 2))
            .expect("split");
        layout.commit_split(preview).expect("commit");
        let mut snapshot = layout.snapshot();
        snapshot.divider = Some(Rect {
            x: 0,
            y: 0,
            rows: 1,
            columns: 82,
        }); // wrong y
        let meta = LayoutMetadata::new(7, 8, 1, snapshot.revision, 1).unwrap();
        assert_eq!(
            SavedLayout::from_snapshot(meta, &snapshot),
            Err(LayoutRecordError::InvalidGeometry)
        );
    }

    #[test]
    fn from_snapshot_rejects_hole_array_or_count_mismatch() {
        // LayoutSnapshot with pane_count 1 but a divider present; geometry mismatch
        let meta = LayoutMetadata::new(1, 1, 1, 1, 1).unwrap();
        let snapshot = LayoutSnapshot {
            revision: 1,
            rows: 11,
            columns: 82,
            panes: [
                None,
                Some(PaneGeometry {
                    target: target(1, 1),
                    rect: Rect {
                        x: 0,
                        y: 0,
                        rows: 5,
                        columns: 82,
                    },
                }),
                None,
                None,
            ],
            divider: Some(Rect {
                x: 0,
                y: 5,
                rows: 1,
                columns: 82,
            }),
            dividers: [
                Some(Rect {
                    x: 0,
                    y: 5,
                    rows: 1,
                    columns: 82,
                }),
                None,
                None,
            ],
        };
        assert_eq!(
            SavedLayout::from_snapshot(meta, &snapshot),
            Err(LayoutRecordError::InvalidGeometry)
        );
    }

    #[test]
    fn from_snapshot_rejects_malformed_snapshot_not_normalized() {
        // A snapshot that claims 2 panes but has a single full-size rect (not canonical split)
        let meta = LayoutMetadata::new(1, 1, 1, 1, 1).unwrap();
        let snapshot = LayoutSnapshot {
            revision: 1,
            rows: 11,
            columns: 82,
            panes: [
                Some(PaneGeometry {
                    target: target(1, 1),
                    rect: root_rect(11, 82),
                }),
                Some(PaneGeometry {
                    target: target(2, 2),
                    rect: root_rect(11, 82),
                }),
                None,
                None,
            ],
            divider: None,
            dividers: [None; MAX_PANES - 1],
        };
        assert_eq!(
            SavedLayout::from_snapshot(meta, &snapshot),
            Err(LayoutRecordError::InvalidGeometry)
        );
    }

    #[test]
    fn time_may_decrease_independently_of_sequence() {
        let meta_early = LayoutMetadata::new(1, 1, 1, 1, 100).unwrap();
        let meta_late = LayoutMetadata::new(1, 1, 2, 1, 50).unwrap();
        let empty1 = SavedLayout::empty(meta_early);
        let empty2 = SavedLayout::empty(meta_late);
        let bytes1 = encode(&empty1);
        let bytes2 = encode(&empty2);
        let decoded1 = decode(&bytes1).unwrap();
        let decoded2 = decode(&bytes2).unwrap();
        assert_eq!(decoded1.metadata().observed_at_unix_ms(), 100);
        assert_eq!(decoded2.metadata().observed_at_unix_ms(), 50);
        assert_eq!(decoded2.metadata().snapshot_sequence(), 2);
    }

    #[test]
    fn no_wrap_with_extreme_u64_values() {
        let meta = LayoutMetadata::new(u64::MAX, u64::MAX, u64::MAX, u64::MAX, u64::MAX).unwrap();
        let empty = SavedLayout::empty(meta);
        let bytes = encode(&empty);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.metadata(), meta);
    }

    #[test]
    fn debug_and_error_are_redacted_and_sanitized() {
        let layout = SavedLayout::empty(sample_meta());
        let debug = format!("{:?}", layout);
        assert!(
            debug.contains("redacted") || debug.contains("SavedLayout"),
            "unexpected debug: {debug}"
        );
        for marker in ["7", "8", "9", "10", "11"] {
            assert!(
                !debug.contains(marker),
                "debug leaked value {marker} in {debug}"
            );
        }

        let meta_debug = format!("{:?}", sample_meta());
        assert!(
            meta_debug.contains("redacted"),
            "unexpected meta debug: {meta_debug}"
        );

        let err = LayoutRecordError::InvalidMetadata;
        let display = format!("{err}");
        assert_eq!(display, "invalid saved layout record");
        let err_debug = format!("{err:?}");
        assert!(err_debug.contains("InvalidMetadata"));
    }

    #[test]
    fn geometry_derives_correct_rects_for_count2() {
        let mut layout = PaneLayout::new(target(1, 1), 15, 82).expect("root");
        let preview = layout
            .preview_split(target(1, 1), 1, SplitDirection::Down, target(2, 2))
            .expect("split");
        layout.commit_split(preview).expect("commit");
        let snapshot = layout.snapshot();
        let meta = LayoutMetadata::new(7, 8, 1, snapshot.revision, 1).unwrap();
        let saved = SavedLayout::from_snapshot(meta, &snapshot).expect("valid");
        let geom = saved.geometry();
        assert_eq!(
            geom[0].unwrap().rect,
            Rect {
                x: 0,
                y: 0,
                rows: 7,
                columns: 82
            }
        );
        assert_eq!(
            geom[1].unwrap().rect,
            Rect {
                x: 0,
                y: 8,
                rows: 7,
                columns: 82
            }
        );
    }

    #[test]
    fn geometry_matches_snapshot_for_count1() {
        let t = target(1, 1);
        let snapshot = LayoutSnapshot {
            revision: 1,
            rows: 10,
            columns: 30,
            panes: [
                Some(PaneGeometry {
                    target: t,
                    rect: root_rect(10, 30),
                }),
                None,
                None,
                None,
            ],
            divider: None,
            dividers: [None; MAX_PANES - 1],
        };
        let meta = LayoutMetadata::new(7, 8, 1, 1, 1).unwrap();
        let saved = SavedLayout::from_snapshot(meta, &snapshot).unwrap();
        let geom = saved.geometry();
        assert_eq!(geom[0].unwrap().rect, root_rect(10, 30));
        assert!(geom[1].is_none());
        assert_eq!(geom[0].unwrap().target, t);
    }

    #[test]
    fn decode_counts_and_axis_byte_exactly() {
        let one_pane = SavedLayout::from_snapshot(
            sample_meta(),
            &LayoutSnapshot {
                revision: sample_meta().layout_revision(),
                rows: 10,
                columns: 30,
                panes: [
                    Some(PaneGeometry {
                        target: target(1, 1),
                        rect: root_rect(10, 30),
                    }),
                    None,
                    None,
                    None,
                ],
                divider: None,
                dividers: [None; MAX_PANES - 1],
            },
        )
        .unwrap();
        let bytes = encode(&one_pane);
        assert_eq!(bytes[52], 1);
        assert_eq!(bytes[53], 0);
    }
}
