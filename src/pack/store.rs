//! Loose/packed object storage, indexes, and bounded caches.

use super::limits::{
    MAX_COMMIT_CACHE_ENTRIES, MAX_GIT_OBJECT_BYTES, MAX_OBJECT_CACHE_BYTES,
    MAX_OBJECT_CACHE_ENTRIES, MAX_PACK_DELTA_DEPTH, MAX_TREE_CACHE_BYTES, MAX_TREE_CACHE_ENTRIES,
};
use super::objects::{
    apply_pack_delta, find_tree_entry, parse_raw_commit, parse_raw_commit_subject,
    parse_tree_entries, raw_kind_from_name, read_be_u32, read_be_u64, read_pack_object_from_bytes,
    validate_git_object_size,
};
use super::types::{
    PackedBase, PackedRawObject, RawCommit, RawGitObject, RawNamedTreeEntry, RawObjectId,
    RawObjectKind, RawTreeEntry, parse_hex_byte,
};
use crate::AnyResult;
use rustc_hash::FxHashMap as HashMap;
use std::cmp::Ordering;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(super) struct RawGitStore {
    git_dir: PathBuf,
    common_dir: PathBuf,
    object_dir: PathBuf,
    indexes: Arc<[PackIndex]>,
    loose_prefixes: [bool; 256],
    object_cache: HashMap<RawObjectId, RawGitObject>,
    object_cache_bytes: usize,
    commit_cache: HashMap<RawObjectId, RawCommit>,
    tree_entries_cache: HashMap<RawObjectId, Arc<[RawNamedTreeEntry]>>,
    tree_entries_cache_bytes: usize,
    offset_cache: HashMap<(usize, u64), RawGitObject>,
    offset_cache_bytes: usize,
    pack_maps: Vec<Option<Arc<memmap2::Mmap>>>,
}

impl RawGitStore {
    pub(super) fn open(repo: &str) -> AnyResult<Self> {
        let repo_root = Path::new(repo);
        let git_dir = resolve_git_dir(repo_root)?;
        let common_dir = resolve_common_dir(&git_dir)?;
        let object_dir = common_dir.join("objects");
        let indexes = load_pack_indexes(&object_dir)?;
        let loose_prefixes = load_loose_prefixes(&object_dir)?;
        Ok(Self {
            git_dir,
            common_dir,
            object_dir,
            pack_maps: std::iter::repeat_with(|| None)
                .take(indexes.len())
                .collect(),
            indexes,
            loose_prefixes,
            object_cache: HashMap::default(),
            object_cache_bytes: 0,
            commit_cache: HashMap::default(),
            tree_entries_cache: HashMap::default(),
            tree_entries_cache_bytes: 0,
            offset_cache: HashMap::default(),
            offset_cache_bytes: 0,
        })
    }

    pub(super) fn fork_empty(&self) -> Self {
        Self {
            git_dir: self.git_dir.clone(),
            common_dir: self.common_dir.clone(),
            object_dir: self.object_dir.clone(),
            pack_maps: self.pack_maps.clone(),
            indexes: Arc::clone(&self.indexes),
            loose_prefixes: self.loose_prefixes,
            object_cache: HashMap::default(),
            object_cache_bytes: 0,
            commit_cache: HashMap::default(),
            tree_entries_cache: HashMap::default(),
            tree_entries_cache_bytes: 0,
            offset_cache: HashMap::default(),
            offset_cache_bytes: 0,
        }
    }

    pub(super) fn head_id(&self) -> AnyResult<RawObjectId> {
        let head = fs::read_to_string(self.git_dir.join("HEAD"))?;
        let head = head.trim();
        if let Some(reference) = head.strip_prefix("ref: ") {
            return self.resolve_ref(reference.trim());
        }
        RawObjectId::from_hex_str(head)
    }

    fn resolve_ref(&self, reference: &str) -> AnyResult<RawObjectId> {
        for base in [&self.git_dir, &self.common_dir] {
            let path = base.join(reference);
            if path.is_file() {
                return RawObjectId::from_hex_str(fs::read_to_string(path)?.trim());
            }
        }
        for base in [&self.git_dir, &self.common_dir] {
            let path = base.join("packed-refs");
            if let Some(id) = packed_ref_id(&path, reference)? {
                return Ok(id);
            }
        }
        Err(format!("could not resolve ref {reference:?}").into())
    }

    pub(super) fn raw_commit(&mut self, id: RawObjectId) -> AnyResult<RawCommit> {
        if let Some(commit) = self.commit_cache.get(&id) {
            return Ok(commit.clone());
        }
        let commit = {
            let object = self.find_object_ref(id)?;
            if object.kind != RawObjectKind::Commit {
                return Err(format!("object {} is not a commit", id.to_hex()).into());
            }
            parse_raw_commit(&object.data)?
        };
        if self.commit_cache.len() >= MAX_COMMIT_CACHE_ENTRIES {
            self.commit_cache.clear();
        }
        self.commit_cache.insert(id, commit.clone());
        Ok(commit)
    }

    pub(super) fn raw_commit_subject(&mut self, id: RawObjectId) -> AnyResult<String> {
        let object = self.find_object_ref(id)?;
        if object.kind != RawObjectKind::Commit {
            return Err(format!("object {} is not a commit", id.to_hex()).into());
        }
        Ok(parse_raw_commit_subject(&object.data))
    }

    pub(super) fn find_tree_child_entry(
        &mut self,
        tree_id: RawObjectId,
        name: &[u8],
    ) -> AnyResult<Option<RawTreeEntry>> {
        let object = self.find_object_ref(tree_id)?;
        if object.kind != RawObjectKind::Tree {
            return Ok(None);
        }
        find_tree_entry(&object.data, name)
    }

    pub(super) fn tree_entries(
        &mut self,
        tree_id: RawObjectId,
    ) -> AnyResult<Arc<[RawNamedTreeEntry]>> {
        if let Some(entries) = self.tree_entries_cache.get(&tree_id) {
            return Ok(Arc::clone(entries));
        }
        let entries = {
            let object = self.find_object_ref(tree_id)?;
            if object.kind != RawObjectKind::Tree {
                return Err(format!("object {} is not a tree", tree_id.to_hex()).into());
            }
            parse_tree_entries(&object.data)?
        };
        let entries: Arc<[RawNamedTreeEntry]> = entries.into();
        let entries_bytes = entries
            .iter()
            .fold(std::mem::size_of_val(entries.as_ref()), |total, entry| {
                total.saturating_add(entry.name.len())
            });
        if cache_needs_reset(
            self.tree_entries_cache_bytes,
            entries_bytes,
            self.tree_entries_cache.len(),
            MAX_TREE_CACHE_ENTRIES,
            MAX_TREE_CACHE_BYTES,
        ) {
            self.tree_entries_cache.clear();
            self.tree_entries_cache_bytes = 0;
        }
        self.tree_entries_cache_bytes = self.tree_entries_cache_bytes.saturating_add(entries_bytes);
        self.tree_entries_cache
            .insert(tree_id, Arc::clone(&entries));
        Ok(entries)
    }

    fn find_object_ref(&mut self, id: RawObjectId) -> AnyResult<&RawGitObject> {
        if !self.object_cache.contains_key(&id) {
            let object = self.load_object_at_delta_depth(id, 0)?;
            self.cache_object(id, object);
        }
        match self.object_cache.get(&id) {
            Some(object) => Ok(object),
            None => Err(format!("object {} not found after load", id.to_hex()).into()),
        }
    }

    fn find_object_at_delta_depth(
        &mut self,
        id: RawObjectId,
        depth: usize,
    ) -> AnyResult<RawGitObject> {
        if let Some(object) = self.object_cache.get(&id) {
            return Ok(object.clone());
        }
        let object = self.load_object_at_delta_depth(id, depth)?;
        self.cache_object(id, object.clone());
        Ok(object)
    }

    fn cache_object(&mut self, id: RawObjectId, object: RawGitObject) {
        if cache_needs_reset(
            self.object_cache_bytes,
            object.data.len(),
            self.object_cache.len(),
            MAX_OBJECT_CACHE_ENTRIES,
            MAX_OBJECT_CACHE_BYTES,
        ) {
            self.object_cache.clear();
            self.object_cache_bytes = 0;
        }
        self.object_cache_bytes = self.object_cache_bytes.saturating_add(object.data.len());
        self.object_cache.insert(id, object);
    }

    fn load_object_at_delta_depth(
        &mut self,
        id: RawObjectId,
        depth: usize,
    ) -> AnyResult<RawGitObject> {
        Ok(if let Some(object) = self.find_loose_object(id)? {
            object
        } else if let Some((pack_index, offset)) = self.find_pack_offset(id) {
            self.find_pack_object_at_depth(pack_index, offset, depth)?
        } else {
            return Err(format!("object {} not found", id.to_hex()).into());
        })
    }

    fn find_pack_offset(&self, id: RawObjectId) -> Option<(usize, u64)> {
        self.indexes
            .iter()
            .enumerate()
            .find_map(|(pack_index, index)| {
                index.lookup_offset(id).map(|offset| (pack_index, offset))
            })
    }

    fn find_loose_object(&self, id: RawObjectId) -> AnyResult<Option<RawGitObject>> {
        if !self.loose_prefixes[id.0[0] as usize] {
            return Ok(None);
        }
        let hex = id.to_hex();
        let path = self.object_dir.join(&hex[..2]).join(&hex[2..]);
        if !path.is_file() {
            return Ok(None);
        }
        let file = File::open(path)?;
        let decoder = flate2::read::ZlibDecoder::new(file);
        let mut inflated = Vec::new();
        decoder
            .take(MAX_GIT_OBJECT_BYTES.saturating_add(1024).saturating_add(1))
            .read_to_end(&mut inflated)?;
        if inflated.len() as u64 > MAX_GIT_OBJECT_BYTES.saturating_add(1024) {
            return Err(format!(
                "loose object {} exceeds the supported size limit of {MAX_GIT_OBJECT_BYTES} bytes",
                id.to_hex()
            )
            .into());
        }
        let Some(header_end) = inflated.iter().position(|byte| *byte == 0) else {
            return Err("loose object missing header delimiter".into());
        };
        let header = std::str::from_utf8(&inflated[..header_end])?;
        let (kind, declared_size) = header
            .split_once(' ')
            .ok_or("loose object header must contain kind and size")?;
        let kind = raw_kind_from_name(kind)?;
        let declared_size: u64 = declared_size.parse()?;
        validate_git_object_size(declared_size, "loose object")?;
        let data_start = header_end + 1;
        let data_len = inflated.len() - data_start;
        if data_len as u64 != declared_size {
            return Err(format!(
                "loose object size mismatch: expected {declared_size}, got {}",
                data_len
            )
            .into());
        }
        inflated.drain(..data_start);
        Ok(Some(RawGitObject {
            kind,
            data: inflated.into(),
        }))
    }

    fn find_pack_object_at_depth(
        &mut self,
        pack_index: usize,
        offset: u64,
        depth: usize,
    ) -> AnyResult<RawGitObject> {
        let cache_key = (pack_index, offset);
        if let Some(object) = self.offset_cache.get(&cache_key) {
            return Ok(object.clone());
        }
        let raw = self.read_pack_object(pack_index, offset)?;
        let object = match raw.type_code {
            1 => RawGitObject {
                kind: RawObjectKind::Commit,
                data: raw.data.into(),
            },
            2 => RawGitObject {
                kind: RawObjectKind::Tree,
                data: raw.data.into(),
            },
            3 => RawGitObject {
                kind: RawObjectKind::Blob,
                data: raw.data.into(),
            },
            4 => RawGitObject {
                kind: RawObjectKind::Tag,
                data: raw.data.into(),
            },
            6 | 7 => {
                let Some(base) = raw.base else {
                    return Err("delta object missing base".into());
                };
                let base_depth = next_pack_delta_depth(depth)?;
                let base = match base {
                    PackedBase::Offset(base_offset) => {
                        self.find_pack_object_at_depth(pack_index, base_offset, base_depth)?
                    }
                    PackedBase::Id(base_id) => {
                        self.find_object_at_delta_depth(base_id, base_depth)?
                    }
                };
                RawGitObject {
                    kind: base.kind,
                    data: apply_pack_delta(&base.data, &raw.data)?.into(),
                }
            }
            other => return Err(format!("unsupported pack object type {other}").into()),
        };
        if cache_needs_reset(
            self.offset_cache_bytes,
            object.data.len(),
            self.offset_cache.len(),
            MAX_OBJECT_CACHE_ENTRIES,
            MAX_OBJECT_CACHE_BYTES,
        ) {
            self.offset_cache.clear();
            self.offset_cache_bytes = 0;
        }
        self.offset_cache_bytes = self.offset_cache_bytes.saturating_add(object.data.len());
        self.offset_cache.insert(cache_key, object.clone());
        Ok(object)
    }

    fn read_pack_object(&mut self, pack_index: usize, offset: u64) -> AnyResult<PackedRawObject> {
        if pack_index >= self.indexes.len() {
            return Err(format!("pack index {pack_index} out of range").into());
        }
        if self.pack_maps[pack_index].is_none() {
            let pack_path = &self.indexes[pack_index].pack_path;
            let file = File::open(pack_path)?;
            let mmap = unsafe { memmap2::Mmap::map(&file)? };
            self.pack_maps[pack_index] = Some(Arc::new(mmap));
        }
        let mmap = self
            .pack_maps
            .get(pack_index)
            .and_then(Option::as_ref)
            .ok_or("failed to open cached pack map")?;
        read_pack_object_from_bytes(mmap, offset)
    }
}

pub(super) fn cache_needs_reset(
    current_bytes: usize,
    incoming_bytes: usize,
    entries: usize,
    max_entries: usize,
    max_bytes: usize,
) -> bool {
    entries >= max_entries || current_bytes.saturating_add(incoming_bytes) > max_bytes
}

pub(super) fn next_pack_delta_depth(depth: usize) -> AnyResult<usize> {
    let next = depth.checked_add(1).ok_or("pack delta depth overflow")?;
    if next > MAX_PACK_DELTA_DEPTH {
        return Err(format!(
            "pack delta chain exceeds the supported depth of {MAX_PACK_DELTA_DEPTH}"
        )
        .into());
    }
    Ok(next)
}

pub(super) struct PackIndex {
    pack_path: PathBuf,
    data: Vec<u8>,
    fanout: [u32; 256],
    count: usize,
    names_start: usize,
    offsets_start: usize,
    large_offsets_start: usize,
}

impl PackIndex {
    pub(super) fn open(idx_path: PathBuf) -> AnyResult<Self> {
        let data = fs::read(&idx_path)?;
        Self::from_data(idx_path, data)
    }

    pub(super) fn from_data(idx_path: PathBuf, data: Vec<u8>) -> AnyResult<Self> {
        if data.len() < 8 + 256 * 4 {
            return Err(format!("idx file too small: {}", idx_path.display()).into());
        }
        if data.get(0..4) != Some(&[0xff, b't', b'O', b'c']) {
            return Err(format!("unsupported idx magic: {}", idx_path.display()).into());
        }
        if read_be_u32(&data, 4)? != 2 {
            return Err(format!("unsupported idx version: {}", idx_path.display()).into());
        }
        let mut fanout = [0u32; 256];
        for (idx, slot) in fanout.iter_mut().enumerate() {
            *slot = read_be_u32(&data, 8 + idx * 4)?;
        }
        if fanout.windows(2).any(|pair| pair[0] > pair[1]) {
            return Err(format!("non-monotonic idx fanout: {}", idx_path.display()).into());
        }
        let count = fanout[255] as usize;
        let names_start: usize = 8 + 256 * 4;
        let names_bytes = count
            .checked_mul(20)
            .ok_or("idx name table size overflow")?;
        let crc_bytes = count.checked_mul(4).ok_or("idx CRC table size overflow")?;
        let offsets_bytes = count
            .checked_mul(4)
            .ok_or("idx offset table size overflow")?;
        let crc_start = names_start
            .checked_add(names_bytes)
            .ok_or("idx name table offset overflow")?;
        let offsets_start = crc_start
            .checked_add(crc_bytes)
            .ok_or("idx CRC table offset overflow")?;
        let large_offsets_start = offsets_start
            .checked_add(offsets_bytes)
            .ok_or("idx offset table offset overflow")?;
        let minimum_size = large_offsets_start
            .checked_add(40)
            .ok_or("idx trailer offset overflow")?;
        if data.len() < minimum_size {
            return Err(format!("truncated idx file: {}", idx_path.display()).into());
        }
        let pack_path = idx_path.with_extension("pack");
        Ok(Self {
            pack_path,
            data,
            fanout,
            count,
            names_start,
            offsets_start,
            large_offsets_start,
        })
    }

    fn lookup_offset(&self, id: RawObjectId) -> Option<u64> {
        let bucket = id.0[0] as usize;
        let start = if bucket == 0 {
            0
        } else {
            self.fanout[bucket - 1] as usize
        };
        let end = self.fanout[bucket] as usize;
        let mut left = start;
        let mut right = end;
        while left < right {
            let mid = (left + right) / 2;
            let raw = self.object_id_slice(mid)?;
            match raw.cmp(id.0.as_slice()) {
                Ordering::Less => left = mid + 1,
                Ordering::Greater => right = mid,
                Ordering::Equal => return self.offset_at(mid).ok(),
            }
        }
        None
    }

    fn object_id_slice(&self, index: usize) -> Option<&[u8]> {
        if index >= self.count {
            return None;
        }
        let start = self.names_start + index * 20;
        self.data.get(start..start + 20)
    }

    fn offset_at(&self, index: usize) -> AnyResult<u64> {
        if index >= self.count {
            return Err("pack index offset out of range".into());
        }
        let raw = read_be_u32(&self.data, self.offsets_start + index * 4)?;
        if raw & 0x8000_0000 == 0 {
            return Ok(raw as u64);
        }
        let large_index = (raw & 0x7fff_ffff) as usize;
        read_be_u64(&self.data, self.large_offsets_start + large_index * 8)
    }
}

fn resolve_git_dir(repo_root: &Path) -> AnyResult<PathBuf> {
    let dot_git = repo_root.join(".git");
    if dot_git.is_dir() {
        return Ok(dot_git);
    }
    let text = fs::read_to_string(&dot_git)?;
    let Some(path) = text.trim().strip_prefix("gitdir:") else {
        return Err(format!("unsupported .git file: {}", dot_git.display()).into());
    };
    Ok(resolve_relative_path(repo_root, path.trim()))
}

fn resolve_common_dir(git_dir: &Path) -> AnyResult<PathBuf> {
    let common_dir = git_dir.join("commondir");
    if !common_dir.is_file() {
        return Ok(git_dir.to_path_buf());
    }
    Ok(resolve_relative_path(
        git_dir,
        fs::read_to_string(common_dir)?.trim(),
    ))
}

fn resolve_relative_path(base: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn load_pack_indexes(object_dir: &Path) -> AnyResult<Arc<[PackIndex]>> {
    let pack_dir = object_dir.join("pack");
    if !pack_dir.is_dir() {
        return Ok(Vec::new().into());
    }
    let mut paths = fs::read_dir(pack_dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "idx"))
        .collect::<Vec<_>>();
    paths.sort();
    let mut indexes = paths
        .into_iter()
        .map(PackIndex::open)
        .collect::<AnyResult<Vec<_>>>()?;
    indexes.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then(left.pack_path.cmp(&right.pack_path))
    });
    Ok(indexes.into())
}

fn load_loose_prefixes(object_dir: &Path) -> AnyResult<[bool; 256]> {
    let mut prefixes = [false; 256];
    if !object_dir.is_dir() {
        return Ok(prefixes);
    }
    for entry in fs::read_dir(object_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.len() != 2 {
            continue;
        }
        if let Ok(prefix) = parse_hex_byte(name.as_bytes()) {
            prefixes[prefix as usize] = true;
        }
    }
    Ok(prefixes)
}

fn packed_ref_id(path: &Path, reference: &str) -> AnyResult<Option<RawObjectId>> {
    if !path.is_file() {
        return Ok(None);
    }
    for line in fs::read_to_string(path)?.lines() {
        if line.is_empty() || line.starts_with('#') || line.starts_with('^') {
            continue;
        }
        let Some((id, name)) = line.split_once(' ') else {
            continue;
        };
        if name == reference {
            return Ok(Some(RawObjectId::from_hex_str(id)?));
        }
    }
    Ok(None)
}
