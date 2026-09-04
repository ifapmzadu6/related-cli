//! Internal object representations shared by the pack reader.

use crate::AnyResult;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) struct RawObjectId(pub(super) [u8; 20]);

impl RawObjectId {
    pub(super) fn from_hex_str(value: &str) -> AnyResult<Self> {
        Self::from_hex(value.as_bytes())
    }

    pub(super) fn from_hex(value: &[u8]) -> AnyResult<Self> {
        if value.len() != 40 {
            return Err(format!("expected 40 hex bytes, got {}", value.len()).into());
        }
        let mut out = [0u8; 20];
        for (idx, slot) in out.iter_mut().enumerate() {
            *slot = (hex_nibble(value[idx * 2])? << 4) | hex_nibble(value[idx * 2 + 1])?;
        }
        Ok(Self(out))
    }

    pub(super) fn to_hex(self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(40);
        for byte in self.0 {
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
        out
    }
}

fn hex_nibble(byte: u8) -> AnyResult<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(format!("invalid hex byte {:?}", byte as char).into()),
    }
}

pub(super) fn parse_hex_byte(value: &[u8]) -> AnyResult<u8> {
    if value.len() != 2 {
        return Err(format!("expected 2 hex bytes, got {}", value.len()).into());
    }
    Ok((hex_nibble(value[0])? << 4) | hex_nibble(value[1])?)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RawObjectKind {
    Commit,
    Tree,
    Blob,
    Tag,
}

#[derive(Clone, Debug)]
pub(super) struct RawGitObject {
    pub(super) kind: RawObjectKind,
    pub(super) data: Arc<[u8]>,
}

#[derive(Clone, Debug)]
pub(super) struct RawCommit {
    pub(super) tree: RawObjectId,
    pub(super) parents: RawParents,
    pub(super) time: i64,
    pub(super) offset: i32,
}

#[derive(Clone, Debug, Default)]
pub(super) struct RawParents {
    first: Option<RawObjectId>,
    extra: Vec<RawObjectId>,
}

impl RawParents {
    pub(super) fn push(&mut self, parent: RawObjectId) {
        if self.first.is_none() {
            self.first = Some(parent);
        } else {
            self.extra.push(parent);
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.first.is_none()
    }

    pub(super) fn len(&self) -> usize {
        usize::from(self.first.is_some()) + self.extra.len()
    }

    pub(super) fn first(&self) -> Option<RawObjectId> {
        self.first
    }

    pub(super) fn iter(&self) -> RawParentsIter<'_> {
        RawParentsIter {
            first: self.first,
            extra: self.extra.iter(),
        }
    }
}

pub(super) struct RawParentsIter<'a> {
    first: Option<RawObjectId>,
    extra: std::slice::Iter<'a, RawObjectId>,
}

impl Iterator for RawParentsIter<'_> {
    type Item = RawObjectId;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(first) = self.first.take() {
            return Some(first);
        }
        self.extra.next().copied()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct RawTreeEntry {
    pub(super) mode: u32,
    pub(super) id: RawObjectId,
}

#[derive(Clone, Debug)]
pub(super) struct RawNamedTreeEntry {
    pub(super) name: Vec<u8>,
    pub(super) entry: RawTreeEntry,
}

#[derive(Clone, Debug)]
pub(super) struct PackedRawObject {
    pub(super) type_code: u8,
    pub(super) base: Option<PackedBase>,
    pub(super) data: Vec<u8>,
}

#[derive(Clone, Debug)]
pub(super) enum PackedBase {
    Offset(u64),
    Id(RawObjectId),
}
