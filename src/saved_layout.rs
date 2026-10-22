//! Pure fixed-size codec for historical pane layout snapshots.
//!
//! Values in this module describe saved geometry and identity only. They do
//! not construct live panes or authorize dispatch to a historical target.

use crate::control_protocol::TargetRef;
use crate::pane_layout::{
    LayoutSnapshot, PaneGeometry, Rect, MAX_PANE_COLUMNS, MAX_PANE_ROWS, MIN_PANE_COLUMNS,
    MIN_PANE_ROWS,
};
use std::error::Error;
use std::fmt;

/// Exact byte length of one version-one saved layout record.
pub const SAVED_LAYOUT_BYTES: usize = 160;
const MAGIC: &[u8; 8] = b"PLLAY001";

/// Historical orientation inferred from the saved spatial geometry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutAxis {
    /// One pane or an empty workspace, with no divider.
    Single,
    /// Two panes arranged left to right.
    SideBySide,
    /// Two panes arranged top to bottom.
    Stacked,
}

/// Sanitized errors returned by the fixed saved-layout codec.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutRecordError {
    /// The input length is not exactly one record.
    InvalidLength,
    /// The record uses an unsupported format version.
    UnsupportedVersion,
    /// A required metadata field is zero or inconsistent.
    InvalidMetadata,
    /// Dimensions, rectangles, or divider are invalid.
    InvalidGeometry,
    /// Pane target identities are invalid or inconsistent.
    InvalidTargets,
    /// The record contains an invalid tag, reserved byte, or shape.
    InvalidRecord,
}

impl fmt::Display for LayoutRecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidLength => "invalid saved layout length",
            Self::UnsupportedVersion => "unsupported saved layout version",
            Self::InvalidMetadata => "invalid saved layout metadata",
            Self::InvalidGeometry => "invalid saved layout geometry",
            Self::InvalidTargets => "invalid saved layout targets",
            Self::InvalidRecord => "invalid saved layout record",
        })
    }
}

impl Error for LayoutRecordError {}

/// Scope and observation metadata supplied explicitly by the owner.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct LayoutMetadata {
    generation: u64,
    workspace: u64,
    snapshot_sequence: u64,
    layout_revision: u64,
    observed_at_unix_ms: u64,
}

impl LayoutMetadata {
    /// Constructs metadata, rejecting any zero field.
    pub fn new(
        generation: u64,
        workspace: u64,
        snapshot_sequence: u64,
        layout_revision: u64,
        observed_at_unix_ms: u64,
    ) -> Result<Self, LayoutRecordError> {
        if [
            generation,
            workspace,
            snapshot_sequence,
            layout_revision,
            observed_at_unix_ms,
        ]
        .contains(&0)
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

    /// Returns the historical server generation.
    pub const fn generation(self) -> u64 {
        self.generation
    }
    /// Returns the historical workspace identity.
    pub const fn workspace(self) -> u64 {
        self.workspace
    }
    /// Returns the owner observation sequence.
    pub const fn snapshot_sequence(self) -> u64 {
        self.snapshot_sequence
    }
    /// Returns the accepted pane layout revision.
    pub const fn layout_revision(self) -> u64 {
        self.layout_revision
    }
    /// Returns the explicitly supplied observation time.
    pub const fn observed_at_unix_ms(self) -> u64 {
        self.observed_at_unix_ms
    }
}

impl fmt::Debug for LayoutMetadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LayoutMetadata")
            .field("generation", &"[redacted]")
            .field("workspace", &"[redacted]")
            .field("snapshot_sequence", &"[redacted]")
            .field("layout_revision", &"[redacted]")
            .field("observed_at_unix_ms", &"[redacted]")
            .finish()
    }
}

/// A validated historical topology with fixed capacity for two targets.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct SavedLayout {
    metadata: LayoutMetadata,
    rows: u16,
    columns: u16,
    axis: LayoutAxis,
    panes: [Option<TargetRef>; 2],
}

impl SavedLayout {
    /// Validates and copies a complete current layout snapshot.
    pub fn from_snapshot(
        metadata: LayoutMetadata,
        snapshot: &LayoutSnapshot,
    ) -> Result<Self, LayoutRecordError> {
        if snapshot.revision != metadata.layout_revision {
            return Err(LayoutRecordError::InvalidMetadata);
        }
        let count = snapshot.panes.iter().filter(|pane| pane.is_some()).count();
        match count {
            1 => {
                let Some(first) = snapshot.panes[0] else {
                    return Err(LayoutRecordError::InvalidGeometry);
                };
                if snapshot.panes[1].is_some()
                    || snapshot.divider.is_some()
                    || !valid_root(snapshot.rows, snapshot.columns)
                    || first.rect != root(snapshot.rows, snapshot.columns)
                {
                    return Err(LayoutRecordError::InvalidGeometry);
                }
                validate_target(first.target, metadata)?;
                Ok(Self {
                    metadata,
                    rows: snapshot.rows,
                    columns: snapshot.columns,
                    axis: LayoutAxis::Single,
                    panes: [Some(first.target), None],
                })
            }
            2 => {
                let [Some(first), Some(second)] = snapshot.panes else {
                    return Err(LayoutRecordError::InvalidGeometry);
                };
                let Some(axis) = infer_axis(
                    snapshot.rows,
                    snapshot.columns,
                    first.rect,
                    second.rect,
                    snapshot.divider,
                )?
                else {
                    return Err(LayoutRecordError::InvalidGeometry);
                };
                validate_target(first.target, metadata)?;
                validate_target(second.target, metadata)?;
                if first.target.pane == second.target.pane
                    || first.target.session == second.target.session
                {
                    return Err(LayoutRecordError::InvalidTargets);
                }
                Ok(Self {
                    metadata,
                    rows: snapshot.rows,
                    columns: snapshot.columns,
                    axis,
                    panes: [Some(first.target), Some(second.target)],
                })
            }
            _ => Err(LayoutRecordError::InvalidGeometry),
        }
    }

    /// Constructs the canonical empty-workspace tombstone.
    pub const fn empty(metadata: LayoutMetadata) -> Self {
        Self {
            metadata,
            rows: 0,
            columns: 0,
            axis: LayoutAxis::Single,
            panes: [None, None],
        }
    }

    /// Returns the record metadata.
    pub const fn metadata(self) -> LayoutMetadata {
        self.metadata
    }
    /// Returns the root height, or zero for an empty tombstone.
    pub const fn rows(self) -> u16 {
        self.rows
    }
    /// Returns the root width, or zero for an empty tombstone.
    pub const fn columns(self) -> u16 {
        self.columns
    }
    /// Returns the historical spatial axis.
    pub const fn axis(self) -> LayoutAxis {
        self.axis
    }
    /// Returns the number of saved panes.
    pub fn pane_count(self) -> usize {
        self.panes.iter().filter(|pane| pane.is_some()).count()
    }
    /// Returns the historical pane target references in spatial order.
    pub const fn panes(self) -> [Option<TargetRef>; 2] {
        self.panes
    }

    /// Derives fixed-capacity historical display rectangles.
    pub fn geometry(self) -> [Option<PaneGeometry>; 2] {
        let targets = self.panes;
        let rects = match self.axis {
            LayoutAxis::Single if targets[0].is_some() => {
                [Some(root(self.rows, self.columns)), None]
            }
            LayoutAxis::Single => [None, None],
            LayoutAxis::SideBySide => {
                let left = (self.columns - 1) / 2;
                let right_start = left + 1;
                [
                    Some(Rect {
                        x: 0,
                        y: 0,
                        rows: self.rows,
                        columns: left,
                    }),
                    Some(Rect {
                        x: right_start,
                        y: 0,
                        rows: self.rows,
                        columns: self.columns - right_start,
                    }),
                ]
            }
            LayoutAxis::Stacked => {
                let top = (self.rows - 1) / 2;
                let bottom_start = top + 1;
                [
                    Some(Rect {
                        x: 0,
                        y: 0,
                        rows: top,
                        columns: self.columns,
                    }),
                    Some(Rect {
                        x: 0,
                        y: bottom_start,
                        rows: self.rows - bottom_start,
                        columns: self.columns,
                    }),
                ]
            }
        };
        [
            geometry(targets[0], rects[0]),
            geometry(targets[1], rects[1]),
        ]
    }
}

impl fmt::Debug for SavedLayout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SavedLayout")
            .field("metadata", &self.metadata)
            .field("rows", &self.rows)
            .field("columns", &self.columns)
            .field("axis", &self.axis)
            .field("pane_count", &self.pane_count())
            .field("panes", &"[redacted]")
            .finish()
    }
}

/// Encodes a validated value into its canonical 160-byte representation.
pub fn encode(layout: &SavedLayout) -> [u8; SAVED_LAYOUT_BYTES] {
    let mut bytes = [0; SAVED_LAYOUT_BYTES];
    bytes[0..8].copy_from_slice(MAGIC);
    put_u64(&mut bytes, 8, layout.metadata.generation);
    put_u64(&mut bytes, 16, layout.metadata.workspace);
    put_u64(&mut bytes, 24, layout.metadata.snapshot_sequence);
    put_u64(&mut bytes, 32, layout.metadata.layout_revision);
    put_u64(&mut bytes, 40, layout.metadata.observed_at_unix_ms);
    bytes[48..50].copy_from_slice(&layout.rows.to_be_bytes());
    bytes[50..52].copy_from_slice(&layout.columns.to_be_bytes());
    bytes[52] = layout.pane_count() as u8;
    bytes[53] = match layout.axis {
        LayoutAxis::Single => 0,
        LayoutAxis::SideBySide => 1,
        LayoutAxis::Stacked => 2,
    };
    for (index, target) in layout.panes.into_iter().enumerate() {
        if let Some(target) = target {
            put_target(&mut bytes, 64 + index * 40, target);
        }
    }
    bytes
}

/// Decodes one exact, canonical version-one record.
pub fn decode(bytes: &[u8]) -> Result<SavedLayout, LayoutRecordError> {
    if bytes.len() != SAVED_LAYOUT_BYTES {
        return Err(LayoutRecordError::InvalidLength);
    }
    if bytes[0..8] != *MAGIC {
        return if &bytes[0..5] == b"PLLAY" {
            Err(LayoutRecordError::UnsupportedVersion)
        } else {
            Err(LayoutRecordError::InvalidRecord)
        };
    }
    if bytes[54..64].iter().any(|byte| *byte != 0) || bytes[144..160].iter().any(|byte| *byte != 0)
    {
        return Err(LayoutRecordError::InvalidRecord);
    }
    let metadata = LayoutMetadata::new(
        get_u64(bytes, 8),
        get_u64(bytes, 16),
        get_u64(bytes, 24),
        get_u64(bytes, 32),
        get_u64(bytes, 40),
    )?;
    let rows = get_u16(bytes, 48);
    let columns = get_u16(bytes, 50);
    let count = bytes[52];
    let axis = match bytes[53] {
        0 => LayoutAxis::Single,
        1 => LayoutAxis::SideBySide,
        2 => LayoutAxis::Stacked,
        _ => return Err(LayoutRecordError::InvalidRecord),
    };
    let first = get_target(bytes, 64);
    let second = get_target(bytes, 104);
    let panes = match count {
        0 if axis == LayoutAxis::Single
            && rows == 0
            && columns == 0
            && first.is_none()
            && second.is_none() =>
        {
            [None, None]
        }
        1 if axis == LayoutAxis::Single
            && valid_root(rows, columns)
            && first.is_some()
            && second.is_none() =>
        {
            [first, None]
        }
        2 if axis != LayoutAxis::Single
            && valid_root(rows, columns)
            && first.is_some()
            && second.is_some() =>
        {
            [first, second]
        }
        _ => return Err(LayoutRecordError::InvalidRecord),
    };
    if let Some(target) = panes[0] {
        validate_target(target, metadata)?;
    }
    if let Some(target) = panes[1] {
        validate_target(target, metadata)?;
        let first = panes[0].ok_or(LayoutRecordError::InvalidRecord)?;
        if first.pane == target.pane || first.session == target.session {
            return Err(LayoutRecordError::InvalidTargets);
        }
    }
    let result = SavedLayout {
        metadata,
        rows,
        columns,
        axis,
        panes,
    };
    if count == 2 {
        let [Some(a), Some(b)] = result.geometry() else {
            return Err(LayoutRecordError::InvalidGeometry);
        };
        let divider = canonical_divider(rows, columns, axis);
        if !valid_leaf(a.rect) || !valid_leaf(b.rect) || divider.is_none() {
            return Err(LayoutRecordError::InvalidGeometry);
        }
    }
    Ok(result)
}

fn infer_axis(
    rows: u16,
    columns: u16,
    first: Rect,
    second: Rect,
    divider: Option<Rect>,
) -> Result<Option<LayoutAxis>, LayoutRecordError> {
    if !valid_root(rows, columns) {
        return Err(LayoutRecordError::InvalidGeometry);
    }
    if !valid_leaf(first) || !valid_leaf(second) {
        return Ok(None);
    }
    for axis in [LayoutAxis::SideBySide, LayoutAxis::Stacked] {
        if let Some(expected_divider) = canonical_divider(rows, columns, axis) {
            let expected = match axis {
                LayoutAxis::SideBySide => {
                    let left = (columns - 1) / 2;
                    let start = left + 1;
                    [
                        Rect {
                            x: 0,
                            y: 0,
                            rows,
                            columns: left,
                        },
                        Rect {
                            x: start,
                            y: 0,
                            rows,
                            columns: columns - start,
                        },
                    ]
                }
                LayoutAxis::Stacked => {
                    let top = (rows - 1) / 2;
                    let start = top + 1;
                    [
                        Rect {
                            x: 0,
                            y: 0,
                            rows: top,
                            columns,
                        },
                        Rect {
                            x: 0,
                            y: start,
                            rows: rows - start,
                            columns,
                        },
                    ]
                }
                LayoutAxis::Single => unreachable!(),
            };
            if [first, second] == expected && divider == Some(expected_divider) {
                return Ok(Some(axis));
            }
        }
    }
    Ok(None)
}

fn canonical_divider(rows: u16, columns: u16, axis: LayoutAxis) -> Option<Rect> {
    match axis {
        LayoutAxis::SideBySide if columns >= 2 => Some(Rect {
            x: (columns - 1) / 2,
            y: 0,
            rows,
            columns: 1,
        }),
        LayoutAxis::Stacked if rows >= 2 => Some(Rect {
            x: 0,
            y: (rows - 1) / 2,
            rows: 1,
            columns,
        }),
        _ => None,
    }
}

fn valid_root(rows: u16, columns: u16) -> bool {
    (MIN_PANE_ROWS..=MAX_PANE_ROWS).contains(&rows)
        && (MIN_PANE_COLUMNS..=MAX_PANE_COLUMNS).contains(&columns)
}
fn valid_leaf(rect: Rect) -> bool {
    (MIN_PANE_ROWS..=MAX_PANE_ROWS).contains(&rect.rows)
        && (MIN_PANE_COLUMNS..=MAX_PANE_COLUMNS).contains(&rect.columns)
}
fn root(rows: u16, columns: u16) -> Rect {
    Rect {
        x: 0,
        y: 0,
        rows,
        columns,
    }
}
fn geometry(target: Option<TargetRef>, rect: Option<Rect>) -> Option<PaneGeometry> {
    Some(PaneGeometry {
        target: target?,
        rect: rect?,
    })
}
fn validate_target(target: TargetRef, metadata: LayoutMetadata) -> Result<(), LayoutRecordError> {
    if [
        target.generation,
        target.workspace,
        target.pane,
        target.session,
        target.occupant_revision,
    ]
    .contains(&0)
    {
        return Err(LayoutRecordError::InvalidTargets);
    }
    if target.generation != metadata.generation || target.workspace != metadata.workspace {
        return Err(LayoutRecordError::InvalidTargets);
    }
    Ok(())
}
fn put_u64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_be_bytes());
}
fn get_u64(bytes: &[u8], at: usize) -> u64 {
    u64::from_be_bytes(bytes[at..at + 8].try_into().expect("fixed slice"))
}
fn get_u16(bytes: &[u8], at: usize) -> u16 {
    u16::from_be_bytes(bytes[at..at + 2].try_into().expect("fixed slice"))
}
fn put_target(bytes: &mut [u8], at: usize, target: TargetRef) {
    for (offset, value) in [
        target.generation,
        target.workspace,
        target.pane,
        target.session,
        target.occupant_revision,
    ]
    .into_iter()
    .enumerate()
    {
        put_u64(bytes, at + offset * 8, value);
    }
}
fn get_target(bytes: &[u8], at: usize) -> Option<TargetRef> {
    let values = [
        get_u64(bytes, at),
        get_u64(bytes, at + 8),
        get_u64(bytes, at + 16),
        get_u64(bytes, at + 24),
        get_u64(bytes, at + 32),
    ];
    if values.iter().all(|value| *value == 0) {
        return None;
    }
    Some(TargetRef {
        generation: values[0],
        workspace: values[1],
        pane: values[2],
        session: values[3],
        occupant_revision: values[4],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane_layout::{PaneLayout, SplitDirection};

    fn meta(sequence: u64, revision: u64, time: u64) -> LayoutMetadata {
        LayoutMetadata::new(11, 22, sequence, revision, time).expect("valid metadata")
    }
    fn first() -> TargetRef {
        TargetRef {
            generation: 11,
            workspace: 22,
            pane: 33,
            session: 44,
            occupant_revision: 55,
        }
    }
    fn second() -> TargetRef {
        TargetRef {
            generation: 11,
            workspace: 22,
            pane: 66,
            session: 77,
            occupant_revision: 88,
        }
    }
    fn snap(direction: SplitDirection, rows: u16, columns: u16) -> LayoutSnapshot {
        let layout = PaneLayout::new(first(), rows, columns).expect("root");
        layout
            .preview_split(first(), 1, direction, second())
            .expect("split")
            .planned_snapshot()
    }

    #[test]
    fn one_pane_and_empty_roundtrip() {
        let one = PaneLayout::new(first(), 5, 20).expect("layout").snapshot();
        let value = SavedLayout::from_snapshot(meta(1, one.revision, 9), &one).expect("snapshot");
        assert_eq!(decode(&encode(&value)), Ok(value));
        assert_eq!(value.geometry()[0].expect("pane").rect, root(5, 20));
        let empty = SavedLayout::empty(meta(2, 1, 1));
        assert_eq!(decode(&encode(&empty)), Ok(empty));
        assert_eq!(empty.geometry(), [None, None]);
    }

    #[test]
    fn snapshots_cover_spatial_orders_odd_even_and_minimum_maximum() {
        for (direction, rows, columns) in [
            (SplitDirection::Left, 7, 82),
            (SplitDirection::Right, 7, 83),
            (SplitDirection::Up, 11, 20),
            (SplitDirection::Down, 12, 20),
            (SplitDirection::Left, 80, 240),
            (SplitDirection::Down, 80, 240),
        ] {
            let snapshot = snap(direction, rows, columns);
            let value = SavedLayout::from_snapshot(meta(1, snapshot.revision, 99), &snapshot)
                .expect("canonical snapshot");
            assert_eq!(decode(&encode(&value)), Ok(value));
            assert_eq!(
                value.geometry().map(|pane| pane.map(|p| p.rect)),
                snapshot.panes.map(|pane| pane.map(|p| p.rect))
            );
        }
    }

    #[test]
    fn rejects_every_nonexact_length_and_trailing_bytes() {
        let value = SavedLayout::empty(meta(1, 1, 1));
        let bytes = encode(&value);
        for length in 0..SAVED_LAYOUT_BYTES {
            assert_eq!(
                decode(&bytes[..length]),
                Err(LayoutRecordError::InvalidLength)
            );
        }
        let mut long = bytes.to_vec();
        long.push(0);
        assert_eq!(decode(&long), Err(LayoutRecordError::InvalidLength));
    }

    #[test]
    fn rejects_bad_header_reserved_tags_and_metadata() {
        let mut bytes = encode(&SavedLayout::empty(meta(1, 1, 1)));
        bytes[0] = b'X';
        assert_eq!(decode(&bytes), Err(LayoutRecordError::InvalidRecord));
        bytes = encode(&SavedLayout::empty(meta(1, 1, 1)));
        bytes[7] = b'2';
        assert_eq!(decode(&bytes), Err(LayoutRecordError::UnsupportedVersion));
        for at in [54, 63, 144, 159] {
            bytes = encode(&SavedLayout::empty(meta(1, 1, 1)));
            bytes[at] = 1;
            assert_eq!(decode(&bytes), Err(LayoutRecordError::InvalidRecord));
        }
        bytes = encode(&SavedLayout::empty(meta(1, 1, 1)));
        bytes[53] = 3;
        assert_eq!(decode(&bytes), Err(LayoutRecordError::InvalidRecord));
        for at in [8, 16, 24, 32, 40] {
            bytes = encode(&SavedLayout::empty(meta(1, 1, 1)));
            bytes[at..at + 8].fill(0);
            assert_eq!(decode(&bytes), Err(LayoutRecordError::InvalidMetadata));
        }
    }

    #[test]
    fn rejects_count_axis_hole_unused_target_and_scope_errors() {
        let one = PaneLayout::new(first(), 5, 20).expect("layout").snapshot();
        let value = SavedLayout::from_snapshot(meta(1, 1, 1), &one).expect("snapshot");
        let mut bytes = encode(&value);
        bytes[52] = 2;
        assert_eq!(decode(&bytes), Err(LayoutRecordError::InvalidRecord));
        bytes = encode(&value);
        bytes[64..104].fill(0);
        assert_eq!(decode(&bytes), Err(LayoutRecordError::InvalidRecord));
        bytes = encode(&value);
        bytes[104] = 1;
        assert_eq!(decode(&bytes), Err(LayoutRecordError::InvalidRecord));
        bytes = encode(&value);
        bytes[64..72].copy_from_slice(&12_u64.to_be_bytes());
        assert_eq!(decode(&bytes), Err(LayoutRecordError::InvalidTargets));
        let two = SavedLayout::from_snapshot(meta(1, 2, 1), &snap(SplitDirection::Right, 10, 41))
            .expect("split");
        bytes = encode(&two);
        bytes[104 + 16..104 + 24].copy_from_slice(&first().pane.to_be_bytes());
        assert_eq!(decode(&bytes), Err(LayoutRecordError::InvalidTargets));
        bytes = encode(&two);
        bytes[104 + 24..104 + 32].copy_from_slice(&first().session.to_be_bytes());
        assert_eq!(decode(&bytes), Err(LayoutRecordError::InvalidTargets));
    }

    #[test]
    fn rejects_geometry_bounds_and_snapshot_counterexamples_without_normalizing() {
        let two = snap(SplitDirection::Right, 10, 41);
        for alter in 0..5 {
            let mut bad = two;
            match alter {
                0 => bad.revision += 1,
                1 => bad.panes[0].as_mut().unwrap().rect.x += 1,
                2 => bad.divider = None,
                3 => bad.panes.swap(0, 1),
                _ => bad.panes[1] = None,
            }
            assert!(SavedLayout::from_snapshot(meta(1, two.revision, 1), &bad).is_err());
        }
        let one = PaneLayout::new(first(), 5, 20).expect("layout").snapshot();
        let mut bad = one;
        bad.rows = 4;
        assert_eq!(
            SavedLayout::from_snapshot(meta(1, 1, 1), &bad),
            Err(LayoutRecordError::InvalidGeometry)
        );
        let mut bytes = encode(&SavedLayout::from_snapshot(meta(1, 2, 1), &two).expect("split"));
        bytes[50..52].copy_from_slice(&40_u16.to_be_bytes());
        assert_eq!(decode(&bytes), Err(LayoutRecordError::InvalidGeometry));
    }

    #[test]
    fn rejects_canonical_splits_with_leaves_below_minimum() {
        let side_by_side = LayoutSnapshot {
            revision: 2,
            rows: 5,
            columns: 20,
            panes: [
                Some(PaneGeometry {
                    target: first(),
                    rect: Rect {
                        x: 0,
                        y: 0,
                        rows: 5,
                        columns: 9,
                    },
                }),
                Some(PaneGeometry {
                    target: second(),
                    rect: Rect {
                        x: 10,
                        y: 0,
                        rows: 5,
                        columns: 10,
                    },
                }),
            ],
            divider: Some(Rect {
                x: 9,
                y: 0,
                rows: 5,
                columns: 1,
            }),
        };
        assert_eq!(
            SavedLayout::from_snapshot(meta(1, 2, 1), &side_by_side),
            Err(LayoutRecordError::InvalidGeometry)
        );

        let stacked = LayoutSnapshot {
            revision: 2,
            rows: 5,
            columns: 20,
            panes: [
                Some(PaneGeometry {
                    target: first(),
                    rect: Rect {
                        x: 0,
                        y: 0,
                        rows: 2,
                        columns: 20,
                    },
                }),
                Some(PaneGeometry {
                    target: second(),
                    rect: Rect {
                        x: 0,
                        y: 3,
                        rows: 2,
                        columns: 20,
                    },
                }),
            ],
            divider: Some(Rect {
                x: 0,
                y: 2,
                rows: 1,
                columns: 20,
            }),
        };
        assert_eq!(
            SavedLayout::from_snapshot(meta(1, 2, 1), &stacked),
            Err(LayoutRecordError::InvalidGeometry)
        );
    }

    #[test]
    fn sequence_and_time_roundtrip_independently_and_debug_is_redacted() {
        let a = SavedLayout::empty(meta(2, 1, 900));
        let b = SavedLayout::empty(meta(3, 1, 100));
        assert_eq!(
            decode(&encode(&a))
                .expect("decode")
                .metadata()
                .snapshot_sequence(),
            2
        );
        assert_eq!(
            decode(&encode(&b))
                .expect("decode")
                .metadata()
                .observed_at_unix_ms(),
            100
        );
        let rendered = format!(
            "{:?} {:?} {:?}",
            a,
            a.metadata(),
            LayoutRecordError::InvalidMetadata
        );
        for secret in ["11", "22", "900"] {
            assert!(!rendered.contains(secret));
        }
    }
}
