//! Anvil `.mca` region-file container: the 32×32-chunk file holding up to 1024
//! compressed chunk payloads behind a two-table header.
//!
//! Reference: decompiled `RegionFile` / `RegionFileStorage` / `RegionFileVersion`
//! (MC 26.2). Reimplemented from the observed on-disk layout, not copied.
//!
//! # Layout
//!
//! The file is a sequence of 4096-byte *sectors*. The first two sectors are the
//! header:
//!   * sector 0 — 1024 big-endian `u32` **location** entries, one per chunk. A
//!     location packs `sectorNumber << 8 | sectorCount`; `0` means "absent".
//!   * sector 1 — 1024 big-endian `u32` **timestamps** (epoch seconds), parallel
//!     to the locations.
//!
//! Each present chunk starts at `sectorNumber * 4096` with a 5-byte stream
//! header: a big-endian `u32` `length` (the byte count that follows, including
//! the one version byte) and a `u8` `version` (1 = gzip, 2 = zlib/deflate,
//! 3 = uncompressed). The remaining `length - 1` bytes are the compressed NBT.
//!
//! We write with zlib (version 2), vanilla's `RegionFileVersion.DEFAULT`. The
//! oversized-chunk `.mcc` external-file path (chunks needing ≥256 sectors, i.e.
//! ≥1 MiB compressed) is **not** implemented — our chunks are a few KiB — and a
//! chunk that large is rejected rather than silently split.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use flate2::read::{GzDecoder, ZlibDecoder};
use flate2::write::ZlibEncoder;
use flate2::Compression;

/// Bytes per sector — the file's allocation granularity.
const SECTOR_BYTES: usize = 4096;
/// Location/timestamp entries per table (one per chunk in a 32×32 region).
const ENTRIES: usize = 1024;
/// Bytes in the two-table header (two full sectors).
const HEADER_BYTES: usize = 2 * SECTOR_BYTES;
/// The 5-byte per-chunk stream header (`u32` length + `u8` version).
const CHUNK_HEADER: usize = 5;

/// Region-file compression tags (`RegionFileVersion` ids).
const VERSION_GZIP: u8 = 1;
const VERSION_ZLIB: u8 = 2;
const VERSION_NONE: u8 = 3;
/// The high bit of the version byte flags an external `.mcc` payload.
const EXTERNAL_FLAG: u8 = 0x80;

/// A `sectorsNeeded >= 256` chunk goes to an external file in vanilla; we cap
/// well under that (1 MiB) and refuse rather than implement the `.mcc` path.
const MAX_SECTORS: usize = 256;

/// An open Anvil region file: the parsed header plus a free-sector bitmap, over a
/// read/write file handle. One instance maps one `r.<rx>.<rz>.mca`.
pub struct RegionFile {
    file: File,
    /// Packed location entries (`sectorNumber << 8 | sectorCount`), index
    /// `localX + localZ * 32`.
    locations: [u32; ENTRIES],
    /// Parallel epoch-second timestamps.
    timestamps: [u32; ENTRIES],
    /// `used[i]` marks sector `i` as allocated. Grows as the file does; sectors
    /// past its end are implicitly free.
    used: Vec<bool>,
}

impl RegionFile {
    /// Open (creating if absent) the region file at `path`, parsing its header
    /// and reconstructing the free-sector bitmap. A freshly created file gets an
    /// all-zero header written on the first `write_chunk`.
    pub fn open(path: &Path) -> io::Result<Self> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false) // keep an existing region file's contents
            .open(path)?;

        let len = file.metadata()?.len() as usize;
        let mut locations = [0u32; ENTRIES];
        let mut timestamps = [0u32; ENTRIES];
        // Sectors 0 and 1 are the header; always occupied.
        let sector_count = len.div_ceil(SECTOR_BYTES).max(2);
        let mut used = vec![false; sector_count];
        used[0] = true;
        used[1] = true;

        if len >= HEADER_BYTES {
            let mut header = [0u8; HEADER_BYTES];
            file.seek(SeekFrom::Start(0))?;
            file.read_exact(&mut header)?;
            for i in 0..ENTRIES {
                let loc = u32::from_be_bytes(header[i * 4..i * 4 + 4].try_into().unwrap());
                let ts_off = SECTOR_BYTES + i * 4;
                timestamps[i] =
                    u32::from_be_bytes(header[ts_off..ts_off + 4].try_into().unwrap());
                if loc == 0 {
                    continue;
                }
                let (sector, count) = unpack_location(loc);
                // Discard entries that overlap the header or run past the file,
                // exactly as vanilla does on load.
                if sector < 2 || count == 0 || (sector + count) * SECTOR_BYTES > len.max(HEADER_BYTES) {
                    continue;
                }
                locations[i] = loc;
                mark_used(&mut used, sector, count);
            }
        }

        Ok(Self {
            file,
            locations,
            timestamps,
            used,
        })
    }

    /// Read chunk `(local_x, local_z)`'s NBT bytes (decompressed), or `None` if
    /// the slot is empty. `local_*` are the chunk's coordinates within the region
    /// (`chunk & 31`).
    pub fn read_chunk(&mut self, local_x: usize, local_z: usize) -> io::Result<Option<Vec<u8>>> {
        let loc = self.locations[index(local_x, local_z)];
        if loc == 0 {
            return Ok(None);
        }
        let (sector, count) = unpack_location(loc);
        self.file.seek(SeekFrom::Start((sector * SECTOR_BYTES) as u64))?;
        let mut header = [0u8; CHUNK_HEADER];
        self.file.read_exact(&mut header)?;
        let length = u32::from_be_bytes(header[0..4].try_into().unwrap()) as usize;
        let version = header[4];
        if length == 0 {
            return Ok(None);
        }
        if version & EXTERNAL_FLAG != 0 {
            // Oversized external `.mcc` payloads are unsupported; treat as absent.
            return Ok(None);
        }
        let stream_len = length - 1; // the version byte is counted in `length`
        if stream_len > count * SECTOR_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "region chunk stream truncated",
            ));
        }
        let mut compressed = vec![0u8; stream_len];
        self.file.read_exact(&mut compressed)?;
        Ok(Some(decompress(version, &compressed)?))
    }

    /// Write chunk `(local_x, local_z)`'s NBT `data`, compressing with zlib and
    /// (re)allocating sectors. The old sectors, if any, are freed.
    pub fn write_chunk(&mut self, local_x: usize, local_z: usize, data: &[u8]) -> io::Result<()> {
        let compressed = compress(data);
        let payload_len = CHUNK_HEADER + compressed.len();
        let sectors_needed = payload_len.div_ceil(SECTOR_BYTES);
        if sectors_needed >= MAX_SECTORS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "chunk too large for inline region storage (external files unsupported)",
            ));
        }

        let idx = index(local_x, local_z);
        let old = self.locations[idx];
        let sector = self.allocate(sectors_needed);

        // A full-sector-padded buffer: 5-byte header, the zlib stream, then zero
        // padding to the sector boundary.
        let mut buf = vec![0u8; sectors_needed * SECTOR_BYTES];
        let length = (compressed.len() + 1) as u32; // +1 for the version byte
        buf[0..4].copy_from_slice(&length.to_be_bytes());
        buf[4] = VERSION_ZLIB;
        buf[CHUNK_HEADER..payload_len].copy_from_slice(&compressed);
        self.file.seek(SeekFrom::Start((sector * SECTOR_BYTES) as u64))?;
        self.file.write_all(&buf)?;

        self.locations[idx] = pack_location(sector, sectors_needed);
        self.timestamps[idx] = now_seconds();
        self.write_header()?;

        // Free the previous allocation only after the new data + header are on disk.
        if old != 0 {
            let (old_sector, old_count) = unpack_location(old);
            mark_free(&mut self.used, old_sector, old_count);
        }
        Ok(())
    }

    /// Whether chunk `(local_x, local_z)` has a stored payload.
    #[allow(dead_code)]
    pub fn has_chunk(&self, local_x: usize, local_z: usize) -> bool {
        self.locations[index(local_x, local_z)] != 0
    }

    /// Flush buffered writes to the OS.
    pub fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }

    /// Write the 8192-byte header (locations then timestamps) back to disk.
    fn write_header(&mut self) -> io::Result<()> {
        let mut header = [0u8; HEADER_BYTES];
        for i in 0..ENTRIES {
            header[i * 4..i * 4 + 4].copy_from_slice(&self.locations[i].to_be_bytes());
            let ts_off = SECTOR_BYTES + i * 4;
            header[ts_off..ts_off + 4].copy_from_slice(&self.timestamps[i].to_be_bytes());
        }
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&header)?;
        Ok(())
    }

    /// Reserve the first run of `size` contiguous free sectors, extending the
    /// bitmap (and thus the file, on the next write) when none fits. Mirrors
    /// vanilla `RegionBitmap.allocate`: first-fit from sector 2 up.
    fn allocate(&mut self, size: usize) -> usize {
        let mut run_start = 2;
        let mut run = 0;
        let mut i = 2;
        while i < self.used.len() {
            if self.used[i] {
                run = 0;
                run_start = i + 1;
            } else {
                run += 1;
                if run == size {
                    mark_used(&mut self.used, run_start, size);
                    return run_start;
                }
            }
            i += 1;
        }
        // No interior gap fit: allocate at the tail, growing the bitmap.
        let start = run_start.max(2);
        mark_used(&mut self.used, start, size);
        start
    }
}

/// Ensure `used` covers `[sector, sector + count)` and mark it allocated.
fn mark_used(used: &mut Vec<bool>, sector: usize, count: usize) {
    if used.len() < sector + count {
        used.resize(sector + count, false);
    }
    for s in used.iter_mut().take(sector + count).skip(sector) {
        *s = true;
    }
}

/// Mark `[sector, sector + count)` free (leaves the bitmap length unchanged).
fn mark_free(used: &mut [bool], sector: usize, count: usize) {
    for s in used.iter_mut().skip(sector).take(count) {
        *s = false;
    }
}

/// The header index of a chunk within its region (`localX + localZ * 32`).
fn index(local_x: usize, local_z: usize) -> usize {
    local_x + local_z * 32
}

/// Split a packed location into `(sectorNumber, sectorCount)`.
fn unpack_location(loc: u32) -> (usize, usize) {
    ((loc >> 8) as usize, (loc & 0xFF) as usize)
}

/// Pack `(sectorNumber, sectorCount)` into a location entry.
fn pack_location(sector: usize, count: usize) -> u32 {
    ((sector as u32) << 8) | (count as u32 & 0xFF)
}

/// Current time in epoch seconds, truncated to `u32` like the region timestamp.
fn now_seconds() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0)
}

/// zlib-deflate the chunk NBT (version 2). Default level (6) matches Java's
/// `Deflater` default, as used elsewhere in the codec.
fn compress(data: &[u8]) -> Vec<u8> {
    let mut enc = ZlibEncoder::new(Vec::with_capacity(data.len() / 2 + 16), Compression::default());
    enc.write_all(data).expect("zlib deflate into Vec is infallible");
    enc.finish().expect("zlib finish into Vec is infallible")
}

/// Decompress a stored chunk stream by its version tag.
fn decompress(version: u8, data: &[u8]) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    match version {
        VERSION_ZLIB => {
            ZlibDecoder::new(data).read_to_end(&mut out)?;
        }
        VERSION_GZIP => {
            GzDecoder::new(data).read_to_end(&mut out)?;
        }
        VERSION_NONE => out.extend_from_slice(data),
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown region compression version {other}"),
            ))
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_region() -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "vela-region-{}-{}.mca",
            std::process::id(),
            // A per-call nonce so parallel tests don't collide.
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        p
    }

    #[test]
    fn location_packing_round_trips() {
        for (sector, count) in [(2, 1), (5, 3), (16_777_215, 255)] {
            assert_eq!(unpack_location(pack_location(sector, count)), (sector, count));
        }
    }

    #[test]
    fn write_then_read_round_trips() {
        let path = temp_region();
        let payload = b"hello anvil world, this is chunk NBT".to_vec();
        {
            let mut r = RegionFile::open(&path).unwrap();
            assert!(!r.has_chunk(1, 2));
            r.write_chunk(1, 2, &payload).unwrap();
            assert!(r.has_chunk(1, 2));
            assert_eq!(r.read_chunk(1, 2).unwrap().as_deref(), Some(&payload[..]));
            r.flush().unwrap();
        }
        // Reopen: the header persisted, so the chunk is still there.
        {
            let mut r = RegionFile::open(&path).unwrap();
            assert_eq!(r.read_chunk(1, 2).unwrap(), Some(payload));
            assert_eq!(r.read_chunk(0, 0).unwrap(), None);
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn rewrite_grows_and_frees_sectors() {
        let path = temp_region();
        let mut r = RegionFile::open(&path).unwrap();
        // Small then large then small again, at the same slot: the allocator must
        // relocate and free without corrupting the payload.
        r.write_chunk(0, 0, &[1u8; 100]).unwrap();
        let first = unpack_location(r.locations[0]);
        r.write_chunk(0, 0, &vec![2u8; 20_000]).unwrap();
        let second = unpack_location(r.locations[0]);
        assert_ne!(first.0, second.0, "larger payload relocated");
        assert_eq!(r.read_chunk(0, 0).unwrap(), Some(vec![2u8; 20_000]));
        // The freed first sector is reused by a later small chunk.
        r.write_chunk(3, 3, &[9u8; 50]).unwrap();
        assert_eq!(unpack_location(r.locations[index(3, 3)]).0, first.0);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn many_chunks_do_not_overlap() {
        let path = temp_region();
        let mut r = RegionFile::open(&path).unwrap();
        for lz in 0..4 {
            for lx in 0..4 {
                let data = vec![(lx * 16 + lz) as u8; 200 + lx * 100 + lz * 7];
                r.write_chunk(lx, lz, &data).unwrap();
            }
        }
        for lz in 0..4 {
            for lx in 0..4 {
                let expect = vec![(lx * 16 + lz) as u8; 200 + lx * 100 + lz * 7];
                assert_eq!(r.read_chunk(lx, lz).unwrap(), Some(expect), "({lx},{lz})");
            }
        }
        std::fs::remove_file(&path).ok();
    }
}
