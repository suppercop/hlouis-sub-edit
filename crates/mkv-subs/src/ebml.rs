//! Low-level streaming EBML reader with buffered zero-allocation skipping.

use crate::MkvError;
use std::io::{BufReader, Read, Seek, SeekFrom};

/// Matroska / EBML element IDs used by the subtitle extractor.
pub mod id {
    pub const EBML: u32 = 0x1A45_DFA3;
    pub const VOID: u32 = 0xEC;
    pub const CRC32: u32 = 0xBF;
    pub const SEGMENT: u32 = 0x1853_8067;

    // Top-level Segment children
    pub const SEEK_HEAD: u32 = 0x114D_9B74;
    pub const INFO: u32 = 0x1549_A966;
    pub const TRACKS: u32 = 0x1654_AE6B;
    pub const CLUSTER: u32 = 0x1F43_B675;
    pub const CUES: u32 = 0x1C53_BB6B;
    pub const ATTACHMENTS: u32 = 0x1941_A469;
    pub const CHAPTERS: u32 = 0x1043_A770;
    pub const TAGS: u32 = 0x1254_C367;

    // SeekHead
    pub const SEEK: u32 = 0x4DBB;
    pub const SEEK_ID: u32 = 0x53AB;
    pub const SEEK_POSITION: u32 = 0x53AC;

    // Info
    pub const TIMESTAMP_SCALE: u32 = 0x002A_D7B1;

    // Tracks
    pub const TRACK_ENTRY: u32 = 0xAE;
    pub const TRACK_NUMBER: u32 = 0xD7;
    pub const TRACK_TYPE: u32 = 0x83;
    pub const FLAG_DEFAULT: u32 = 0x88;
    pub const FLAG_FORCED: u32 = 0x55AA;
    pub const NAME: u32 = 0x536E;
    pub const LANGUAGE: u32 = 0x22_B59C;
    pub const LANGUAGE_BCP47: u32 = 0x22_B59D;
    pub const CODEC_ID: u32 = 0x86;
    pub const CODEC_PRIVATE: u32 = 0x63A2;
    pub const CONTENT_ENCODINGS: u32 = 0x6D80;
    pub const CONTENT_ENCODING: u32 = 0x6240;
    pub const CONTENT_ENCODING_ORDER: u32 = 0x5031;
    pub const CONTENT_ENCODING_SCOPE: u32 = 0x5032;
    pub const CONTENT_ENCODING_TYPE: u32 = 0x5033;
    pub const CONTENT_COMPRESSION: u32 = 0x5034;
    pub const CONTENT_COMP_ALGO: u32 = 0x4254;
    pub const CONTENT_COMP_SETTINGS: u32 = 0x4255;

    // Cluster
    pub const CLUSTER_TIMESTAMP: u32 = 0xE7;
    pub const SIMPLE_BLOCK: u32 = 0xA3;
    pub const BLOCK_GROUP: u32 = 0xA0;
    pub const BLOCK: u32 = 0xA1;
    pub const BLOCK_DURATION: u32 = 0x9B;

    // Cues
    pub const CUE_POINT: u32 = 0xBB;
    pub const CUE_TRACK_POSITIONS: u32 = 0xB7;
    pub const CUE_TRACK: u32 = 0xF7;
    pub const CUE_CLUSTER_POSITION: u32 = 0xF1;
}

/// Returns `true` if `element_id` is a top-level Matroska `Segment` child (used to detect the end
/// of unknown-size clusters).
#[must_use]
pub const fn is_segment_top_level_id(element_id: u32) -> bool {
    matches!(
        element_id,
        id::SEEK_HEAD
            | id::INFO
            | id::TRACKS
            | id::CLUSTER
            | id::CUES
            | id::ATTACHMENTS
            | id::CHAPTERS
            | id::TAGS
    )
}

/// Decoded EBML element header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElementHeader {
    /// Raw EBML element ID (including VINT marker bits, matching Matroska spec constants).
    pub id: u32,
    /// Data payload size (`None` means unknown/streaming size).
    pub size: Option<u64>,
    /// Byte offset where this element's header started.
    pub header_pos: u64,
    /// Byte offset where this element's data payload begins.
    pub data_pos: u64,
}

impl ElementHeader {
    /// Byte offset immediately after this element's payload, or `None` if unknown-size.
    #[must_use]
    pub fn end_pos(self) -> Option<u64> {
        self.size.and_then(|s| self.data_pos.checked_add(s))
    }
}

/// Buffered streaming EBML reader tracking exact byte position and stream length.
pub struct EbmlReader<R: Read + Seek> {
    inner: BufReader<R>,
    pos: u64,
    len: u64,
}

impl<R: Read + Seek> EbmlReader<R> {
    /// Wraps `reader` in a 64 KiB `BufReader` and records its total byte length.
    pub fn new(mut reader: R) -> Result<Self, MkvError> {
        let len = reader.seek(SeekFrom::End(0))?;
        let pos = reader.seek(SeekFrom::Start(0))?;
        Ok(Self {
            inner: BufReader::with_capacity(64 * 1024, reader),
            pos,
            len,
        })
    }

    /// Current stream position in bytes.
    #[inline]
    #[must_use]
    pub const fn pos(&self) -> u64 {
        self.pos
    }

    /// Total byte length of the underlying stream.
    #[inline]
    #[must_use]
    pub const fn len(&self) -> u64 {
        self.len
    }

    /// Returns `true` if the underlying stream is empty.
    #[inline]
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Seeks to absolute byte offset `target`.
    pub fn seek_to(&mut self, target: u64) -> Result<(), MkvError> {
        if target == self.pos {
            return Ok(());
        }
        let delta = i64::try_from(target)
            .ok()
            .and_then(|t| i64::try_from(self.pos).ok().and_then(|p| t.checked_sub(p)));
        if let Some(d) = delta {
            self.inner.seek_relative(d)?;
        } else {
            self.inner.seek(SeekFrom::Start(target))?;
        }
        self.pos = target;
        Ok(())
    }

    /// Skips `count` bytes forward without reading or allocating payload buffers.
    pub fn skip(&mut self, count: u64) -> Result<(), MkvError> {
        if count == 0 {
            return Ok(());
        }
        if let Ok(delta) = i64::try_from(count) {
            self.inner.seek_relative(delta)?;
            self.pos = self.pos.saturating_add(count);
        } else {
            let target = self.pos.saturating_add(count);
            self.inner.seek(SeekFrom::Start(target))?;
            self.pos = target;
        }
        Ok(())
    }

    /// Reads exactly `buf.len()` bytes.
    pub fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), MkvError> {
        self.inner.read_exact(buf)?;
        self.pos = self.pos.saturating_add(buf.len() as u64);
        Ok(())
    }

    /// Reads a single byte, returning `Ok(None)` at EOF.
    pub fn read_u8_opt(&mut self) -> Result<Option<u8>, MkvError> {
        if self.pos >= self.len {
            return Ok(None);
        }
        let mut b = [0u8; 1];
        match self.inner.read_exact(&mut b) {
            Ok(()) => {
                self.pos += 1;
                Ok(Some(b[0]))
            }
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(None),
            Err(e) => Err(MkvError::Io(e)),
        }
    }

    /// Reads an EBML element header (ID + VINT size), returning `Ok(None)` at EOF or `limit`.
    pub fn read_header(&mut self, limit: u64) -> Result<Option<ElementHeader>, MkvError> {
        if self.pos >= limit || self.pos >= self.len {
            return Ok(None);
        }
        let header_pos = self.pos;
        let Some(id) = self.read_element_id()? else {
            return Ok(None);
        };
        let size = self.read_vint_size()?;
        let data_pos = self.pos;
        Ok(Some(ElementHeader {
            id,
            size,
            header_pos,
            data_pos,
        }))
    }

    /// Reads a 1-to-4 byte EBML Element ID (keeping marker bits).
    pub fn read_element_id(&mut self) -> Result<Option<u32>, MkvError> {
        let Some(first) = self.read_u8_opt()? else {
            return Ok(None);
        };
        let leading_zeros = first.leading_zeros();
        if leading_zeros > 3 {
            return Err(MkvError::MalformedEbml(format!(
                "invalid EBML element ID leading byte 0x{first:02X} at offset {}",
                self.pos - 1
            )));
        }
        let extra = leading_zeros as usize;
        let mut id = u32::from(first);
        if extra > 0 {
            let mut rest = [0u8; 3];
            self.read_exact(&mut rest[..extra])?;
            for &b in &rest[..extra] {
                id = (id << 8) | u32::from(b);
            }
        }
        Ok(Some(id))
    }

    /// Reads a 1-to-8 byte EBML VINT data size (`None` if unknown size — all data bits set to 1).
    pub fn read_vint_size(&mut self) -> Result<Option<u64>, MkvError> {
        let Some(first) = self.read_u8_opt()? else {
            return Err(MkvError::MalformedEbml(
                "unexpected EOF reading VINT size".to_owned(),
            ));
        };
        let leading_zeros = first.leading_zeros();
        if leading_zeros > 7 {
            return Err(MkvError::MalformedEbml(format!(
                "invalid VINT size byte 0x00 at offset {}",
                self.pos - 1
            )));
        }
        let extra = leading_zeros as usize;
        let mask = if leading_zeros == 7 {
            0u8
        } else {
            (1u8 << (7 - leading_zeros)) - 1
        };
        let mut val = u64::from(first & mask);
        if extra > 0 {
            let mut rest = [0u8; 7];
            self.read_exact(&mut rest[..extra])?;
            for &b in &rest[..extra] {
                val = (val << 8) | u64::from(b);
            }
        }
        let all_ones = (1u64 << (7 * (extra + 1))) - 1;
        if val == all_ones {
            Ok(None)
        } else {
            Ok(Some(val))
        }
    }

    /// Reads an unsigned EBML VINT (used for track numbers inside `Block`/`SimpleBlock`),
    /// returning `(value, bytes_consumed)`.
    pub fn read_vint_uint(&mut self) -> Result<(u64, u64), MkvError> {
        let Some(first) = self.read_u8_opt()? else {
            return Err(MkvError::MalformedEbml(
                "unexpected EOF reading VINT uint".to_owned(),
            ));
        };
        let leading_zeros = first.leading_zeros();
        if leading_zeros > 7 {
            return Err(MkvError::MalformedEbml(
                "invalid VINT uint byte 0x00".to_owned(),
            ));
        }
        let extra = leading_zeros as usize;
        let mask = if leading_zeros == 7 {
            0u8
        } else {
            (1u8 << (7 - leading_zeros)) - 1
        };
        let mut val = u64::from(first & mask);
        if extra > 0 {
            let mut rest = [0u8; 7];
            self.read_exact(&mut rest[..extra])?;
            for &b in &rest[..extra] {
                val = (val << 8) | u64::from(b);
            }
        }
        Ok((val, (extra + 1) as u64))
    }

    /// Reads a big-endian unsigned integer of `size` bytes (`0..=8`).
    pub fn read_uint(&mut self, size: u64) -> Result<u64, MkvError> {
        if size > 8 {
            return Err(MkvError::MalformedEbml(format!(
                "EBML uint size {size} exceeds 8 bytes"
            )));
        }
        let mut buf = [0u8; 8];
        let n = usize::try_from(size).unwrap_or(8);
        if n > 0 {
            self.read_exact(&mut buf[8 - n..])?;
        }
        Ok(u64::from_be_bytes(buf))
    }

    /// Reads `size` bytes into a `Vec<u8>`.
    pub fn read_bytes(&mut self, size: u64) -> Result<Vec<u8>, MkvError> {
        let n = usize::try_from(size)
            .map_err(|_| MkvError::MalformedEbml(format!("payload size {size} too large")))?;
        let mut buf = vec![0u8; n];
        if n > 0 {
            self.read_exact(&mut buf)?;
        }
        Ok(buf)
    }

    /// Reads `size` bytes as a UTF-8 string, trimming trailing NUL bytes per Matroska spec.
    pub fn read_string(&mut self, size: u64) -> Result<String, MkvError> {
        let mut bytes = self.read_bytes(size)?;
        while bytes.last() == Some(&0) {
            bytes.pop();
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}
