//! MESH1 binary mesh envelope — Rust-side header validation only.
//!
//! Meshes travel verbatim end-to-end (worker → Rust `MeshCache` → webview
//! zero-copy typed arrays). Rust does NOT parse the mesh body, reorder, or
//! re-encode sections (Invariant 5): it validates the 64-byte header and the
//! section table bounds, then forwards the blob unchanged. All multi-byte fields
//! are little-endian; section data is 4-byte aligned. See
//! `../../protocol/mesh_format.md`.

/// MESH1 magic as a little-endian `u32` (`0x4D455348`). On the wire the four
/// bytes are `48 53 45 4D`; read back as an LE `u32` they equal this. Compare the
/// LE `u32` value (or the byte sequence), never the ASCII string `"MESH"` against
/// stream order. See `mesh_format.md` §2.
pub const MESH_MAGIC_LE: u32 = 0x4D45_5348;

/// The MESH1 magic as it appears in stream order (`[0x48, 0x53, 0x45, 0x4D]`).
pub const MESH_MAGIC_BYTES: [u8; 4] = MESH_MAGIC_LE.to_le_bytes();

/// The only MESH1 header version this crate accepts.
pub const MESH_VERSION: u16 = 1;

/// MESH1 header size: exactly one cache line.
pub const MESH_HEADER_LEN: usize = 64;

/// Offset where the section table begins (immediately after the header).
pub const MESH_SECTION_TABLE_OFF: usize = 0x40;

/// Size of one section-table entry: `type(u32) pad(u32) offset(u32) byteLen(u32)`.
pub const MESH_SECTION_ENTRY_LEN: usize = 16;

/// `flags` bit: NORMALS section present.
pub const FLAG_HAS_NORMALS: u16 = 0x0001;
/// `flags` bit: EDGE_* sections present.
pub const FLAG_HAS_EDGES: u16 = 0x0002;
/// `flags` bit: FACE_BBOXES section present.
pub const FLAG_HAS_FACE_BBOXES: u16 = 0x0004;
/// `flags` bit: id tables carry minted ElementIds (else pure TopoKeys).
pub const FLAG_IDS_HAVE_ELEMENTIDS: u16 = 0x0008;

/// Mask of all defined `flags` bits; any bit outside this must be zero.
const FLAGS_KNOWN_MASK: u16 =
    FLAG_HAS_NORMALS | FLAG_HAS_EDGES | FLAG_HAS_FACE_BBOXES | FLAG_IDS_HAVE_ELEMENTIDS;

/// Level-of-detail tier for a tessellation (`mesh_format.md` §2 `lod`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lod {
    /// `lod = 0`.
    Coarse,
    /// `lod = 1`.
    Medium,
    /// `lod = 2`.
    Fine,
}

impl Lod {
    /// Map the raw `u16` LOD field, if it is a known tier.
    pub fn from_u16(v: u16) -> Option<Lod> {
        match v {
            0 => Some(Lod::Coarse),
            1 => Some(Lod::Medium),
            2 => Some(Lod::Fine),
            _ => None,
        }
    }
}

/// One parsed section-table entry (`mesh_format.md` §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeshSectionEntry {
    /// Section type (see `mesh_format.md` §4).
    pub section_type: u32,
    /// Byte offset of section data from blob start (4-byte aligned).
    pub offset: u32,
    /// Section data length in bytes.
    pub byte_len: u32,
}

/// Validated view over a MESH1 header + section table.
///
/// Owns copies of the header fields and the parsed section entries; the blob
/// itself is forwarded verbatim by the caller (this view never mutates it).
#[derive(Debug, Clone, PartialEq)]
pub struct MeshHeaderView {
    pub version: u16,
    pub flags: u16,
    pub vertex_count: u32,
    pub triangle_count: u32,
    pub face_count: u32,
    pub edge_count: u32,
    pub edge_point_count: u32,
    pub lod: u16,
    pub section_count: u16,
    pub bbox_min: [f32; 3],
    pub bbox_max: [f32; 3],
    /// Total blob length in bytes (equals the input slice length).
    pub total_len: usize,
    /// Section table, in file order (sorted by offset).
    pub sections: Vec<MeshSectionEntry>,
}

impl MeshHeaderView {
    /// Whether the NORMALS section is declared present.
    pub fn has_normals(&self) -> bool {
        self.flags & FLAG_HAS_NORMALS != 0
    }
    /// Whether the EDGE_* sections are declared present.
    pub fn has_edges(&self) -> bool {
        self.flags & FLAG_HAS_EDGES != 0
    }
    /// Whether id tables carry minted ElementIds (else pure TopoKeys).
    pub fn ids_have_element_ids(&self) -> bool {
        self.flags & FLAG_IDS_HAVE_ELEMENTIDS != 0
    }
    /// Look up a section entry by type.
    pub fn section(&self, section_type: u32) -> Option<&MeshSectionEntry> {
        self.sections
            .iter()
            .find(|s| s.section_type == section_type)
    }
}

/// Reasons a MESH1 blob's header/table is rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MeshError {
    /// Fewer than 64 bytes — no room for a header.
    #[error("mesh blob too small: {0} bytes < 64-byte header")]
    TooSmall(usize),
    /// The magic `u32` (LE) was not `0x4D455348`.
    #[error("bad MESH1 magic")]
    BadMagic,
    /// Header `version` was not `1`.
    #[error("unsupported MESH1 version: {0}")]
    UnsupportedVersion(u16),
    /// A reserved header field was non-zero.
    #[error("reserved header field {0} must be zero")]
    ReservedNonZero(&'static str),
    /// A `flags` bit outside the defined mask was set.
    #[error("unknown flags bits set: {0:#06x}")]
    UnknownFlags(u16),
    /// The section table runs past the end of the blob.
    #[error("truncated section table: need {need} bytes, blob is {have}")]
    TruncatedTable { need: usize, have: usize },
    /// A section-entry `pad` field was non-zero.
    #[error("section {index} pad must be zero")]
    SectionPadNonZero { index: usize },
    /// A section `offset` was not 4-byte aligned.
    #[error("section {index} offset {offset} is not 4-byte aligned")]
    Misaligned { index: usize, offset: u32 },
    /// A section runs outside the blob (or into the header/table region).
    #[error("section {index} out of bounds: offset {offset} len {len}, blob {blob}")]
    SectionOutOfBounds {
        index: usize,
        offset: u32,
        len: u32,
        blob: usize,
    },
    /// Section entries were not sorted by ascending offset, or two overlap.
    #[error("section {index} overlaps previous section or is not sorted by offset")]
    SectionOrder { index: usize },
    /// The same section `type` appeared more than once.
    #[error("duplicate section type {0}")]
    DuplicateType(u32),
}

#[inline]
fn u16_le(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}

#[inline]
fn u32_le(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

#[inline]
fn f32_le(buf: &[u8], off: usize) -> f32 {
    f32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

/// Validate a MESH1 blob's header and section table, returning a header view.
///
/// Cheap and header-only per `mesh_format.md` §6.2: checks the magic (LE `u32`),
/// version, reserved fields, section-table bounds, and each section's alignment,
/// bounds, ordering and type-uniqueness. Does NOT parse section *data*. The blob
/// is forwarded verbatim by the caller.
pub fn validate_mesh_blob(buf: &[u8]) -> Result<MeshHeaderView, MeshError> {
    if buf.len() < MESH_HEADER_LEN {
        return Err(MeshError::TooSmall(buf.len()));
    }
    if u32_le(buf, 0) != MESH_MAGIC_LE {
        return Err(MeshError::BadMagic);
    }
    let version = u16_le(buf, 0x04);
    if version != MESH_VERSION {
        return Err(MeshError::UnsupportedVersion(version));
    }
    let flags = u16_le(buf, 0x06);
    if flags & !FLAGS_KNOWN_MASK != 0 {
        return Err(MeshError::UnknownFlags(flags & !FLAGS_KNOWN_MASK));
    }
    let vertex_count = u32_le(buf, 0x08);
    let triangle_count = u32_le(buf, 0x0C);
    let face_count = u32_le(buf, 0x10);
    let edge_count = u32_le(buf, 0x14);
    let edge_point_count = u32_le(buf, 0x18);
    let lod = u16_le(buf, 0x1C);
    let section_count = u16_le(buf, 0x1E) as usize;
    let bbox_min = [f32_le(buf, 0x20), f32_le(buf, 0x24), f32_le(buf, 0x28)];
    let bbox_max = [f32_le(buf, 0x2C), f32_le(buf, 0x30), f32_le(buf, 0x34)];
    if u32_le(buf, 0x38) != 0 {
        return Err(MeshError::ReservedNonZero("reserved0"));
    }
    if u32_le(buf, 0x3C) != 0 {
        return Err(MeshError::ReservedNonZero("reserved1"));
    }

    let table_end = MESH_SECTION_TABLE_OFF + section_count * MESH_SECTION_ENTRY_LEN;
    if buf.len() < table_end {
        return Err(MeshError::TruncatedTable {
            need: table_end,
            have: buf.len(),
        });
    }

    let mut sections = Vec::with_capacity(section_count);
    let mut seen_types: Vec<u32> = Vec::with_capacity(section_count);
    let mut prev_end: usize = table_end; // sections start after the table
    for index in 0..section_count {
        let base = MESH_SECTION_TABLE_OFF + index * MESH_SECTION_ENTRY_LEN;
        let section_type = u32_le(buf, base);
        let pad = u32_le(buf, base + 0x04);
        let offset = u32_le(buf, base + 0x08);
        let byte_len = u32_le(buf, base + 0x0C);

        if pad != 0 {
            return Err(MeshError::SectionPadNonZero { index });
        }
        if !offset.is_multiple_of(4) {
            return Err(MeshError::Misaligned { index, offset });
        }
        // Bounds: offset + byte_len must fit inside the blob (checked-add).
        let end = (offset as usize)
            .checked_add(byte_len as usize)
            .filter(|&e| e <= buf.len())
            .ok_or(MeshError::SectionOutOfBounds {
                index,
                offset,
                len: byte_len,
                blob: buf.len(),
            })?;
        // Sorted by ascending offset, non-overlapping, and past the table.
        if (offset as usize) < prev_end {
            return Err(MeshError::SectionOrder { index });
        }
        if seen_types.contains(&section_type) {
            return Err(MeshError::DuplicateType(section_type));
        }
        seen_types.push(section_type);
        sections.push(MeshSectionEntry {
            section_type,
            offset,
            byte_len,
        });
        prev_end = end;
    }

    Ok(MeshHeaderView {
        version,
        flags,
        vertex_count,
        triangle_count,
        face_count,
        edge_count,
        edge_point_count,
        lod,
        section_count: section_count as u16,
        bbox_min,
        bbox_max,
        total_len: buf.len(),
        sections,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build the exact 424-byte golden blob from `mesh_format.md` §5, byte by
    /// byte, so the test is an independent re-derivation of the worked example.
    fn golden_blob() -> Vec<u8> {
        let mut b = vec![0u8; 424];
        // --- header (0x00..0x40) ---
        b[0x00..0x04].copy_from_slice(&MESH_MAGIC_LE.to_le_bytes()); // 48 53 45 4D
        b[0x04..0x06].copy_from_slice(&1u16.to_le_bytes()); // version
        b[0x06..0x08].copy_from_slice(&0x0003u16.to_le_bytes()); // HAS_NORMALS|HAS_EDGES
        b[0x08..0x0C].copy_from_slice(&4u32.to_le_bytes()); // vertexCount
        b[0x0C..0x10].copy_from_slice(&2u32.to_le_bytes()); // triangleCount
        b[0x10..0x14].copy_from_slice(&2u32.to_le_bytes()); // faceCount
        b[0x14..0x18].copy_from_slice(&1u32.to_le_bytes()); // edgeCount
        b[0x18..0x1C].copy_from_slice(&2u32.to_le_bytes()); // edgePointCount
        b[0x1C..0x1E].copy_from_slice(&0u16.to_le_bytes()); // lod
        b[0x1E..0x20].copy_from_slice(&10u16.to_le_bytes()); // sectionCount
        b[0x2C..0x30].copy_from_slice(&1.0f32.to_le_bytes()); // bboxMaxX
        b[0x30..0x34].copy_from_slice(&1.0f32.to_le_bytes()); // bboxMaxY
                                                              // reserved0/1 already zero.

        // --- section table (0x40..0xE0): type,pad,offset,byteLen ---
        let entries: [(u32, u32, u32); 10] = [
            (1, 224, 48), // POSITIONS
            (2, 272, 48), // NORMALS
            (3, 320, 24), // INDICES
            (4, 344, 16), // FACE_RANGES
            (5, 360, 12), // FACE_ID_OFFS
            (6, 372, 7),  // FACE_ID_CHARS
            (7, 380, 8),  // EDGE_RANGES
            (8, 388, 24), // EDGE_POSITIONS
            (9, 412, 8),  // EDGE_ID_OFFS
            (10, 420, 3), // EDGE_ID_CHARS
        ];
        for (i, (ty, off, len)) in entries.iter().enumerate() {
            let base = 0x40 + i * 16;
            b[base..base + 4].copy_from_slice(&ty.to_le_bytes());
            // pad stays 0
            b[base + 8..base + 12].copy_from_slice(&off.to_le_bytes());
            b[base + 12..base + 16].copy_from_slice(&len.to_le_bytes());
        }

        // Section data content is not validated by the header check, but fill a
        // couple of representative sections so the blob is a faithful example.
        // FACE_ID_CHARS @372: "f:7" + "f:12"
        b[372..379].copy_from_slice(b"f:7f:12");
        // EDGE_ID_CHARS @420: "e:5"
        b[420..423].copy_from_slice(b"e:5");
        b
    }

    #[test]
    fn golden_blob_validates() {
        let blob = golden_blob();
        let view = validate_mesh_blob(&blob).expect("golden must validate");
        assert_eq!(view.version, 1);
        assert_eq!(view.vertex_count, 4);
        assert_eq!(view.triangle_count, 2);
        assert_eq!(view.face_count, 2);
        assert_eq!(view.edge_count, 1);
        assert_eq!(view.edge_point_count, 2);
        assert_eq!(view.section_count, 10);
        assert!(view.has_normals());
        assert!(view.has_edges());
        assert!(!view.ids_have_element_ids());
        assert_eq!(view.bbox_min, [0.0, 0.0, 0.0]);
        assert_eq!(view.bbox_max, [1.0, 1.0, 0.0]);
        assert_eq!(view.total_len, 424);
        // POSITIONS section resolves.
        let pos = view.section(1).unwrap();
        assert_eq!(pos.offset, 224);
        assert_eq!(pos.byte_len, 48);
        assert_eq!(Lod::from_u16(view.lod), Some(Lod::Coarse));
    }

    #[test]
    fn too_small_rejected() {
        assert_eq!(validate_mesh_blob(&[0u8; 10]), Err(MeshError::TooSmall(10)));
    }

    #[test]
    fn bad_magic_rejected() {
        let mut blob = golden_blob();
        blob[0] = 0x00;
        assert_eq!(validate_mesh_blob(&blob), Err(MeshError::BadMagic));
    }

    #[test]
    fn wrong_version_rejected() {
        let mut blob = golden_blob();
        blob[0x04..0x06].copy_from_slice(&2u16.to_le_bytes());
        assert_eq!(
            validate_mesh_blob(&blob),
            Err(MeshError::UnsupportedVersion(2))
        );
    }

    #[test]
    fn reserved_nonzero_rejected() {
        let mut blob = golden_blob();
        blob[0x38] = 1;
        assert_eq!(
            validate_mesh_blob(&blob),
            Err(MeshError::ReservedNonZero("reserved0"))
        );
    }

    #[test]
    fn truncated_blob_rejected() {
        let blob = golden_blob();
        // Chop the final EDGE_ID_CHARS section off — the last section now runs
        // past the (shorter) blob end.
        let truncated = &blob[..420];
        match validate_mesh_blob(truncated) {
            Err(MeshError::SectionOutOfBounds { index, .. }) => assert_eq!(index, 9),
            other => panic!("expected out-of-bounds, got {other:?}"),
        }
    }

    #[test]
    fn truncated_section_table_rejected() {
        let mut blob = golden_blob();
        // Claim 200 sections but keep the short blob: the table itself overruns.
        blob[0x1E..0x20].copy_from_slice(&200u16.to_le_bytes());
        assert!(matches!(
            validate_mesh_blob(&blob),
            Err(MeshError::TruncatedTable { .. })
        ));
    }

    #[test]
    fn misaligned_section_rejected() {
        let mut blob = golden_blob();
        // Bump POSITIONS offset from 224 to 226 (not 4-aligned).
        let base = 0x40;
        blob[base + 8..base + 12].copy_from_slice(&226u32.to_le_bytes());
        match validate_mesh_blob(&blob) {
            Err(MeshError::Misaligned { index, offset }) => {
                assert_eq!(index, 0);
                assert_eq!(offset, 226);
            }
            other => panic!("expected Misaligned, got {other:?}"),
        }
    }

    #[test]
    fn section_pad_nonzero_rejected() {
        let mut blob = golden_blob();
        let base = 0x40; // first entry
        blob[base + 4..base + 8].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(
            validate_mesh_blob(&blob),
            Err(MeshError::SectionPadNonZero { index: 0 })
        );
    }

    #[test]
    fn duplicate_section_type_rejected() {
        let mut blob = golden_blob();
        // Make the NORMALS entry (index 1) claim type 1 (== POSITIONS).
        let base = 0x40 + 16;
        blob[base..base + 4].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(validate_mesh_blob(&blob), Err(MeshError::DuplicateType(1)));
    }

    #[test]
    fn out_of_order_sections_rejected() {
        let mut blob = golden_blob();
        // Swap offsets of the first two sections so they are no longer ascending.
        // POSITIONS -> 272, NORMALS -> 224 makes index 1 (offset 224) < prev_end.
        blob[0x40 + 8..0x40 + 12].copy_from_slice(&272u32.to_le_bytes());
        blob[0x50 + 8..0x50 + 12].copy_from_slice(&224u32.to_le_bytes());
        match validate_mesh_blob(&blob) {
            Err(MeshError::SectionOrder { index }) => assert_eq!(index, 1),
            other => panic!("expected SectionOrder, got {other:?}"),
        }
    }

    #[test]
    fn magic_bytes_stream_order() {
        // On the wire the magic reads 48 53 45 4D per mesh_format.md §2.
        assert_eq!(MESH_MAGIC_BYTES, [0x48, 0x53, 0x45, 0x4D]);
    }
}
