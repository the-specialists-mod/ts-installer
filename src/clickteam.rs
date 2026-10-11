//! Extractor for Clickteam Install Creator installers, the format of the TS 3.0 client
//! installer (`ts-3-final.exe`).
//!
//! The installer is a small PE stub followed by an overlay that starts with the signature
//! `wwgT)H`. The overlay is a sequence of blocks (`u16 id, u16 ?, u32 size`): `0x143A` is the
//! compressed file list and `0x7F7F` holds every file's data. Format notes are from Bioruebe's
//! cicdec (<https://github.com/Bioruebe/cicdec>); only the version 4.0 node layout is
//! implemented because that is what TS 3.0 uses.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use bzip2::read::BzDecoder;
use flate2::read::{DeflateDecoder, ZlibDecoder};

const SIGNATURE: &[u8] = b"wwgT)H";
const BLOCK_FILE_LIST: u16 = 0x143A;
const BLOCK_FILE_DATA: u16 = 0x7F7F;

/// One file stored in the installer.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Path as stored, with Windows separators, relative to the install directory.
    pub path: String,
    pub offset: u32,
    pub compressed_size: u32,
    pub size: u32,
}

impl Entry {
    /// The stored path as a relative path, or `None` when it would escape the target directory.
    pub fn relative_path(&self) -> Option<PathBuf> {
        let mut out = PathBuf::new();
        for part in self.path.split(['\\', '/']).filter(|p| !p.is_empty() && *p != ".") {
            let c = Path::new(part).components().next()?;
            if !matches!(c, Component::Normal(_)) || part.contains(':') {
                return None;
            }
            out.push(part);
        }
        (!out.as_os_str().is_empty()).then_some(out)
    }
}

pub struct Installer {
    reader: BufReader<File>,
    data_start: u64,
    pub entries: Vec<Entry>,
}

impl Installer {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        let len = file.metadata()?.len();
        let mut reader = BufReader::with_capacity(1 << 20, file);
        let mut pos = find_signature(&mut reader, len)? + SIGNATURE.len() as u64;

        let mut list = None;
        let mut data_start = None;
        while pos + 8 <= len {
            reader.seek(SeekFrom::Start(pos))?;
            let id = read_u16(&mut reader)?;
            read_u16(&mut reader)?;
            let size = read_u32(&mut reader)? as u64;
            pos += 8;
            match id {
                BLOCK_FILE_DATA => data_start = Some(pos),
                BLOCK_FILE_LIST => list = Some(unpack_block(&mut reader, size)?),
                _ => {}
            }
            pos += size;
        }
        let list = list.context("installer has no file list block")?;
        let data_start = data_start.context("installer has no data block (external .D01 data files are not supported)")?;
        let entries = parse_file_list(&list)?;
        Ok(Self { reader, data_start, entries })
    }

    /// Total uncompressed size of all entries.
    pub fn total_size(&self) -> u64 {
        self.entries.iter().map(|e| e.size as u64).sum()
    }

    /// Decompresses `entry` into `out`. Deflated entries are zlib streams, so the Adler-32
    /// trailer checks the data.
    pub fn extract(&mut self, entry: &Entry, out: &mut impl Write) -> Result<()> {
        if entry.size == 0 {
            return Ok(());
        }
        self.reader.seek(SeekFrom::Start(self.data_start + 4 + entry.offset as u64))?;
        let method = read_u8(&mut self.reader)?;
        let raw = (&mut self.reader).take(entry.compressed_size as u64);
        let copied = match method {
            0 => io::copy(&mut raw.take(entry.size as u64), out)?,
            1 => {
                let mut decoder = ZlibDecoder::new(raw);
                let n = io::copy(&mut (&mut decoder).take(entry.size as u64), out)?;
                // Read to the end of the stream so the decoder checks the Adler-32 trailer.
                io::copy(&mut decoder, &mut io::sink()).with_context(|| format!("{}: corrupt data", entry.path))?;
                n
            }
            2 => io::copy(&mut BzDecoder::new(raw).take(entry.size as u64), out)?,
            m => bail!("{}: unsupported compression method {m}", entry.path),
        };
        ensure!(copied == entry.size as u64, "{}: short read ({copied} of {} bytes)", entry.path, entry.size);
        Ok(())
    }

    /// Extracts every entry under `dest`. `filter` selects entries by relative path; `progress`
    /// receives the uncompressed bytes written so far.
    pub fn extract_all(
        &mut self,
        dest: &Path,
        mut filter: impl FnMut(&Path) -> bool,
        mut progress: impl FnMut(u64, &Path) -> Result<()>,
    ) -> Result<usize> {
        let mut done = 0u64;
        let mut count = 0;
        for entry in self.entries.clone() {
            let Some(rel) = entry.relative_path() else {
                log::warn!("skipping unsafe path {:?}", entry.path);
                continue;
            };
            if !filter(&rel) {
                continue;
            }
            let target = dest.join(&rel);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut file = io::BufWriter::new(File::create(&target).with_context(|| format!("create {}", target.display()))?);
            self.extract(&entry, &mut file)?;
            file.flush()?;
            done += entry.size as u64;
            count += 1;
            progress(done, &rel)?;
        }
        Ok(count)
    }
}

/// Finds the overlay signature: first where the PE headers say the overlay starts, then by
/// scanning the whole file.
fn find_signature(reader: &mut BufReader<File>, len: u64) -> Result<u64> {
    if let Some(at) = pe_overlay_offset(reader).ok().flatten() {
        let mut sig = [0u8; 6];
        reader.seek(SeekFrom::Start(at))?;
        if reader.read_exact(&mut sig).is_ok() && sig == SIGNATURE {
            return Ok(at);
        }
    }
    reader.seek(SeekFrom::Start(0))?;
    let mut buf = vec![0u8; 1 << 20];
    let mut base = 0u64;
    let mut carry = 0usize;
    while base < len {
        let n = reader.read(&mut buf[carry..])?;
        if n == 0 {
            break;
        }
        let filled = carry + n;
        if let Some(i) = buf[..filled].windows(SIGNATURE.len()).position(|w| w == SIGNATURE) {
            return Ok(base + i as u64 - carry as u64);
        }
        let tail = filled.saturating_sub(SIGNATURE.len() - 1);
        buf.copy_within(tail..filled, 0);
        base += n as u64;
        carry = filled - tail;
    }
    bail!("not a Clickteam Install Creator installer (signature not found)")
}

fn pe_overlay_offset(reader: &mut BufReader<File>) -> Result<Option<u64>> {
    reader.seek(SeekFrom::Start(0))?;
    if read_u16(reader)? != 0x5A4D {
        return Ok(None);
    }
    reader.seek(SeekFrom::Start(0x3C))?;
    let pe = read_u32(reader)? as u64;
    reader.seek(SeekFrom::Start(pe + 6))?;
    let sections = read_u16(reader)? as u64;
    reader.seek(SeekFrom::Start(pe + 20))?;
    let optional = read_u16(reader)? as u64;
    let table = pe + 24 + optional;
    let mut end = 0;
    for i in 0..sections {
        reader.seek(SeekFrom::Start(table + i * 40 + 16))?;
        let raw_size = read_u32(reader)? as u64;
        let raw_ptr = read_u32(reader)? as u64;
        end = end.max(raw_ptr + raw_size);
    }
    Ok(Some(end))
}

/// Reads a block body: `u32 uncompressed size, u8 method, payload`.
fn unpack_block(reader: &mut impl Read, size: u64) -> Result<Vec<u8>> {
    ensure!(size >= 5, "block too small");
    let uncompressed = read_u32(reader)? as usize;
    let method = read_u8(reader)?;
    let mut payload = vec![0u8; (size - 5) as usize];
    reader.read_exact(&mut payload)?;
    let mut out = Vec::with_capacity(uncompressed);
    match method {
        0 => out = payload,
        1 => {
            ensure!(payload.len() >= 2, "truncated zlib block");
            DeflateDecoder::new(&payload[2..]).take(uncompressed as u64).read_to_end(&mut out)?;
        }
        2 => {
            BzDecoder::new(&payload[..]).take(uncompressed as u64).read_to_end(&mut out)?;
        }
        m => bail!("unsupported block compression method {m}"),
    }
    ensure!(out.len() == uncompressed, "block decompressed to {} bytes, expected {uncompressed}", out.len());
    Ok(out)
}

/// Parses the version 4.0 file list. Each node is `u32 size, u16 type, ...`; type 0 is a file.
/// File nodes hold a 32-bit checksum of unknown kind at +20 (not CRC32), sizes and data offset at +24, three FILETIMEs at +40 and the
/// NUL-terminated path at +64. Empty files have a short node (flag byte 0xE2 at +9) and no data.
fn parse_file_list(list: &[u8]) -> Result<Vec<Entry>> {
    ensure!(list.len() >= 4, "file list too small");
    let count = u16::from_le_bytes([list[0], list[1]]) as usize;
    let mut pos = 4;
    let mut entries = Vec::new();
    for i in 0..count {
        let node = list.get(pos..pos + 6).with_context(|| format!("file list truncated at node {i}"))?;
        let size = u32::from_le_bytes(node[..4].try_into().unwrap()) as usize;
        let kind = u16::from_le_bytes(node[4..6].try_into().unwrap());
        let body = list.get(pos..pos + size).with_context(|| format!("node {i} overruns the file list"))?;
        pos += size;
        ensure!(size >= 10, "node {i} too small");
        if kind != 0 {
            continue;
        }
        let u32_at = |o: usize| body.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
        let (path_at, entry) = if body[9] == 0xE2 {
            (40, Entry { path: String::new(), offset: 0, compressed_size: 0, size: 0 })
        } else {
            let field = |o| u32_at(o).with_context(|| format!("node {i} truncated"));
            (64, Entry { path: String::new(), size: field(24)?, offset: field(28)?, compressed_size: field(32)? })
        };
        let raw = body.get(path_at..).unwrap_or_default();
        let raw = &raw[..raw.iter().position(|&b| b == 0).unwrap_or(raw.len())];
        // Paths are in the Windows ANSI code page; every TS path is ASCII.
        let path = raw.iter().map(|&b| b as char).collect();
        entries.push(Entry { path, ..entry });
    }
    Ok(entries)
}

fn read_u8(r: &mut impl Read) -> io::Result<u8> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b)?;
    Ok(b[0])
}

fn read_u16(r: &mut impl Read) -> io::Result<u16> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_le_bytes(b))
}

fn read_u32(r: &mut impl Read) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_are_sanitized() {
        let e = |p: &str| Entry { path: p.into(), offset: 0, compressed_size: 0, size: 0 };
        assert_eq!(e(r"ts\cl_dlls\client.dll").relative_path(), Some(PathBuf::from("ts/cl_dlls/client.dll")));
        assert_eq!(e(r"..\evil.dll").relative_path(), None);
        assert_eq!(e(r"C:\Windows\x.dll").relative_path(), None);
        assert_eq!(e(r"\ts\.\a.txt").relative_path(), Some(PathBuf::from("ts/a.txt")));
    }

    /// Runs against the real installer when `TS_INSTALLER` points at `ts-3-final.exe`.
    #[test]
    fn ts_installer_lists_and_extracts() {
        let Ok(path) = std::env::var("TS_INSTALLER") else { return };
        let mut inst = Installer::open(Path::new(&path)).unwrap();
        assert!(inst.entries.len() > 1000);
        let client = inst.entries.iter().find(|e| e.path.eq_ignore_ascii_case(r"ts\cl_dlls\client.dll")).unwrap().clone();
        let mut buf = Vec::new();
        inst.extract(&client, &mut buf).unwrap();
        assert_eq!(&buf[..2], b"MZ");
    }
}
