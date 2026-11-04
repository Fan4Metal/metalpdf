//! A ZIP archive written as it goes, which is all an Office file is: each
//! entry deflated (XML) or stored (PNG, already compressed) in memory,
//! then its local header and data written; the central directory at the
//! end. Names are UTF-8 (flag bit 11). No ZIP64: an archive past 4 GiB is
//! an error.

use std::io::{self, Write};

use flate2::Compression;
use flate2::write::DeflateEncoder;

struct Entry {
    name: String,
    crc: u32,
    compressed: u32,
    size: u32,
    method: u16,
    offset: u32,
}

pub struct ZipWriter<W: Write> {
    out: W,
    offset: u64,
    entries: Vec<Entry>,
}

/// 1980-01-01 00:00, the first date DOS times know.
const DOS_DATE: u16 = 0x0021;
const UTF8_NAMES: u16 = 1 << 11;

fn too_large() -> io::Error {
    io::Error::other("the file would pass 4 GiB")
}

impl<W: Write> ZipWriter<W> {
    pub fn new(out: W) -> ZipWriter<W> {
        ZipWriter { out, offset: 0, entries: Vec::new() }
    }

    /// Add `data` as `name`; `deflate` compresses it.
    pub fn add(&mut self, name: &str, data: &[u8], deflate: bool) -> io::Result<()> {
        let mut crc = flate2::Crc::new();
        crc.update(data);
        let compressed_data;
        let (body, method): (&[u8], u16) = if deflate {
            let mut encoder = DeflateEncoder::new(Vec::with_capacity(data.len() / 3), Compression::default());
            encoder.write_all(data)?;
            compressed_data = encoder.finish()?;
            (&compressed_data, 8)
        } else {
            (data, 0)
        };
        let size = u32::try_from(data.len()).map_err(|_| too_large())?;
        let compressed = u32::try_from(body.len()).map_err(|_| too_large())?;
        let offset = u32::try_from(self.offset).map_err(|_| too_large())?;
        let entry = Entry { name: name.to_owned(), crc: crc.sum(), compressed, size, method, offset };
        let mut header = Vec::with_capacity(30 + name.len());
        header.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        header.extend_from_slice(&20u16.to_le_bytes());
        header.extend_from_slice(&UTF8_NAMES.to_le_bytes());
        header.extend_from_slice(&method.to_le_bytes());
        header.extend_from_slice(&0u16.to_le_bytes());
        header.extend_from_slice(&DOS_DATE.to_le_bytes());
        header.extend_from_slice(&entry.crc.to_le_bytes());
        header.extend_from_slice(&compressed.to_le_bytes());
        header.extend_from_slice(&size.to_le_bytes());
        header.extend_from_slice(&(name.len() as u16).to_le_bytes());
        header.extend_from_slice(&0u16.to_le_bytes());
        header.extend_from_slice(name.as_bytes());
        self.out.write_all(&header)?;
        self.out.write_all(body)?;
        self.offset += header.len() as u64 + body.len() as u64;
        self.entries.push(entry);
        Ok(())
    }

    /// The central directory written; the writer given back, flushed.
    pub fn finish(mut self) -> io::Result<W> {
        let start = u32::try_from(self.offset).map_err(|_| too_large())?;
        let mut dir = Vec::new();
        for e in &self.entries {
            dir.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            dir.extend_from_slice(&20u16.to_le_bytes());
            dir.extend_from_slice(&20u16.to_le_bytes());
            dir.extend_from_slice(&UTF8_NAMES.to_le_bytes());
            dir.extend_from_slice(&e.method.to_le_bytes());
            dir.extend_from_slice(&0u16.to_le_bytes());
            dir.extend_from_slice(&DOS_DATE.to_le_bytes());
            dir.extend_from_slice(&e.crc.to_le_bytes());
            dir.extend_from_slice(&e.compressed.to_le_bytes());
            dir.extend_from_slice(&e.size.to_le_bytes());
            dir.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
            // Extra field, comment, disk, internal and external attributes.
            dir.extend_from_slice(&[0u8; 12]);
            dir.extend_from_slice(&e.offset.to_le_bytes());
            dir.extend_from_slice(e.name.as_bytes());
        }
        let count = u16::try_from(self.entries.len()).map_err(|_| io::Error::other("too many entries"))?;
        let dir_len = u32::try_from(dir.len()).map_err(|_| too_large())?;
        start.checked_add(dir_len).ok_or_else(too_large)?;
        dir.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        dir.extend_from_slice(&[0u8; 4]);
        dir.extend_from_slice(&count.to_le_bytes());
        dir.extend_from_slice(&count.to_le_bytes());
        dir.extend_from_slice(&dir_len.to_le_bytes());
        dir.extend_from_slice(&start.to_le_bytes());
        dir.extend_from_slice(&0u16.to_le_bytes());
        self.out.write_all(&dir)?;
        self.out.flush()?;
        Ok(self.out)
    }
}

/// Entry `name` of an archive, read through its central directory and
/// inflated: for the tests.
#[cfg(test)]
pub fn read_entry(bytes: &[u8], name: &str) -> Option<Vec<u8>> {
    let end = bytes.len().checked_sub(22)?;
    let le16 = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]) as usize;
    let le32 = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
    let mut at = le32(end + 16);
    for _ in 0..le16(end + 10) {
        let (method, csize, name_len, offset) = (le16(at + 10), le32(at + 20), le16(at + 28), le32(at + 42));
        if &bytes[at + 46..at + 46 + name_len] == name.as_bytes() {
            let data_at = offset + 30 + le16(offset + 26);
            let body = &bytes[data_at..data_at + csize];
            if method == 0 {
                return Some(body.to_vec());
            }
            let mut out = Vec::new();
            std::io::Read::read_to_end(&mut flate2::read::DeflateDecoder::new(body), &mut out).ok()?;
            return Some(out);
        }
        at += 46 + name_len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The entries read back by walking the central directory: names,
    /// methods and the data inflated again.
    #[test]
    fn an_archive_reads_back() {
        let mut zip = ZipWriter::new(Vec::new());
        let xml = "<a>Привет, мир</a>".repeat(50);
        zip.add("word/document.xml", xml.as_bytes(), true).unwrap();
        zip.add("media/имя.png", b"\x89PNG raw", false).unwrap();
        let bytes = zip.finish().unwrap();
        let end = bytes.len() - 22;
        assert_eq!(&bytes[end..end + 4], &0x0605_4b50u32.to_le_bytes());
        let le16 = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]) as usize;
        let le32 = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        assert_eq!(le16(end + 10), 2);
        let mut at = le32(end + 16);
        let mut read = Vec::new();
        for _ in 0..2 {
            assert_eq!(le32(at), 0x0201_4b50);
            let (method, csize, size, name_len, offset) = (le16(at + 10), le32(at + 20), le32(at + 24), le16(at + 28), le32(at + 42));
            let name = String::from_utf8(bytes[at + 46..at + 46 + name_len].to_vec()).unwrap();
            let data_at = offset + 30 + le16(offset + 26);
            let body = &bytes[data_at..data_at + csize];
            let data = if method == 8 {
                let mut out = Vec::new();
                std::io::Read::read_to_end(&mut flate2::read::DeflateDecoder::new(body), &mut out).unwrap();
                out
            } else {
                body.to_vec()
            };
            assert_eq!(data.len(), size);
            read.push((name, data));
            at += 46 + name_len;
        }
        assert_eq!(read[0], ("word/document.xml".to_owned(), xml.into_bytes()));
        assert_eq!(read[1], ("media/имя.png".to_owned(), b"\x89PNG raw".to_vec()));
    }
}
