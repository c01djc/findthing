use std::collections::{HashMap, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::ntfs::{self, Delta, FileRec};

const FLAG_DIR: u32 = 1;
const FLAG_DELETED: u32 = 2;
const CAP: usize = 2_000_000;
const UNKNOWN: u8 = 0;
const INCLUDE: u8 = 1;
const PRUNE: u8 = 2;
const SKIP: u8 = 3;
const UNSET: u32 = u32::MAX;
const DROPPED: u32 = u32::MAX - 1;

#[derive(Clone)]
struct BitSet {
    words: Vec<u64>,
}

impl BitSet {
    fn set(&mut self, id: u32) {
        let index = (id as usize) >> 6;
        if index >= self.words.len() {
            self.words.resize(index + 1, 0);
        }
        self.words[index] |= 1u64 << (id & 63);
    }

    fn unset(&mut self, id: u32) {
        let index = (id as usize) >> 6;
        if let Some(word) = self.words.get_mut(index) {
            *word &= !(1u64 << (id & 63));
        }
    }

    fn test(&self, id: u32) -> bool {
        let index = (id as usize) >> 6;
        self.words.get(index).is_some_and(|word| word & (1u64 << (id & 63)) != 0)
    }
}

#[derive(Clone)]
struct Node {
    parent: u32,
    start: u32,
    len: u32,
    flags: u32,
    frn: u64,
}

#[derive(Clone)]
struct VolumeCursor {
    letter: u8,
    journal_id: u64,
    next_usn: i64,
}

#[derive(Clone)]
pub struct Index {
    names: String,
    nodes: Vec<Node>,
    ascii: [BitSet; 36],
    other: HashMap<char, Vec<u32>>,
    prefix: [Vec<u32>; 36],
    prefix_other: HashMap<char, Vec<u32>>,
    volumes: Vec<VolumeCursor>,
    deleted: u32,
    wasted: u64,
}

impl Index {
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn live(&self) -> u64 {
        self.nodes.iter().filter(|node| node.flags & FLAG_DELETED == 0).count() as u64
    }

    pub fn push(&mut self, parent: u32, name: &str, dir: bool) -> u32 {
        let flags = (if dir { FLAG_DIR } else { 0 }) | (0xFEu32 << 16);
        self.add_node(parent, name, flags, 0)
    }

    fn add_node(&mut self, parent: u32, name: &str, flags: u32, frn: u64) -> u32 {
        let start = self.names.len() as u32;
        self.names.push_str(name);
        let id = self.nodes.len() as u32;
        self.nodes.push(Node {
            parent,
            start,
            len: name.len() as u32,
            flags,
            frn,
        });
        if flags & FLAG_DELETED == 0 {
            self.index_name(id, name);
        }
        id
    }

    fn index_name(&mut self, id: u32, name: &str) {
        let mut seen = [false; 36];
        for ch in name.chars() {
            if let Some(slot) = ascii_slot(ch) {
                let slot = slot as usize;
                if !seen[slot] {
                    seen[slot] = true;
                    self.ascii[slot].set(id);
                }
            } else if !ch.is_ascii() {
                let list = self.other.entry(ch).or_default();
                if let Err(pos) = list.binary_search(&id) {
                    list.insert(pos, id);
                }
            }
        }
        if let Some(first) = name.chars().next() {
            if let Some(slot) = ascii_slot(first) {
                self.prefix[slot as usize].push(id);
            } else if !first.is_ascii() {
                self.prefix_other.entry(first).or_default().push(id);
            }
        }
    }

    fn unindex_chars(&mut self, id: u32, name: &str) {
        let mut seen = [false; 36];
        for ch in name.chars() {
            if let Some(slot) = ascii_slot(ch) {
                let slot = slot as usize;
                if !seen[slot] {
                    seen[slot] = true;
                    self.ascii[slot].unset(id);
                }
            } else if !ch.is_ascii() {
                if let Some(list) = self.other.get_mut(&ch) {
                    if let Ok(pos) = list.binary_search(&id) {
                        list.remove(pos);
                    }
                }
            }
        }
    }

    pub fn alive(&self, id: u32) -> bool {
        self.nodes.get(id as usize).is_some_and(|node| node.flags & FLAG_DELETED == 0)
    }

    pub fn prefix_ids(&self, needle: &str) -> Option<&[u32]> {
        let ch = needle.chars().next()?;
        if let Some(slot) = ascii_slot(ch) {
            Some(&self.prefix[slot as usize])
        } else if !ch.is_ascii() {
            self.prefix_other.get(&ch).map(Vec::as_slice)
        } else {
            None
        }
    }

    pub fn has_query_chars(&self, id: u32, needle: &str) -> bool {
        for ch in needle.chars() {
            if let Some(slot) = ascii_slot(ch) {
                if !self.ascii[slot as usize].test(id) {
                    return false;
                }
            } else if !ch.is_ascii() {
                let Some(list) = self.other.get(&ch) else {
                    return false;
                };
                if list.binary_search(&id).is_err() {
                    return false;
                }
            }
        }
        true
    }

    pub fn visit_matches(&self, needle: &str, mut visit: impl FnMut(u32)) {
        let keys = match_keys(needle);
        if keys.is_empty() {
            for id in 0..self.nodes.len() as u32 {
                if self.alive(id) {
                    visit(id);
                }
            }
            return;
        }
        let mut ascii = Vec::new();
        let mut others: Vec<&[u32]> = Vec::new();
        for key in keys {
            match key {
                Key::Ascii(slot) => ascii.push(slot),
                Key::Other(ch) => {
                    let Some(list) = self.other.get(&ch) else {
                        return;
                    };
                    if list.is_empty() {
                        return;
                    }
                    others.push(list);
                }
            }
        }
        if !others.is_empty() {
            others.sort_by_key(|list| list.len());
            for &id in others[0] {
                if !self.alive(id) {
                    continue;
                }
                if ascii.iter().any(|slot| !self.ascii[*slot as usize].test(id)) {
                    continue;
                }
                if others.iter().skip(1).any(|list| list.binary_search(&id).is_err()) {
                    continue;
                }
                visit(id);
            }
            return;
        }
        self.visit_ascii(&ascii, &mut visit);
    }

    fn visit_ascii(&self, slots: &[u8], visit: &mut impl FnMut(u32)) {
        if slots.is_empty() {
            return;
        }
        let sets: Vec<&BitSet> = slots.iter().map(|slot| &self.ascii[*slot as usize]).collect();
        let words = sets.iter().map(|set| set.words.len()).min().unwrap_or(0);
        let limit = self.nodes.len() as u32;
        for word in 0..words {
            let mut bits = u64::MAX;
            for set in &sets {
                bits &= set.words[word];
            }
            while bits != 0 {
                let bit = bits.trailing_zeros();
                bits &= bits - 1;
                let id = word as u32 * 64 + bit;
                if id < limit && self.alive(id) {
                    visit(id);
                }
            }
        }
    }

    pub fn name(&self, id: u32) -> &str {
        let node = &self.nodes[id as usize];
        let start = node.start as usize;
        &self.names[start..start + node.len as usize]
    }

    pub fn is_dir(&self, id: u32) -> bool {
        self.nodes[id as usize].flags & FLAG_DIR != 0
    }

    pub fn parent(&self, id: u32) -> u32 {
        self.nodes[id as usize].parent
    }

    pub fn brief(&self, id: u32) -> Option<(u32, bool)> {
        let node = self.nodes.get(id as usize)?;
        if node.parent == u32::MAX || node.flags & FLAG_DELETED != 0 {
            return None;
        }
        Some((node.len, node.flags & FLAG_DIR != 0))
    }

    pub fn path(&self, mut id: u32) -> String {
        let mut parts = Vec::new();
        let mut guard = 0;
        while id != u32::MAX {
            parts.push(self.name(id));
            id = self.nodes[id as usize].parent;
            guard += 1;
            if guard > 80 {
                break;
            }
        }
        parts.reverse();
        parts.join("\\")
    }

    fn mount(&mut self, letter: u8, snapshot: ntfs::Snapshot, profile: Option<&Path>) {
        if self.volumes.len() >= 250 || self.nodes.len() >= CAP {
            return;
        }
        let vol = self.volumes.len() as u8;
        self.volumes.push(VolumeCursor {
            letter,
            journal_id: snapshot.journal.id,
            next_usn: snapshot.journal.next_usn,
        });
        let drive = self.add_node(u32::MAX, &format!("{}:", letter as char), FLAG_DIR | ((vol as u32) << 16), 0);
        let records = snapshot.records;
        if records.is_empty() || self.nodes.len() >= CAP {
            return;
        }
        let mut by_frn: HashMap<u64, u32> = HashMap::with_capacity(records.len());
        for (index, rec) in records.iter().enumerate() {
            by_frn.insert(rec.frn, index as u32);
        }
        let mut state = vec![UNKNOWN; records.len()];
        for index in 0..records.len() {
            resolve_state(index, &records, &by_frn, &mut state);
        }
        let mut ids = vec![UNSET; records.len()];
        let room = CAP.saturating_sub(self.nodes.len());
        let needed = state.iter().filter(|kind| **kind == INCLUDE || **kind == PRUNE).count();
        if needed > room {
            if let Some(profile_at) = profile_record(letter, profile, &records, &by_frn) {
                let (head, next) = child_links(&records, &by_frn);
                let mut seen = vec![false; records.len()];
                self.ensure_chain(profile_at, drive, vol, &records, &by_frn, &state, &mut ids);
                let mut queue = VecDeque::new();
                enqueue_children(profile_at, &head, &next, &mut queue);
                while let Some(child) = queue.pop_front() {
                    let child = child as usize;
                    if seen[child] {
                        continue;
                    }
                    seen[child] = true;
                    if self.nodes.len() >= CAP {
                        break;
                    }
                    self.ensure_chain(child as u32, drive, vol, &records, &by_frn, &state, &mut ids);
                    enqueue_children(child as u32, &head, &next, &mut queue);
                }
            }
        }
        for index in 0..records.len() {
            if self.nodes.len() >= CAP {
                break;
            }
            self.ensure_chain(index as u32, drive, vol, &records, &by_frn, &state, &mut ids);
        }
    }

    fn ensure_chain(
        &mut self,
        start: u32,
        drive: u32,
        vol: u8,
        records: &[FileRec],
        by_frn: &HashMap<u64, u32>,
        state: &[u8],
        ids: &mut [u32],
    ) {
        let start = start as usize;
        if start >= ids.len() || ids[start] != UNSET {
            return;
        }
        let mut chain = Vec::new();
        let mut cur = start;
        loop {
            if ids[cur] != UNSET || state[cur] == SKIP {
                break;
            }
            chain.push(cur);
            if records[cur].parent == ntfs::ROOT_FRN {
                break;
            }
            let Some(&parent) = by_frn.get(&records[cur].parent) else {
                for node in &chain {
                    ids[*node] = DROPPED;
                }
                return;
            };
            let parent = parent as usize;
            if chain.len() > 200 || chain.contains(&parent) {
                for node in &chain {
                    ids[*node] = DROPPED;
                }
                return;
            }
            cur = parent;
        }
        while let Some(node) = chain.pop() {
            if ids[node] != UNSET {
                continue;
            }
            if state[node] != INCLUDE && state[node] != PRUNE || self.nodes.len() >= CAP {
                ids[node] = DROPPED;
                continue;
            }
            let parent_id = if records[node].parent == ntfs::ROOT_FRN {
                drive
            } else {
                let Some(&parent) = by_frn.get(&records[node].parent) else {
                    ids[node] = DROPPED;
                    continue;
                };
                let parent_id = ids[parent as usize];
                if parent_id >= DROPPED {
                    ids[node] = DROPPED;
                    continue;
                }
                parent_id
            };
            let dir = records[node].attrs & 0x10 != 0;
            let flags = (if dir { FLAG_DIR } else { 0 }) | ((vol as u32) << 16);
            ids[node] = self.add_node(parent_id, &records[node].name, flags, records[node].frn);
        }
    }

    fn apply_volume(&mut self, vol: u8, changes: &[ntfs::Change]) {
        let mut by_frn: HashMap<u64, u32> = HashMap::new();
        for (id, node) in self.nodes.iter().enumerate() {
            if vol_of(node.flags) == vol && node.frn != 0 {
                by_frn.insert(node.frn, id as u32);
            }
        }
        let Some(drive) = self
            .nodes
            .iter()
            .position(|node| node.parent == u32::MAX && vol_of(node.flags) == vol)
            .map(|id| id as u32)
        else {
            return;
        };
        let mut deferred = Vec::new();
        for change in changes {
            if !self.apply_one(vol, drive, change, &mut by_frn) {
                deferred.push(change);
            }
        }
        for _ in 0..8 {
            if deferred.is_empty() {
                break;
            }
            let pending = std::mem::take(&mut deferred);
            for change in pending {
                if !self.apply_one(vol, drive, change, &mut by_frn) {
                    deferred.push(change);
                }
            }
        }
    }

    fn apply_one(&mut self, vol: u8, drive: u32, change: &ntfs::Change, by_frn: &mut HashMap<u64, u32>) -> bool {
        let make = change.reason & (ntfs::REASON_CREATE | ntfs::REASON_RENAME_NEW) != 0;
        let drop = change.reason & (ntfs::REASON_DELETE | ntfs::REASON_RENAME_OLD) != 0;
        if make {
            self.upsert(vol, drive, change, by_frn)
        } else if drop {
            if let Some(&id) = by_frn.get(&change.frn) {
                self.remove_tree(id);
            }
            true
        } else {
            true
        }
    }

    fn upsert(&mut self, vol: u8, drive: u32, change: &ntfs::Change, by_frn: &mut HashMap<u64, u32>) -> bool {
        if change.name.starts_with('$') {
            return true;
        }
        let parent_known = if change.parent == ntfs::ROOT_FRN {
            Some(drive)
        } else {
            by_frn.get(&change.parent).copied()
        };
        let Some(parent) = parent_known else {
            return false;
        };
        if !self.alive(parent) || self.blocked(parent) {
            return true;
        }
        let dir = change.attrs & 0x10 != 0;
        if let Some(&id) = by_frn.get(&change.frn) {
            self.retarget(id, parent, &change.name, dir);
            true
        } else if self.nodes.len() >= CAP {
            true
        } else {
            let flags = (if dir { FLAG_DIR } else { 0 }) | ((vol as u32) << 16);
            let id = self.add_node(parent, &change.name, flags, change.frn);
            by_frn.insert(change.frn, id);
            true
        }
    }

    fn retarget(&mut self, id: u32, parent: u32, name: &str, dir: bool) {
        let old = self.name(id).to_string();
        let was_deleted = self.nodes[id as usize].flags & FLAG_DELETED != 0;
        if was_deleted {
            self.deleted = self.deleted.saturating_sub(1);
        }
        if was_deleted || old != name {
            if !was_deleted {
                self.unindex_chars(id, &old);
                self.wasted += old.len() as u64;
            }
            let start = self.names.len() as u32;
            self.names.push_str(name);
            let node = &mut self.nodes[id as usize];
            node.start = start;
            node.len = name.len() as u32;
            node.parent = parent;
            node.flags &= !FLAG_DELETED;
            if dir {
                node.flags |= FLAG_DIR;
            } else {
                node.flags &= !FLAG_DIR;
            }
            self.index_name(id, name);
        } else {
            let node = &mut self.nodes[id as usize];
            node.parent = parent;
            node.flags &= !FLAG_DELETED;
            if dir {
                node.flags |= FLAG_DIR;
            } else {
                node.flags &= !FLAG_DIR;
            }
        }
    }

    fn remove_tree(&mut self, id: u32) {
        if self.nodes[id as usize].flags & FLAG_DELETED != 0 {
            return;
        }
        let name = self.name(id).to_string();
        self.unindex_chars(id, &name);
        self.wasted += self.nodes[id as usize].len as u64;
        self.deleted += 1;
        self.nodes[id as usize].flags |= FLAG_DELETED;
        let children: Vec<u32> = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node.parent == id && node.flags & FLAG_DELETED == 0)
            .map(|(id, _)| id as u32)
            .collect();
        for child in children {
            self.remove_tree(child);
        }
    }

    fn blocked(&self, mut id: u32) -> bool {
        let mut guard = 0;
        while id != u32::MAX && guard < 80 {
            if self.is_prune(id) {
                return true;
            }
            id = self.nodes[id as usize].parent;
            guard += 1;
        }
        false
    }

    fn is_prune(&self, id: u32) -> bool {
        let parent = self.nodes[id as usize].parent;
        let parent_is_root = parent != u32::MAX && self.nodes[parent as usize].parent == u32::MAX;
        prunes(self.name(id), parent_is_root)
    }

    fn worth_compacting(&self) -> bool {
        self.deleted >= 8_192 || self.wasted >= 2 * 1024 * 1024
    }

    fn compact(&self) -> Index {
        let count = self.nodes.len();
        let mut head = vec![UNSET; count];
        let mut next = vec![UNSET; count];
        let mut roots = Vec::new();
        for (id, node) in self.nodes.iter().enumerate() {
            if node.flags & FLAG_DELETED != 0 {
                continue;
            }
            if node.parent == u32::MAX {
                roots.push(id as u32);
            } else if (node.parent as usize) < count && self.nodes[node.parent as usize].flags & FLAG_DELETED == 0 {
                next[id] = head[node.parent as usize];
                head[node.parent as usize] = id as u32;
            }
        }
        let mut fresh = Index::default();
        fresh.volumes = self.volumes.clone();
        let mut map = vec![UNSET; count];
        let mut queue = VecDeque::new();
        for id in roots {
            self.shift(&mut fresh, &mut map, id, u32::MAX);
            queue.push_back(id);
        }
        while let Some(id) = queue.pop_front() {
            let parent = map[id as usize];
            let mut child = head[id as usize];
            while child != UNSET {
                let follow = next[child as usize];
                self.shift(&mut fresh, &mut map, child, parent);
                queue.push_back(child);
                child = follow;
            }
        }
        fresh
    }

    fn shift(&self, fresh: &mut Index, map: &mut [u32], id: u32, parent: u32) {
        let node = &self.nodes[id as usize];
        let name = self.name_of(node).to_string();
        map[id as usize] = fresh.add_node(parent, &name, node.flags, node.frn);
    }

    fn recount(&mut self) {
        let mut deleted = 0u32;
        let mut used = 0u64;
        for node in &self.nodes {
            if node.flags & FLAG_DELETED != 0 {
                deleted += 1;
            } else {
                used += node.len as u64;
            }
        }
        self.deleted = deleted;
        self.wasted = (self.names.len() as u64).saturating_sub(used);
    }

    pub fn store(&self, path: &Path) -> bool {
        let tmp = path.with_extension("bin.tmp");
        let Ok(file) = std::fs::File::create(&tmp) else {
            return false;
        };
        let mut out = std::io::BufWriter::new(file);
        let saved = (|| {
            use std::io::Write;
            out.write_all(b"FTIX")?;
            out.write_all(&2u32.to_le_bytes())?;
            out.write_all(&(self.volumes.len() as u32).to_le_bytes())?;
            for volume in &self.volumes {
                let mut letter = [0u8; 8];
                letter[0] = volume.letter;
                out.write_all(&letter)?;
                out.write_all(&volume.journal_id.to_le_bytes())?;
                out.write_all(&volume.next_usn.to_le_bytes())?;
            }
            out.write_all(&(self.nodes.len() as u32).to_le_bytes())?;
            for node in &self.nodes {
                out.write_all(&node.parent.to_le_bytes())?;
                out.write_all(&node.flags.to_le_bytes())?;
                out.write_all(&node.frn.to_le_bytes())?;
                let name = self.name_of(node);
                out.write_all(&(name.len() as u32).to_le_bytes())?;
                out.write_all(name.as_bytes())?;
            }
            out.flush()
        })();
        if saved.is_err() {
            let _ = std::fs::remove_file(&tmp);
            return false;
        }
        std::fs::rename(&tmp, path).is_ok()
    }

    pub fn load(path: &Path) -> Option<Self> {
        let file = std::fs::File::open(path).ok()?;
        let mut input = std::io::BufReader::new(file);
        let mut magic = [0u8; 4];
        use std::io::Read;
        input.read_exact(&mut magic).ok()?;
        if &magic != b"FTIX" {
            return None;
        }
        match read_u32(&mut input)? {
            1 => load_v1(&mut input),
            2 => load_v2(&mut input),
            _ => None,
        }
    }

    fn name_of(&self, node: &Node) -> &str {
        let start = node.start as usize;
        &self.names[start..start + node.len as usize]
    }
}

impl Default for Index {
    fn default() -> Self {
        Self {
            names: String::new(),
            nodes: Vec::new(),
            ascii: std::array::from_fn(|_| BitSet { words: Vec::new() }),
            other: HashMap::new(),
            prefix: std::array::from_fn(|_| Vec::new()),
            prefix_other: HashMap::new(),
            volumes: Vec::new(),
            deleted: 0,
            wasted: 0,
        }
    }
}

fn load_v1(input: &mut impl std::io::Read) -> Option<Index> {
    let count = read_u32(input)? as usize;
    if count > CAP {
        return None;
    }
    let mut index = Index::default();
    index.nodes.reserve(count);
    for _ in 0..count {
        let parent = read_u32(input)?;
        let flags = read_u32(input)?;
        let len = read_u32(input)? as usize;
        if len > 32_768 || (parent != u32::MAX && parent as usize >= index.len()) {
            return None;
        }
        let mut bytes = vec![0u8; len];
        input.read_exact(&mut bytes).ok()?;
        let name = std::str::from_utf8(&bytes).ok()?;
        index.push(parent, name, flags & FLAG_DIR != 0);
    }
    index.recount();
    Some(index)
}

fn load_v2(input: &mut impl std::io::Read) -> Option<Index> {
    let volumes = read_u32(input)? as usize;
    if volumes > 64 {
        return None;
    }
    let mut index = Index::default();
    for _ in 0..volumes {
        let mut letter = [0u8; 8];
        input.read_exact(&mut letter).ok()?;
        let mut id = [0u8; 8];
        let mut next = [0u8; 8];
        input.read_exact(&mut id).ok()?;
        input.read_exact(&mut next).ok()?;
        index.volumes.push(VolumeCursor {
            letter: letter[0],
            journal_id: u64::from_le_bytes(id),
            next_usn: i64::from_le_bytes(next),
        });
    }
    let count = read_u32(input)? as usize;
    if count > CAP {
        return None;
    }
    index.nodes.reserve(count);
    for _ in 0..count {
        let parent = read_u32(input)?;
        let flags = read_u32(input)?;
        let mut frn = [0u8; 8];
        input.read_exact(&mut frn).ok()?;
        let len = read_u32(input)? as usize;
        if len > 32_768 || (parent != u32::MAX && parent as usize >= index.len()) {
            return None;
        }
        let mut bytes = vec![0u8; len];
        input.read_exact(&mut bytes).ok()?;
        let name = std::str::from_utf8(&bytes).ok()?;
        index.add_node(parent, name, flags, u64::from_le_bytes(frn));
    }
    index.recount();
    Some(index)
}

fn resolve_state(start: usize, records: &[FileRec], by_frn: &HashMap<u64, u32>, state: &mut [u8]) {
    let mut chain = Vec::new();
    let mut cur = start;
    loop {
        if state[cur] != UNKNOWN {
            break;
        }
        chain.push(cur);
        let rec = &records[cur];
        if rec.name.starts_with('$') || rec.frn == ntfs::ROOT_FRN {
            state[cur] = SKIP;
            break;
        }
        if rec.parent == ntfs::ROOT_FRN {
            state[cur] = if prunes(&rec.name, true) { PRUNE } else { INCLUDE };
            break;
        }
        let Some(&parent) = by_frn.get(&rec.parent) else {
            state[cur] = SKIP;
            break;
        };
        let parent = parent as usize;
        if chain.len() > 200 || chain.contains(&parent) {
            state[cur] = SKIP;
            break;
        }
        cur = parent;
    }
    while let Some(node) = chain.pop() {
        if state[node] != UNKNOWN {
            continue;
        }
        let rec = &records[node];
        let parent_state = if rec.parent == ntfs::ROOT_FRN {
            INCLUDE
        } else if let Some(&parent) = by_frn.get(&rec.parent) {
            state[parent as usize]
        } else {
            SKIP
        };
        if parent_state == SKIP || parent_state == PRUNE || parent_state == UNKNOWN {
            state[node] = SKIP;
        } else if prunes(&rec.name, rec.parent == ntfs::ROOT_FRN) {
            state[node] = PRUNE;
        } else {
            state[node] = INCLUDE;
        }
    }
}

fn profile_record(letter: u8, profile: Option<&Path>, records: &[FileRec], by_frn: &HashMap<u64, u32>) -> Option<u32> {
    let profile = profile?;
    let text = profile.to_string_lossy();
    let mut parts = text.split(['\\', '/']).filter(|part| !part.is_empty());
    let drive = parts.next()?;
    if drive.len() < 2 || drive.as_bytes()[0].to_ascii_uppercase() != letter || drive.as_bytes()[1] != b':' {
        return None;
    }
    let mut frn = ntfs::ROOT_FRN;
    for part in parts {
        let found = records.iter().find(|rec| rec.parent == frn && rec.name.eq_ignore_ascii_case(part))?;
        frn = found.frn;
    }
    by_frn.get(&frn).copied()
}

fn child_links(records: &[FileRec], by_frn: &HashMap<u64, u32>) -> (Vec<u32>, Vec<u32>) {
    let mut head = vec![UNSET; records.len()];
    let mut next = vec![UNSET; records.len()];
    for (index, rec) in records.iter().enumerate() {
        let Some(&parent) = by_frn.get(&rec.parent) else {
            continue;
        };
        let parent = parent as usize;
        if parent == index || parent >= records.len() {
            continue;
        }
        next[index] = head[parent];
        head[parent] = index as u32;
    }
    (head, next)
}

fn enqueue_children(parent: u32, head: &[u32], next: &[u32], queue: &mut VecDeque<u32>) {
    let Some(mut child) = head.get(parent as usize).copied() else {
        return;
    };
    let mut guard = 0;
    while child != UNSET && guard < head.len() {
        queue.push_back(child);
        child = next.get(child as usize).copied().unwrap_or(UNSET);
        guard += 1;
    }
}

fn prunes(name: &str, parent_is_root: bool) -> bool {
    if name.eq_ignore_ascii_case("node_modules")
        || name.eq_ignore_ascii_case(".git")
        || name.eq_ignore_ascii_case("WinSxS")
    {
        return true;
    }
    parent_is_root && matches_drive_skip(name)
}

fn vol_of(flags: u32) -> u8 {
    (flags >> 16) as u8
}

enum Poll {
    Quiet,
    Dirty,
    Busy,
    Reset,
}

pub struct Catalog {
    current: RwLock<Arc<Index>>,
    count: AtomicU64,
    done: AtomicBool,
    pending: AtomicU64,
}

impl Catalog {
    pub fn new() -> Self {
        Self {
            current: RwLock::new(Arc::new(Index::default())),
            count: AtomicU64::new(0),
            done: AtomicBool::new(false),
            pending: AtomicU64::new(0),
        }
    }

    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Relaxed)
    }

    pub fn read<T>(&self, f: impl FnOnce(&Index) -> T) -> T {
        let index = {
            let guard = self.current.read().unwrap_or_else(|err| err.into_inner());
            Arc::clone(&guard)
        };
        f(&index)
    }

    pub fn restore(&self, index: Index) {
        self.swap(index, false);
    }

    fn swap(&self, index: Index, done: bool) {
        self.count.store(index.live(), Ordering::Relaxed);
        self.done.store(done, Ordering::Relaxed);
        *self.current.write().unwrap_or_else(|err| err.into_inner()) = Arc::new(index);
    }

    fn publish_clone(&self, index: &Index) {
        self.count.store(index.live(), Ordering::Relaxed);
        *self.current.write().unwrap_or_else(|err| err.into_inner()) = Arc::new(index.clone());
    }

    fn with_unique(&self, f: impl FnOnce(&mut Index)) -> bool {
        let live;
        {
            let mut guard = self.current.write().unwrap_or_else(|err| err.into_inner());
            if Arc::strong_count(&guard) > 1 {
                return false;
            }
            let Some(index) = Arc::get_mut(&mut guard) else {
                return false;
            };
            f(index);
            live = index.live();
        }
        self.count.store(live, Ordering::Relaxed);
        true
    }

    pub fn walk(&self, drives: Vec<PathBuf>, profile: Option<PathBuf>) {
        if !self.rebuild_usn(&drives, profile.as_deref()) {
            self.rebuild_dirs(&drives, profile.as_deref());
        }
        let mut watcher: Option<ntfs::Watcher> = None;
        let mut dirty_since: Option<Instant> = None;
        loop {
            if watcher.is_none() {
                let cursors = self.read(|index| {
                    index
                        .volumes
                        .iter()
                        .map(|volume| (volume.letter, volume.journal_id, volume.next_usn))
                        .collect::<Vec<_>>()
                });
                watcher = ntfs::Watcher::open(&cursors);
            }
            if let Some(watch) = watcher.as_mut() {
                match watch.wait(2_000) {
                    ntfs::Wake::Quiet => self.on_quiet(&mut dirty_since),
                    ntfs::Wake::Reset { vol } => {
                        let _ = vol;
                        watcher = None;
                        self.done.store(false, Ordering::Relaxed);
                        if !self.rebuild_usn(&drives, profile.as_deref()) {
                            self.rebuild_dirs(&drives, profile.as_deref());
                        }
                        dirty_since = None;
                    }
                    ntfs::Wake::Changes { vol, records, next_usn } => {
                        if self.apply_wake(vol, &records, next_usn) {
                            watch.commit(vol, next_usn);
                            self.flush_due(false, &mut dirty_since);
                        } else {
                            thread::sleep(Duration::from_millis(20));
                        }
                    }
                }
            } else {
                thread::sleep(Duration::from_secs(2));
                if matches!(self.poll_usn(), Poll::Reset) {
                    self.done.store(false, Ordering::Relaxed);
                    if !self.rebuild_usn(&drives, profile.as_deref()) {
                        self.rebuild_dirs(&drives, profile.as_deref());
                    }
                    dirty_since = None;
                } else {
                    self.on_quiet(&mut dirty_since);
                }
            }
        }
    }

    fn rebuild_usn(&self, drives: &[PathBuf], profile: Option<&Path>) -> bool {
        self.done.store(false, Ordering::Relaxed);
        let saved = self.count();
        let mut index = Index::default();
        let mut any = false;
        let mut missed = Vec::new();
        for drive in drives {
            let Some(letter) = ntfs::drive_letter(drive) else {
                missed.push(drive.clone());
                continue;
            };
            match ntfs::scan(letter) {
                Ok(snapshot) => {
                    index.mount(letter, snapshot, profile);
                    any = true;
                }
                Err(()) => missed.push(drive.clone()),
            }
        }
        if !any {
            return false;
        }
        if !missed.is_empty() {
            fill_dirs(&mut index, &missed, profile, saved, &mut |_| {});
        }
        if saved > 10_000 && index.live() * 2 < saved {
            return false;
        }
        let _ = index.store(&store_path());
        self.swap(index, true);
        true
    }

    fn rebuild_dirs(&self, drives: &[PathBuf], profile: Option<&Path>) {
        self.done.store(false, Ordering::Relaxed);
        let saved = self.count();
        let mut index = Index::default();
        fill_dirs(&mut index, drives, profile, saved, &mut |built| self.publish_clone(built));
        let _ = index.store(&store_path());
        self.swap(index, true);
    }

    fn poll_usn(&self) -> Poll {
        let cursors = self.read(|index| index.volumes.clone());
        if cursors.is_empty() {
            return Poll::Quiet;
        }
        let mut batches = Vec::new();
        for (vol, cursor) in cursors.iter().enumerate() {
            if cursor.journal_id == 0 {
                continue;
            }
            match ntfs::delta(cursor.letter, cursor.journal_id, cursor.next_usn) {
                Delta::Reset => return Poll::Reset,
                Delta::Unavailable => {}
                Delta::Changes { records, journal } => batches.push((vol as u8, records, journal)),
            }
        }
        let mut dirty = false;
        let ran = self.with_unique(|index| {
            for (vol, records, journal) in &batches {
                let vol = *vol as usize;
                if !records.is_empty() {
                    index.apply_volume(vol as u8, records);
                    dirty = true;
                }
                if let Some(cursor) = index.volumes.get_mut(vol) {
                    cursor.journal_id = journal.id;
                    cursor.next_usn = journal.next_usn;
                }
            }
        });
        if !ran {
            return Poll::Busy;
        }
        if dirty {
            self.pending.fetch_add(1, Ordering::Relaxed);
            Poll::Dirty
        } else {
            Poll::Quiet
        }
    }

    fn apply_wake(&self, vol: usize, records: &[ntfs::Change], next_usn: i64) -> bool {
        let mut dirty = false;
        let ran = self.with_unique(|index| {
            if !records.is_empty() {
                index.apply_volume(vol as u8, records);
                dirty = true;
            }
            if let Some(cursor) = index.volumes.get_mut(vol) {
                cursor.next_usn = next_usn;
            }
        });
        if !ran {
            return false;
        }
        if dirty {
            self.pending.fetch_add(1, Ordering::Relaxed);
        }
        true
    }

    fn on_quiet(&self, dirty_since: &mut Option<Instant>) {
        let snap = {
            let guard = self.current.read().unwrap_or_else(|err| err.into_inner());
            Arc::clone(&guard)
        };
        if snap.worth_compacting() {
            let fresh = snap.compact();
            if fresh.store(&store_path()) {
                let done = self.is_done();
                self.swap(fresh, done);
                self.pending.store(0, Ordering::Relaxed);
                *dirty_since = None;
            }
            return;
        }
        self.flush_due(true, dirty_since);
    }

    fn flush_due(&self, force: bool, dirty_since: &mut Option<Instant>) {
        let gen = self.pending.load(Ordering::Relaxed);
        if gen == 0 {
            *dirty_since = None;
            return;
        }
        if dirty_since.is_none() {
            *dirty_since = Some(Instant::now());
        }
        let waited = dirty_since.is_some_and(|start| start.elapsed() >= Duration::from_secs(2));
        if !force && !waited {
            return;
        }
        let snap = {
            let guard = self.current.read().unwrap_or_else(|err| err.into_inner());
            Arc::clone(&guard)
        };
        if snap.store(&store_path()) {
            let _ = self.pending.compare_exchange(gen, 0, Ordering::Relaxed, Ordering::Relaxed);
            *dirty_since = None;
        }
    }
}

fn fill_dirs(index: &mut Index, drives: &[PathBuf], profile: Option<&Path>, saved: u64, progress: &mut dyn FnMut(&Index)) {
    let mut queue = VecDeque::new();
    let mut last_publish = Instant::now();
    for drive in drives {
        let label = drive_label(drive);
        let id = index.push(u32::MAX, &label, true);
        queue.push_back((drive.clone(), id));
    }
    while let Some((path, parent)) = queue.pop_front() {
        if index.len() >= CAP {
            break;
        }
        let children = read_children(&path);
        let mut inserted = Vec::with_capacity(children.len());
        for child in &children {
            if index.len() >= CAP {
                break;
            }
            let id = index.push(parent, &child.name, child.dir);
            inserted.push(id);
        }
        let interval = if index.len() >= 100_000 {
            Duration::from_millis(4000)
        } else {
            Duration::from_millis(1200)
        };
        let caught_up = saved == 0 || index.len() as u64 >= saved;
        if saved == 0 && caught_up && last_publish.elapsed() >= interval {
            progress(index);
            last_publish = Instant::now();
        }
        let inside = profile.is_some_and(|profile| path.starts_with(profile));
        let mut normal = Vec::new();
        let mut priority = None;
        for (child, id) in children.into_iter().zip(inserted) {
            if !child.dir || child.reparse || skip_children(&path, &child.name) {
                continue;
            }
            let next = (child.path, id);
            if inside {
                normal.push(next);
            } else if profile.is_some_and(|profile| profile.starts_with(&next.0)) {
                priority = Some(next);
            } else {
                normal.push(next);
            }
        }
        if inside {
            for item in normal.into_iter().rev() {
                queue.push_front(item);
            }
        } else {
            for item in normal {
                queue.push_back(item);
            }
            if let Some(item) = priority {
                queue.push_front(item);
            }
        }
    }
}

pub fn store_path() -> PathBuf {
    let mut path = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    path.push("findthing");
    let _ = std::fs::create_dir_all(&path);
    path.push("index.bin");
    path
}

fn read_u32(input: &mut impl std::io::Read) -> Option<u32> {
    let mut buf = [0u8; 4];
    input.read_exact(&mut buf).ok()?;
    Some(u32::from_le_bytes(buf))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Key {
    Ascii(u8),
    Other(char),
}

fn ascii_slot(ch: char) -> Option<u8> {
    match ch {
        'a'..='z' => Some(ch as u8 - b'a'),
        'A'..='Z' => Some(ch as u8 - b'A'),
        '0'..='9' => Some(26 + (ch as u8 - b'0')),
        _ => None,
    }
}

fn match_keys(needle: &str) -> Vec<Key> {
    let mut keys = Vec::new();
    let mut seen = [false; 36];
    let mut others = Vec::new();
    for ch in needle.chars() {
        if let Some(slot) = ascii_slot(ch) {
            if !seen[slot as usize] {
                seen[slot as usize] = true;
                keys.push(Key::Ascii(slot));
            }
        } else if !ch.is_ascii() && !others.contains(&ch) {
            others.push(ch);
            keys.push(Key::Other(ch));
        }
    }
    keys
}

struct Child {
    name: String,
    path: PathBuf,
    dir: bool,
    reparse: bool,
}

fn read_children(path: &Path) -> Vec<Child> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        FindClose, FindFirstFileExW, FindNextFileW, FIND_FIRST_EX_LARGE_FETCH, FindExInfoBasic,
        FindExSearchNameMatch, WIN32_FIND_DATAW,
    };

    let mut text = long_path(path).to_string_lossy().into_owned();
    if !text.ends_with('\\') {
        text.push('\\');
    }
    text.push('*');
    let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let mut data = WIN32_FIND_DATAW::default();
    let handle = unsafe {
        FindFirstFileExW(
            PCWSTR(wide.as_ptr()),
            FindExInfoBasic,
            &mut data as *mut _ as *mut _,
            FindExSearchNameMatch,
            None,
            FIND_FIRST_EX_LARGE_FETCH,
        )
    };
    let Ok(handle) = handle else {
        return Vec::new();
    };
    let mut children = Vec::new();
    loop {
        let name = wide_text(&data.cFileName);
        if !name.is_empty() && name != "." && name != ".." {
            let dir = data.dwFileAttributes & 0x10 != 0;
            let reparse = data.dwFileAttributes & 0x400 != 0;
            children.push(Child {
                path: path.join(&name),
                name,
                dir,
                reparse,
            });
        }
        if unsafe { FindNextFileW(handle, &mut data) }.is_err() {
            break;
        }
    }
    unsafe {
        let _ = FindClose(handle);
    }
    children
}

fn wide_text(wide: &[u16]) -> String {
    let end = wide.iter().position(|unit| *unit == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..end])
}

fn skip_children(parent: &Path, name: &str) -> bool {
    if name.eq_ignore_ascii_case("node_modules")
        || name.eq_ignore_ascii_case(".git")
        || name.eq_ignore_ascii_case("WinSxS")
    {
        return true;
    }
    is_drive_root(parent) && matches_drive_skip(name)
}

fn matches_drive_skip(name: &str) -> bool {
    name.eq_ignore_ascii_case("Windows")
        || name.eq_ignore_ascii_case("$Recycle.Bin")
        || name.eq_ignore_ascii_case("System Volume Information")
        || name.eq_ignore_ascii_case("Recovery")
        || name.eq_ignore_ascii_case("Config.Msi")
}

fn is_drive_root(path: &Path) -> bool {
    let mut components = path.components();
    let Some(first) = components.next() else {
        return false;
    };
    if !matches!(first, Component::Prefix(_)) {
        return false;
    }
    match components.next() {
        None => true,
        Some(Component::RootDir) => components.next().is_none(),
        _ => false,
    }
}

fn drive_label(path: &Path) -> String {
    path.to_string_lossy().trim_end_matches(['\\', '/']).to_string()
}

fn long_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if text.len() >= 248 && !text.starts_with(r"\\?\") {
        PathBuf::from(format!(r"\\?\{text}"))
    } else {
        path.to_path_buf()
    }
}

pub fn fixed_drives() -> Vec<PathBuf> {
    use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
    use windows::core::PCWSTR;

    let mask = unsafe { GetLogicalDrives() };
    let mut drives = Vec::new();
    for index in 0..26 {
        if mask & (1 << index) == 0 {
            continue;
        }
        let letter = b'A' + index;
        let mut wide = [0u16; 4];
        wide[0] = letter as u16;
        wide[1] = b':' as u16;
        wide[2] = b'\\' as u16;
        let kind = unsafe { GetDriveTypeW(PCWSTR(wide.as_ptr())) };
        if kind == 3 {
            drives.push(PathBuf::from(format!("{}:\\", letter as char)));
        }
    }
    drives
}

pub fn relax_thread() {
    use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_windows_path() {
        let mut index = Index::default();
        let drive = index.push(u32::MAX, "C:", true);
        let users = index.push(drive, "Users", true);
        let file = index.push(users, "笔记.txt", false);
        assert_eq!(index.path(file), r"C:\Users\笔记.txt");
        assert!(!index.is_dir(file));
        assert_eq!(index.parent(file), users);
    }

    #[test]
    fn a_saved_index_roundtrips() {
        let mut index = Index::default();
        let drive = index.push(u32::MAX, "D:", true);
        let folder = index.push(drive, "工作", true);
        let file = index.push(folder, "报告.pdf", false);
        let path = std::env::temp_dir().join(format!("findthing-index-{}.bin", std::process::id()));
        assert!(index.store(&path));
        let loaded = Index::load(&path).expect("load");
        let _ = std::fs::remove_file(&path);
        assert_eq!(loaded.len(), index.len());
        assert_eq!(loaded.path(file), r"D:\工作\报告.pdf");
        assert!(loaded.is_dir(folder));
        assert!(!loaded.is_dir(file));
        assert!(loaded.has_query_chars(file, "pdf"));
    }

    #[test]
    fn an_older_index_still_loads() {
        let path = std::env::temp_dir().join(format!("findthing-v1-{}.bin", std::process::id()));
        {
            use std::io::Write;
            let mut file = std::fs::File::create(&path).unwrap();
            file.write_all(b"FTIX").unwrap();
            file.write_all(&1u32.to_le_bytes()).unwrap();
            file.write_all(&1u32.to_le_bytes()).unwrap();
            file.write_all(&u32::MAX.to_le_bytes()).unwrap();
            file.write_all(&FLAG_DIR.to_le_bytes()).unwrap();
            file.write_all(&2u32.to_le_bytes()).unwrap();
            file.write_all(b"C:").unwrap();
        }
        let loaded = Index::load(&path).expect("v1");
        let _ = std::fs::remove_file(&path);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.name(0), "C:");
    }

    #[test]
    fn compact_drops_deleted_names_and_keeps_the_path() {
        let mut index = Index::default();
        let drive = index.push(u32::MAX, "D:", true);
        let folder = index.push(drive, "work", true);
        let keep = index.push(folder, "keep.txt", false);
        let drop_id = index.push(folder, "old.txt", false);
        index.remove_tree(drop_id);
        index.retarget(keep, folder, "kept.txt", false);
        assert!(index.wasted > 0);
        assert_eq!(index.deleted, 1);
        let fresh = index.compact();
        assert_eq!(fresh.live(), 3);
        assert_eq!(fresh.len() as u64, fresh.live());
        assert_eq!(fresh.deleted, 0);
        assert_eq!(fresh.wasted, 0);
        let kept = (0..fresh.len() as u32).find(|id| fresh.name(*id) == "kept.txt").unwrap();
        assert_eq!(fresh.path(kept), r"D:\work\kept.txt");
        assert!(fresh.has_query_chars(kept, "kept"));
        assert!((0..fresh.len() as u32).all(|id| fresh.name(id) != "old.txt"));
        let used: usize = (0..fresh.len()).map(|id| fresh.name(id as u32).len()).sum();
        assert_eq!(fresh.names.len(), used);
    }

    #[test]
    fn recognizes_drive_roots() {
        assert!(is_drive_root(Path::new(r"C:\")));
        assert!(!is_drive_root(Path::new(r"C:\Windows")));
    }
}
