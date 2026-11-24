//! Extract the largest embedded icon from a Windows .exe (v0.2.6).
//!
//! Last-resort cover art: when a game has no art from Steam, GOG,
//! SteamGridDB or a manual pick, the .exe's own icon is shown
//! centered on the Sonic gradient card. Only PNG icons are used
//! (modern icons embed PNG data); old BMP/DIB-only icons are
//! skipped, and icons smaller than 64 px on their largest side are
//! skipped so a tiny icon never shows up blurry.
//!
//! Pure-Rust PE resource parsing, no new dependencies. Every parse
//! step is bounds-checked and returns `None` on malformed input —
//! never panics, never crashes the UI.

use std::path::Path;

/// Largest side (px) an icon must have to be used. Documented:
/// the tile shows the icon at ~62% of a 108–175 px tile, so
/// anything under 64 px would upscale blurry.
pub const MIN_ICON_PX: u32 = 64;

/// Largest .exe we will read (bytes). Game exes are rarely above a
/// few tens of MB; anything bigger is not worth slurping into memory
/// for an icon.
pub const MAX_EXE_BYTES: usize = 128 * 1024 * 1024;

const PNG_MAGIC: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

/// `true` when the bytes start with the PNG magic signature.
pub fn is_png(b: &[u8]) -> bool {
    b.len() >= PNG_MAGIC.len() && b[..8] == *PNG_MAGIC
}

/// Width and height from a PNG's IHDR chunk (`None` when the bytes
/// are not a PNG or are too short to hold an IHDR). The CRC is not
/// checked — only the header is parsed.
pub fn png_dimensions(png: &[u8]) -> Option<(u32, u32)> {
    if !is_png(png) || png.len() < 24 {
        return None;
    }
    if &png[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(png[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(png[20..24].try_into().ok()?);
    Some((w, h))
}

fn u16le(b: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(off..off + 2)?.try_into().ok()?))
}

fn u32le(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(off..off + 4)?.try_into().ok()?))
}

struct Section {
    virt_addr: u32,
    virt_size: u32,
    raw_ptr: u32,
    raw_size: u32,
}

struct PeInfo {
    rsrc_rva: u32,
    rsrc_size: u32,
    sections: Vec<Section>,
    headers_size: u32,
}

/// Parse the DOS + NT headers and the section table. Returns the
/// resource directory's RVA/size plus the sections needed to map
/// RVAs to file offsets. `None` on any malformed header.
fn parse_pe(pe: &[u8]) -> Option<PeInfo> {
    if pe.len() < 0x40 || &pe[0..2] != b"MZ" {
        return None;
    }
    let e_lfanew = u32le(pe, 0x3C)? as usize;
    let nt = pe.get(e_lfanew..)?;
    if nt.len() < 24 || &nt[0..4] != b"PE\0\0" {
        return None;
    }
    let num_sections = u16le(nt, 6)? as usize;
    if num_sections == 0 || num_sections > 96 {
        return None;
    }
    let opt_size = u16le(nt, 20)? as usize;
    let opt = nt.get(24..24 + opt_size)?;
    let magic = u16le(opt, 0)?;
    // Data directories start at offset 96 (PE32) / 112 (PE32+);
    // entry 2 is the resource directory (RVA, Size).
    let dd_base = match magic {
        0x10B => 96,
        0x20B => 112,
        _ => return None,
    };
    if opt.len() < dd_base + 24 {
        return None;
    }
    let rsrc_rva = u32le(opt, dd_base + 16)?;
    let rsrc_size = u32le(opt, dd_base + 20)?;
    // SizeOfHeaders sits at offset 60 in both PE32 and PE32+.
    let headers_size = u32le(opt, 60)?;
    let sect_off = e_lfanew.checked_add(24)?.checked_add(opt_size)?;
    let mut sections = Vec::new();
    for i in 0..num_sections {
        let s = pe.get(sect_off + i * 40..sect_off + i * 40 + 40)?;
        sections.push(Section {
            virt_addr: u32le(s, 12)?,
            virt_size: u32le(s, 8)?,
            raw_ptr: u32le(s, 20)?,
            raw_size: u32le(s, 16)?,
        });
    }
    Some(PeInfo {
        rsrc_rva,
        rsrc_size,
        sections,
        headers_size,
    })
}

/// Map a relative virtual address to a file offset. RVAs inside the
/// headers map to themselves; otherwise the containing section wins.
fn rva_to_offset(info: &PeInfo, rva: u32) -> Option<usize> {
    if rva < info.headers_size {
        return Some(rva as usize);
    }
    for s in &info.sections {
        if rva >= s.virt_addr {
            let delta = rva.checked_sub(s.virt_addr)?;
            if delta < s.virt_size.max(s.raw_size) {
                return (s.raw_ptr as usize).checked_add(delta as usize);
            }
        }
    }
    None
}

/// One resource directory, as a slice of the resource section.
struct Rsrc<'a> {
    pe: &'a [u8],
    info: &'a PeInfo,
    data: &'a [u8],
}

impl<'a> Rsrc<'a> {
    /// `(named_count, id_count)` of the directory at `off`.
    fn dir_header(&self, off: usize) -> Option<(usize, usize)> {
        let _ = self.data.get(off..off + 16)?;
        Some((
            u16le(self.data, off + 12)? as usize,
            u16le(self.data, off + 14)? as usize,
        ))
    }

    /// Entry `idx` of the directory at `off`: `(name, target)`.
    /// A target with the high bit set is a subdirectory (offset
    /// relative to the resource section); otherwise it is a data
    /// entry (offset of a 16-byte IMAGE_RESOURCE_DATA_ENTRY).
    fn dir_entry(&self, off: usize, idx: usize) -> Option<(u32, u32)> {
        let e = self.data.get(off + 16 + idx * 8..off + 16 + idx * 8 + 8)?;
        Some((u32le(e, 0)?, u32le(e, 4)?))
    }

    /// The subdirectory offset (relative to the resource section)
    /// for numeric `name_id` under the directory at `dir_off`.
    /// String names (high bit set) are skipped.
    fn find_subdir(&self, dir_off: usize, name_id: u16) -> Option<usize> {
        let (named, ids) = self.dir_header(dir_off)?;
        let total = named.checked_add(ids)?;
        for i in 0..total {
            let (name, target) = self.dir_entry(dir_off, i)?;
            if name & 0x8000_0000 != 0 {
                continue;
            }
            if name as u16 == name_id && target & 0x8000_0000 != 0 {
                return Some((target & 0x7FFF_FFFF) as usize);
            }
        }
        None
    }

    /// Read a data entry: returns the (file offset, size) of the
    /// resource bytes it points at.
    fn read_data_entry(&self, data_off: usize) -> Option<(usize, usize)> {
        let e = self.data.get(data_off..data_off + 16)?;
        let rva = u32le(e, 0)?;
        let size = u32le(e, 4)? as usize;
        let off = rva_to_offset(self.info, rva)?;
        self.pe.get(off..off.checked_add(size)?)?;
        Some((off, size))
    }

    /// Find a resource by (type, name): type dir -> name dir ->
    /// first language entry -> data entry.
    fn find_resource(&self, type_id: u16, name_id: u16) -> Option<(usize, usize)> {
        let type_dir = self.find_subdir(0, type_id)?;
        let name_dir = self.find_subdir(type_dir, name_id)?;
        let (named, ids) = self.dir_header(name_dir)?;
        let total = named.checked_add(ids)?;
        for i in 0..total {
            let (_name, target) = self.dir_entry(name_dir, i)?;
            if target & 0x8000_0000 == 0 {
                return self.read_data_entry(target as usize);
            }
        }
        None
    }

    /// All numeric name IDs under a type (e.g. all group-icon names).
    fn list_names(&self, type_id: u16) -> Vec<u16> {
        let mut out = Vec::new();
        let Some(type_dir) = self.find_subdir(0, type_id) else {
            return out;
        };
        let Some((named, ids)) = self.dir_header(type_dir) else {
            return out;
        };
        let Some(total) = named.checked_add(ids) else {
            return out;
        };
        for i in 0..total {
            if let Some((name, _)) = self.dir_entry(type_dir, i) {
                if name & 0x8000_0000 == 0 {
                    out.push(name as u16);
                }
            }
        }
        out
    }
}

/// One entry of a GRPICONDIR (14 bytes): the icon's nominal size
/// (0 means 256), bit depth, byte size, and the RT_ICON resource id.
struct IconEntry {
    width: u32,
    height: u32,
    bit_count: u16,
    bytes_in_res: u32,
    id: u16,
}

/// Parse a GRPICONDIR into its entries. A count larger than the
/// bytes present is capped — a corrupt count never runs away.
fn parse_group(data: &[u8]) -> Option<Vec<IconEntry>> {
    if data.len() < 6 {
        return None;
    }
    let count = u16le(data, 4)? as usize;
    let max = data.len().saturating_sub(6) / 14;
    let count = count.min(max);
    let mut out = Vec::new();
    for i in 0..count {
        let e = data.get(6 + i * 14..6 + i * 14 + 14)?;
        let w = e[0] as u32;
        let h = e[1] as u32;
        out.push(IconEntry {
            width: if w == 0 { 256 } else { w },
            height: if h == 0 { 256 } else { h },
            bit_count: u16le(e, 6)?,
            bytes_in_res: u32le(e, 8)?,
            id: u16le(e, 12)?,
        });
    }
    Some(out)
}

/// Extract the largest embedded icon as PNG bytes, or `None` when
/// the file is not a PE, has no icon resources, there is no PNG
/// icon entry, or the largest PNG is smaller than
/// [`MIN_ICON_PX`] on its largest side. Never panics on malformed
/// input — every step is bounds-checked.
pub fn extract_largest_icon_png(pe: &[u8]) -> Option<Vec<u8>> {
    let info = parse_pe(pe)?;
    if info.rsrc_rva == 0 || info.rsrc_size == 0 {
        return None;
    }
    let rsrc_off = rva_to_offset(&info, info.rsrc_rva)?;
    let rsrc = pe.get(rsrc_off..rsrc_off.checked_add(info.rsrc_size as usize)?)?;
    let r = Rsrc {
        pe,
        info: &info,
        data: rsrc,
    };
    // Largest entry across all RT_GROUP_ICON groups: most pixels,
    // then highest bit depth, then most bytes.
    let mut best: Option<IconEntry> = None;
    for name in r.list_names(14) {
        let Some((off, size)) = r.find_resource(14, name) else {
            continue;
        };
        let Some(group_bytes) = pe.get(off..off.checked_add(size)?) else {
            continue;
        };
        let Some(entries) = parse_group(group_bytes) else {
            continue;
        };
        for e in entries {
            // Only PNG entries compete: a larger BMP must not
            // shadow a smaller usable PNG.
            let Some((doff, dsize)) = r.find_resource(3, e.id) else {
                continue;
            };
            let Some(data) = pe.get(doff..doff.checked_add(dsize)?) else {
                continue;
            };
            if !is_png(data) {
                continue;
            }
            let better = match &best {
                None => true,
                Some(b) => {
                    (e.width * e.height, e.bit_count, e.bytes_in_res)
                        > (b.width * b.height, b.bit_count, b.bytes_in_res)
                }
            };
            if better {
                best = Some(e);
            }
        }
    }
    let best = best?;
    let (off, size) = r.find_resource(3, best.id)?;
    let bytes = pe.get(off..off.checked_add(size)?)?;
    if !is_png(bytes) {
        return None;
    }
    let (w, h) = png_dimensions(bytes)?;
    if w.max(h) < MIN_ICON_PX {
        return None;
    }
    Some(bytes.to_vec())
}

/// Read an .exe (capped at [`MAX_EXE_BYTES`]) and extract its
/// largest embedded icon as PNG bytes, or `None`.
pub fn extract_exe_icon_png(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let f = std::fs::File::open(path).ok()?;
    let mut buf = Vec::new();
    f.take(MAX_EXE_BYTES as u64 + 1)
        .read_to_end(&mut buf)
        .ok()?;
    if buf.len() > MAX_EXE_BYTES {
        return None;
    }
    extract_largest_icon_png(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal PNG bytes with the given IHDR dimensions. Only the
    /// header is real — the parser reads nothing past IHDR.
    fn fake_png(w: u32, h: u32) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        v.extend_from_slice(&13u32.to_be_bytes());
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
        v.extend_from_slice(&[0, 0, 0, 0]); // fake CRC
        v.extend_from_slice(&0u32.to_be_bytes());
        v.extend_from_slice(b"IEND");
        v.extend_from_slice(&[0, 0, 0, 0]);
        v
    }

    /// One group-icon entry: (width byte, height byte, bit count,
    /// icon resource id, icon bytes). Width/height 0 means 256.
    type GroupEntry = (u8, u8, u16, u16, Vec<u8>);

    /// Build a minimal PE32+ with one .rsrc section holding the
    /// given RT_GROUP_ICON groups: (group name id, entries).
    fn build_pe(groups: &[(u16, Vec<GroupEntry>)]) -> Vec<u8> {
        let n_groups = groups.len();
        let n_icons: usize = groups.iter().map(|(_, e)| e.len()).sum();
        // Directory tree layout (offsets relative to the section).
        let root_size = 16 + 2 * 8;
        let t14_size = 16 + n_groups * 8;
        let t3_size = 16 + n_icons * 8;
        let lang_size = 16 + 8; // one language entry each
        let t14_off = root_size;
        let t3_off = t14_off + t14_size;
        let g_lang_off = t3_off + t3_size;
        let i_lang_off = g_lang_off + n_groups * lang_size;
        let data_entries_off = i_lang_off + n_icons * lang_size;
        let n_data = n_groups + n_icons;
        let raw_off = data_entries_off + n_data * 16;
        // Raw data: group bytes first, then icon bytes, in order.
        let mut raw: Vec<u8> = Vec::new();
        let mut group_raw_off: Vec<usize> = Vec::new();
        let mut icon_raw_off: Vec<usize> = Vec::new();
        let mut icon_id_order: Vec<u16> = Vec::new();
        for (_, entries) in groups {
            group_raw_off.push(raw.len());
            let mut g = Vec::new();
            g.extend_from_slice(&0u16.to_le_bytes());
            g.extend_from_slice(&1u16.to_le_bytes());
            g.extend_from_slice(&(entries.len() as u16).to_le_bytes());
            for (w, h, bits, id, bytes) in entries {
                g.push(*w);
                g.push(*h);
                g.push(0); // color count
                g.push(0); // reserved
                g.extend_from_slice(&1u16.to_le_bytes()); // planes
                g.extend_from_slice(&bits.to_le_bytes());
                g.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                g.extend_from_slice(&id.to_le_bytes());
                icon_id_order.push(*id);
            }
            raw.extend_from_slice(&g);
        }
        for (_, entries) in groups {
            for (_, _, _, _, bytes) in entries {
                icon_raw_off.push(raw.len());
                raw.extend_from_slice(bytes);
            }
        }
        let rsrc_len = raw_off + raw.len();
        let mut rsrc = vec![0u8; rsrc_len];
        let w16 = |rsrc: &mut Vec<u8>, off: usize, v: u16| {
            rsrc[off..off + 2].copy_from_slice(&v.to_le_bytes());
        };
        let w32 = |rsrc: &mut Vec<u8>, off: usize, v: u32| {
            rsrc[off..off + 4].copy_from_slice(&v.to_le_bytes());
        };
        // Root dir: 0 named, 2 id entries (types 14 and 3).
        w16(&mut rsrc, 12, 0);
        w16(&mut rsrc, 14, 2);
        w32(&mut rsrc, 16, 14);
        w32(&mut rsrc, 20, (t14_off as u32) | 0x8000_0000);
        w32(&mut rsrc, 24, 3);
        w32(&mut rsrc, 28, (t3_off as u32) | 0x8000_0000);
        // Type 14 dir: one entry per group.
        w16(&mut rsrc, t14_off + 12, 0);
        w16(&mut rsrc, t14_off + 14, n_groups as u16);
        for (gi, (name, _)) in groups.iter().enumerate() {
            let e = t14_off + 16 + gi * 8;
            w32(&mut rsrc, e, *name as u32);
            w32(
                &mut rsrc,
                e + 4,
                ((g_lang_off + gi * lang_size) as u32) | 0x8000_0000,
            );
        }
        // Type 3 dir: one entry per icon.
        w16(&mut rsrc, t3_off + 12, 0);
        w16(&mut rsrc, t3_off + 14, n_icons as u16);
        for (k, id) in icon_id_order.iter().enumerate() {
            let e = t3_off + 16 + k * 8;
            w32(&mut rsrc, e, *id as u32);
            w32(
                &mut rsrc,
                e + 4,
                ((i_lang_off + k * lang_size) as u32) | 0x8000_0000,
            );
        }
        // Language dirs: one entry (0x409) pointing at a data entry.
        for gi in 0..n_groups {
            let d = g_lang_off + gi * lang_size;
            w16(&mut rsrc, d + 12, 0);
            w16(&mut rsrc, d + 14, 1);
            w32(&mut rsrc, d + 16, 0x409);
            w32(&mut rsrc, d + 20, (data_entries_off + gi * 16) as u32);
        }
        for k in 0..n_icons {
            let d = i_lang_off + k * lang_size;
            w16(&mut rsrc, d + 12, 0);
            w16(&mut rsrc, d + 14, 1);
            w32(&mut rsrc, d + 16, 0x409);
            w32(
                &mut rsrc,
                d + 20,
                (data_entries_off + (n_groups + k) * 16) as u32,
            );
        }
        // Data entries: (RVA, size) of the raw bytes.
        let mut gi = 0;
        for (raw_i, len) in group_raw_off
            .iter()
            .map(|o| raw_off + o)
            .zip(groups.iter().map(|(_, e)| 6 + e.len() * 14))
        {
            let d = data_entries_off + gi * 16;
            w32(&mut rsrc, d, (0x1000 + raw_i) as u32);
            w32(&mut rsrc, d + 4, len as u32);
            gi += 1;
        }
        let mut k = 0;
        for (_, entries) in groups {
            for (_, _, _, _, bytes) in entries {
                let d = data_entries_off + (n_groups + k) * 16;
                w32(&mut rsrc, d, (0x1000 + raw_off + icon_raw_off[k]) as u32);
                w32(&mut rsrc, d + 4, bytes.len() as u32);
                k += 1;
            }
        }
        rsrc[raw_off..].copy_from_slice(&raw);
        // Headers: DOS + NT + one .rsrc section at RVA 0x1000,
        // file offset 0x200.
        let mut pe = vec![0u8; 0x200];
        pe[0..2].copy_from_slice(b"MZ");
        pe[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        pe[0x80..0x84].copy_from_slice(b"PE\0\0");
        pe[0x86..0x88].copy_from_slice(&1u16.to_le_bytes()); // 1 section
        pe[0x94..0x96].copy_from_slice(&240u16.to_le_bytes()); // opt size
        pe[0x98..0x9A].copy_from_slice(&0x20Bu16.to_le_bytes()); // PE32+
        pe[0x98 + 60..0x98 + 64].copy_from_slice(&0x200u32.to_le_bytes()); // SizeOfHeaders
        pe[0x98 + 108..0x98 + 112].copy_from_slice(&16u32.to_le_bytes()); // NumberOfRvaAndSizes
        pe[0x98 + 112 + 16..0x98 + 112 + 20].copy_from_slice(&0x1000u32.to_le_bytes()); // rsrc RVA
        pe[0x98 + 112 + 20..0x98 + 112 + 24].copy_from_slice(&(rsrc_len as u32).to_le_bytes()); // rsrc size
        let sect = 0x80 + 24 + 240;
        pe[sect..sect + 5].copy_from_slice(b".rsrc");
        pe[sect + 8..sect + 12].copy_from_slice(&(rsrc_len as u32).to_le_bytes()); // VirtualSize
        pe[sect + 12..sect + 16].copy_from_slice(&0x1000u32.to_le_bytes()); // VirtualAddress
        pe[sect + 16..sect + 20].copy_from_slice(&(rsrc_len as u32).to_le_bytes()); // SizeOfRawData
        pe[sect + 20..sect + 24].copy_from_slice(&0x200u32.to_le_bytes()); // PointerToRawData
        pe.extend_from_slice(&rsrc);
        pe
    }

    #[test]
    fn is_png_checks_the_magic_signature() {
        assert!(is_png(&fake_png(256, 256)));
        assert!(!is_png(b"BM........"));
        assert!(!is_png(b""));
        assert!(!is_png(&[0x89, 0x50, 0x4E, 0x47]));
    }

    #[test]
    fn png_dimensions_reads_ihdr() {
        assert_eq!(png_dimensions(&fake_png(256, 128)), Some((256, 128)));
        assert_eq!(png_dimensions(&fake_png(64, 64)), Some((64, 64)));
        assert_eq!(png_dimensions(b"BM........"), None);
        assert_eq!(png_dimensions(&fake_png(256, 256)[..20]), None);
    }

    #[test]
    fn largest_png_entry_is_picked() {
        let big = fake_png(256, 256);
        let small_bmp = vec![b'B', b'M', 0, 0, 0, 0];
        let pe = build_pe(&[(
            1,
            vec![(0, 0, 32, 100, big.clone()), (32, 32, 24, 101, small_bmp)],
        )]);
        assert_eq!(extract_largest_icon_png(&pe), Some(big));
    }

    #[test]
    fn bmp_only_icons_return_none() {
        let bmp = vec![b'B', b'M', 0, 0, 0, 0];
        let pe = build_pe(&[(1, vec![(0, 0, 32, 100, bmp)])]);
        assert_eq!(extract_largest_icon_png(&pe), None);
    }

    #[test]
    fn largest_png_entry_is_picked_over_larger_bmp() {
        // The largest entry is BMP-only: the largest PNG is
        // promoted instead — a usable icon beats no icon.
        let bmp = vec![b'B', b'M', 0, 0, 0, 0];
        let png64 = fake_png(64, 64);
        let pe = build_pe(&[(
            1,
            vec![(0, 0, 32, 100, bmp), (64, 64, 32, 101, png64.clone())],
        )]);
        assert_eq!(extract_largest_icon_png(&pe), Some(png64));
    }

    #[test]
    fn too_small_png_returns_none_and_64px_passes() {
        let pe = build_pe(&[(1, vec![(32, 32, 32, 100, fake_png(32, 32))])]);
        assert_eq!(extract_largest_icon_png(&pe), None);
        let pe64 = build_pe(&[(1, vec![(64, 64, 32, 100, fake_png(64, 64))])]);
        assert!(extract_largest_icon_png(&pe64).is_some());
    }

    #[test]
    fn malformed_pe_returns_none() {
        assert_eq!(extract_largest_icon_png(b""), None);
        assert_eq!(extract_largest_icon_png(b"not a pe file"), None);
        assert_eq!(extract_largest_icon_png(b"MZ"), None);
        // MZ with an e_lfanew pointing past the end.
        let mut bad = vec![0u8; 64];
        bad[0..2].copy_from_slice(b"MZ");
        bad[0x3C..0x40].copy_from_slice(&0x1000u32.to_le_bytes());
        assert_eq!(extract_largest_icon_png(&bad), None);
        // Valid headers but no resource directory (rsrc RVA 0).
        let mut pe = build_pe(&[(1, vec![(0, 0, 32, 100, fake_png(256, 256))])]);
        pe[0x98 + 112 + 16..0x98 + 112 + 20].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(extract_largest_icon_png(&pe), None);
        // No groups at all.
        assert_eq!(extract_largest_icon_png(&build_pe(&[])), None);
    }

    #[test]
    fn truncated_resource_returns_none_without_panicking() {
        let pe = build_pe(&[(1, vec![(0, 0, 32, 100, fake_png(256, 256))])]);
        // Cut at several points: inside the headers, inside the
        // directories, inside the icon bytes.
        for cut in [10, 0x100, 0x1B0, 0x200 + 20, 0x200 + 60, pe.len() - 5] {
            assert_eq!(
                extract_largest_icon_png(&pe[..cut]),
                None,
                "cut at {cut} should return None"
            );
        }
    }
}
