use panelink::workspace_index::codec::{CodecError, MetadataRecord};
use panelink::workspace_index::{
    ProjectId, WorkspaceIndex, WorkspaceTabId, MAX_DISPLAY_NAME_BYTES,
};

fn project(id: u64) -> ProjectId {
    ProjectId::new(id).unwrap()
}
fn tab(id: u64) -> WorkspaceTabId {
    WorkspaceTabId::new(id).unwrap()
}

fn legacy_record() -> MetadataRecord {
    let mut index = WorkspaceIndex::new();
    index
        .add_project(project(1), "Default", tab(1), "Workspace")
        .unwrap();
    MetadataRecord::new(index, 1, 2).unwrap()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn legacy_default_record_matches_golden_bytes() {
    let expected = concat!(
        "504c574b49445831",
        "0001",
        "0000",
        "00000070",
        "0000000000000001",
        "0000000000000002",
        "0000000000000001",
        "0000000000000001",
        "0001",
        "000000000000",
        "0000000000000001",
        "0000000000000001",
        "0001",
        "0007",
        "00000000",
        "44656661756c74",
        "0000000000000001",
        "0009",
        "000000000000",
        "576f726b7370616365"
    );
    let bytes = legacy_record().encode().unwrap();
    assert_eq!(bytes.len(), 112);
    assert_eq!(hex(&bytes), expected);
    let decoded = MetadataRecord::decode(&bytes).unwrap();
    assert_eq!(decoded.revision(), 1);
    assert_eq!(decoded.next_id(), 2);
    assert_eq!(decoded.index().active_project_id(), Some(project(1)));
    assert_eq!(decoded.index().active_tab_id(), Some(tab(1)));
    assert_eq!(decoded.encode().unwrap(), bytes);
}

#[test]
fn round_trip_preserves_order_selection_and_empty_inactive_project() {
    let mut index = WorkspaceIndex::new();
    index
        .add_project(project(10), "Engineering", tab(11), "Build")
        .unwrap();
    index.add_tab(project(10), tab(12), "Review").unwrap();
    index.reorder_tab(project(10), tab(12), 0).unwrap();
    index.select_tab(project(10), tab(11)).unwrap();
    index
        .add_project(project(20), "Notes", tab(21), "Scratch")
        .unwrap();
    index.remove_tab(project(20), tab(21)).unwrap();
    index.add_tab(project(20), tab(22), "Reopened").unwrap();
    index
        .add_project(project(30), "Archive", tab(31), "Old")
        .unwrap();
    index.remove_tab(project(30), tab(31)).unwrap();
    index.select_project(project(10)).unwrap();

    let record = MetadataRecord::new(index, 37, 32).unwrap();
    let bytes = record.encode().unwrap();
    let decoded = MetadataRecord::decode(&bytes).unwrap();
    assert_eq!(decoded, record);
    assert_eq!(decoded.index().active_tab_id(), Some(tab(11)));
    let projects: Vec<_> = decoded.index().projects().collect();
    let tabs: Vec<_> = decoded.index().tabs(project(10)).unwrap().collect();
    assert_eq!(projects[0].display_name(), "Engineering");
    assert_eq!(
        tabs.iter().map(|item| item.id).collect::<Vec<_>>(),
        [tab(12), tab(11)]
    );
    assert_eq!(projects[1].selected_tab_id(), None);
    assert_eq!(projects[1].tabs().len(), 1);
    assert_eq!(projects[2].selected_tab_id(), None);
    assert_eq!(projects[2].tabs().len(), 0);
}

#[test]
fn exact_maximum_record_is_accepted_and_one_byte_over_is_rejected() {
    let long = "x".repeat(MAX_DISPLAY_NAME_BYTES);
    let mut index = WorkspaceIndex::new();
    let mut id = 1;
    for _ in 0..32 {
        index
            .add_project(project(id), &long, tab(id + 1), &long)
            .unwrap();
        let project_id = project(id);
        id += 2;
        for _ in 1..8 {
            index.add_tab(project_id, tab(id), &long).unwrap();
            id += 1;
        }
    }
    let record = MetadataRecord::new(index, 1, id).unwrap();
    let bytes = record.encode().unwrap();
    assert_eq!(bytes.len(), 23_352);
    assert_eq!(MetadataRecord::decode(&bytes).unwrap(), record);
    let mut too_large = bytes;
    too_large.push(0);
    assert_eq!(
        MetadataRecord::decode(&too_large),
        Err(CodecError::InvalidLength)
    );
}

#[test]
fn rejects_malformed_header_lengths_names_ids_and_selections() {
    let good = legacy_record().encode().unwrap();
    let cases: Vec<(usize, Vec<u8>, CodecError)> = vec![
        (
            0,
            {
                let mut b = good.clone();
                b[8..10].copy_from_slice(&2u16.to_be_bytes());
                b
            },
            CodecError::UnsupportedVersion,
        ),
        (
            1,
            {
                let mut b = good.clone();
                b[10] = 1;
                b
            },
            CodecError::ReservedField,
        ),
        (
            2,
            {
                let mut b = good.clone();
                b[16..24].fill(0);
                b
            },
            CodecError::InvalidRevision,
        ),
        (
            3,
            {
                let mut b = good.clone();
                b[24..32].copy_from_slice(&1u64.to_be_bytes());
                b
            },
            CodecError::InvalidWatermark,
        ),
        (
            4,
            {
                let mut b = good.clone();
                b[32..40].copy_from_slice(&9u64.to_be_bytes());
                b
            },
            CodecError::InvalidLegacyTabId,
        ),
        (
            5,
            {
                let mut b = good.clone();
                b[50] = 1;
                b
            },
            CodecError::ReservedField,
        ),
        (
            15,
            {
                let mut b = good.clone();
                b[48..50].copy_from_slice(&33u16.to_be_bytes());
                b
            },
            CodecError::InvalidCount,
        ),
        (
            6,
            {
                let mut b = good.clone();
                b[76] = 1;
                b
            },
            CodecError::ReservedField,
        ),
        (
            7,
            {
                let mut b = good.clone();
                b[56..64].copy_from_slice(&0u64.to_be_bytes());
                b
            },
            CodecError::InvalidCount,
        ),
        (
            8,
            {
                let mut b = good.clone();
                b[74..76].copy_from_slice(&65u16.to_be_bytes());
                b
            },
            CodecError::InvalidName,
        ),
        (
            9,
            {
                let mut b = good.clone();
                b[103] = 0xff;
                b
            },
            CodecError::InvalidName,
        ),
        (
            10,
            {
                let mut b = good.clone();
                b[64..72].copy_from_slice(&99u64.to_be_bytes());
                b
            },
            CodecError::InvalidSelection,
        ),
        (
            11,
            {
                let mut b = good.clone();
                b[40..48].copy_from_slice(&99u64.to_be_bytes());
                b
            },
            CodecError::InvalidActiveSelection,
        ),
        (
            14,
            {
                let mut b = good.clone();
                b[40..48].fill(0);
                b
            },
            CodecError::InvalidActiveSelection,
        ),
        (
            12,
            {
                let mut b = good.clone();
                b[72..74].copy_from_slice(&9u16.to_be_bytes());
                b
            },
            CodecError::InvalidCount,
        ),
        (
            13,
            {
                let mut b = good.clone();
                b[97] = 1;
                b
            },
            CodecError::ReservedField,
        ),
    ];
    for (_, bytes, expected) in cases {
        assert_eq!(MetadataRecord::decode(&bytes), Err(expected));
    }

    let mut duplicate_project = good.clone();
    duplicate_project[48..50].copy_from_slice(&2u16.to_be_bytes());
    duplicate_project.extend_from_slice(&good[56..]);
    let len = duplicate_project.len() as u32;
    duplicate_project[12..16].copy_from_slice(&len.to_be_bytes());
    assert_eq!(
        MetadataRecord::decode(&duplicate_project),
        Err(CodecError::DuplicateProjectId)
    );

    let mut duplicate_tab = good.clone();
    duplicate_tab[72..74].copy_from_slice(&2u16.to_be_bytes());
    duplicate_tab[12..16].copy_from_slice(&128u32.to_be_bytes());
    duplicate_tab.extend_from_slice(&[0; 16]);
    duplicate_tab[128 - 16..128 - 8].copy_from_slice(&1u64.to_be_bytes());
    duplicate_tab[128 - 8..128 - 6].copy_from_slice(&1u16.to_be_bytes());
    duplicate_tab.extend_from_slice(b"x");
    let duplicate_tab_len = duplicate_tab.len() as u32;
    duplicate_tab[12..16].copy_from_slice(&duplicate_tab_len.to_be_bytes());
    assert_eq!(
        MetadataRecord::decode(&duplicate_tab),
        Err(CodecError::DuplicateTabId)
    );
}

#[test]
fn every_truncation_and_trailing_byte_is_rejected() {
    let bytes = legacy_record().encode().unwrap();
    for end in 0..bytes.len() {
        assert!(
            MetadataRecord::decode(&bytes[..end]).is_err(),
            "prefix length {end}"
        );
    }
    let mut trailing = bytes;
    trailing.push(0);
    assert_eq!(
        MetadataRecord::decode(&trailing),
        Err(CodecError::InvalidLength)
    );
    assert_eq!(
        MetadataRecord::decode(&vec![0; 23_353]),
        Err(CodecError::InvalidLength)
    );
}

#[test]
fn rejects_invalid_model_watermarks_and_keeps_debug_name_free() {
    let mut index = WorkspaceIndex::new();
    index
        .add_project(
            project(4),
            "Sensitive project name",
            tab(8),
            "Sensitive tab name",
        )
        .unwrap();
    assert_eq!(
        MetadataRecord::new(index.clone(), 0, 9),
        Err(CodecError::InvalidRevision)
    );
    assert_eq!(
        MetadataRecord::new(index.clone(), 1, 8),
        Err(CodecError::InvalidWatermark)
    );
    let debug = format!("{:?}", MetadataRecord::new(index, 1, 9).unwrap());
    assert!(!debug.contains("Sensitive"));

    let empty = MetadataRecord::new(WorkspaceIndex::new(), 1, 2).unwrap();
    let mut malformed_empty = empty.encode().unwrap();
    malformed_empty[40..48].copy_from_slice(&1u64.to_be_bytes());
    assert_eq!(
        MetadataRecord::decode(&malformed_empty),
        Err(CodecError::InvalidActiveSelection)
    );
}
