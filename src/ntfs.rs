use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, GetVolumeInformationW, FILE_FLAG_OVERLAPPED, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::{CancelIoEx, DeviceIoControl, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::{
    CreateEventW, ResetEvent, WaitForMultipleObjects, WaitForSingleObject,
};
use windows::Win32::System::Ioctl::{
    FSCTL_CREATE_USN_JOURNAL, FSCTL_ENUM_USN_DATA, FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_USN_JOURNAL,
    READ_USN_JOURNAL_DATA_V0, USN_JOURNAL_DATA_V0, USN_REASON_FILE_CREATE, USN_REASON_FILE_DELETE,
    USN_REASON_RENAME_NEW_NAME, USN_REASON_RENAME_OLD_NAME,
};

pub const ROOT_FRN: u64 = 5;
const FRN_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
const GENERIC_READ: u32 = 0x8000_0000;

pub const REASON_CREATE: u32 = USN_REASON_FILE_CREATE;
pub const REASON_DELETE: u32 = USN_REASON_FILE_DELETE;
pub const REASON_RENAME_OLD: u32 = USN_REASON_RENAME_OLD_NAME;
pub const REASON_RENAME_NEW: u32 = USN_REASON_RENAME_NEW_NAME;

pub struct FileRec {
    pub frn: u64,
    pub parent: u64,
    pub attrs: u32,
    pub name: String,
}

pub struct Journal {
    pub id: u64,
    pub next_usn: i64,
}

pub struct Snapshot {
    pub records: Vec<FileRec>,
    pub journal: Journal,
}

pub struct Change {
    pub frn: u64,
    pub parent: u64,
    pub attrs: u32,
    pub reason: u32,
    pub name: String,
}

pub enum Delta {
    Changes { records: Vec<Change>, journal: Journal },
    Reset,
    Unavailable,
}

struct Volume {
    handle: HANDLE,
}

impl Drop for Volume {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

pub fn scan(letter: u8) -> Result<Snapshot, ()> {
    if !is_ntfs(letter) {
        return Err(());
    }
    let volume = open_volume(letter).ok_or(())?;
    let started = volume.journal().ok_or(())?;
    let mut records = volume.enumerate().map_err(|_| ())?;
    records = dedupe(records);
    if let Delta::Changes { records: changes, .. } = volume.changes_from(started.id, started.next_usn) {
        apply_snapshot(&mut records, changes);
    }
    let journal = volume.journal().unwrap_or(started);
    Ok(Snapshot { records, journal })
}

fn dedupe(records: Vec<FileRec>) -> Vec<FileRec> {
    let mut map: HashMap<u64, FileRec> = HashMap::with_capacity(records.len());
    for rec in records {
        map.insert(rec.frn, rec);
    }
    map.into_values().collect()
}

pub fn delta(letter: u8, journal_id: u64, from: i64) -> Delta {
    let Some(volume) = open_volume(letter) else {
        return Delta::Unavailable;
    };
    let Some(current) = volume.journal() else {
        return Delta::Reset;
    };
    if current.id != journal_id || from < 0 || from > current.next_usn {
        return Delta::Reset;
    }
    volume.changes_from(journal_id, from)
}

fn apply_snapshot(records: &mut Vec<FileRec>, changes: Vec<Change>) {
    let mut index: HashMap<u64, usize> = HashMap::with_capacity(records.len());
    for (at, rec) in records.iter().enumerate() {
        index.insert(rec.frn, at);
    }
    for change in changes {
        let make = change.reason & (REASON_CREATE | REASON_RENAME_NEW) != 0;
        let drop = change.reason & (REASON_DELETE | REASON_RENAME_OLD) != 0;
        if make {
            let rec = FileRec {
                frn: change.frn,
                parent: change.parent,
                attrs: change.attrs,
                name: change.name,
            };
            if let Some(&at) = index.get(&rec.frn) {
                records[at] = rec;
            } else {
                index.insert(rec.frn, records.len());
                records.push(rec);
            }
        } else if drop {
            if let Some(at) = index.remove(&change.frn) {
                records.swap_remove(at);
                if at < records.len() {
                    index.insert(records[at].frn, at);
                }
            }
        }
    }
}

fn is_ntfs(letter: u8) -> bool {
    let root: Vec<u16> = format!("{}:\\", letter as char).encode_utf16().chain(Some(0)).collect();
    let mut name = [0u16; 16];
    unsafe {
        GetVolumeInformationW(PCWSTR(root.as_ptr()), None, None, None, None, Some(&mut name)).is_ok()
            && String::from_utf16_lossy(&name).trim_end_matches('\0').eq_ignore_ascii_case("NTFS")
    }
}

fn open_volume(letter: u8) -> Option<Volume> {
    let path: Vec<u16> = format!("\\\\.\\{}:", letter as char).encode_utf16().chain(Some(0)).collect();
    let handle = unsafe {
        CreateFileW(
            PCWSTR(path.as_ptr()),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_ZERO,
            None,
        )
        .ok()?
    };
    Some(Volume { handle })
}

const FILE_FLAGS_ZERO: windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES =
    windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(0);

impl Volume {
    fn journal(&self) -> Option<Journal> {
        match self.query_journal() {
            Some(journal) => Some(journal),
            None => {
                self.create_journal();
                self.query_journal()
            }
        }
    }

    fn query_journal(&self) -> Option<Journal> {
        let mut data = USN_JOURNAL_DATA_V0::default();
        let mut returned = 0u32;
        unsafe {
            DeviceIoControl(
                self.handle,
                FSCTL_QUERY_USN_JOURNAL,
                None,
                0,
                Some(&mut data as *mut _ as *mut _),
                std::mem::size_of::<USN_JOURNAL_DATA_V0>() as u32,
                Some(&mut returned),
                None,
            )
            .ok()?;
        }
        if returned < 24 {
            return None;
        }
        Some(Journal {
            id: data.UsnJournalID,
            next_usn: data.NextUsn,
        })
    }

    fn create_journal(&self) {
        let mut body = [0u8; 16];
        body[0..8].copy_from_slice(&0x0100_0000u64.to_le_bytes());
        body[8..16].copy_from_slice(&0x0080_0000u64.to_le_bytes());
        let mut returned = 0u32;
        unsafe {
            let _ = DeviceIoControl(
                self.handle,
                FSCTL_CREATE_USN_JOURNAL,
                Some(body.as_ptr().cast()),
                body.len() as u32,
                None,
                0,
                Some(&mut returned),
                None,
            );
        }
    }

    fn enumerate(&self) -> Result<Vec<FileRec>, u32> {
        let mut records = Vec::new();
        let mut start = 0u64;
        let mut buffer = vec![0u8; 1024 * 1024];
        loop {
            let input = enum_input(start);
            let mut returned = 0u32;
            let read = unsafe {
                DeviceIoControl(
                    self.handle,
                    FSCTL_ENUM_USN_DATA,
                    Some(input.as_ptr().cast()),
                    input.len() as u32,
                    Some(buffer.as_mut_ptr().cast()),
                    buffer.len() as u32,
                    Some(&mut returned),
                    None,
                )
            };
            if let Err(err) = read {
                let code = win32_code(&err);
                if code == 38 {
                    break;
                }
                return Err(code);
            }
            if returned < 8 {
                break;
            }
            let next = u64::from_le_bytes(buffer[0..8].try_into().unwrap());
            push_records(&buffer[8..returned as usize], false, |frn, parent, _reason, attrs, name| {
                if accept_name(&name) {
                    records.push(FileRec { frn, parent, attrs, name });
                }
            });
            if next <= start {
                break;
            }
            start = next;
        }
        Ok(records)
    }

    fn changes_from(&self, journal_id: u64, mut from: i64) -> Delta {
        let mut records = Vec::new();
        let mut buffer = vec![0u8; 256 * 1024];
        let mask = REASON_CREATE | REASON_DELETE | REASON_RENAME_OLD | REASON_RENAME_NEW;
        loop {
            let input = READ_USN_JOURNAL_DATA_V0 {
                StartUsn: from,
                ReasonMask: mask,
                ReturnOnlyOnClose: 1,
                Timeout: 0,
                BytesToWaitFor: 0,
                UsnJournalID: journal_id,
            };
            let mut returned = 0u32;
            let read = unsafe {
                DeviceIoControl(
                    self.handle,
                    FSCTL_READ_USN_JOURNAL,
                    Some(&input as *const _ as *const _),
                    std::mem::size_of::<READ_USN_JOURNAL_DATA_V0>() as u32,
                    Some(buffer.as_mut_ptr().cast()),
                    buffer.len() as u32,
                    Some(&mut returned),
                    None,
                )
            };
            if let Err(err) = read {
                let code = win32_code(&err);
                if code == 38 {
                    break;
                }
                if code == 1178 || code == 1179 || code == 1181 {
                    return Delta::Reset;
                }
                return Delta::Unavailable;
            }
            if returned <= 8 {
                break;
            }
            let next = i64::from_le_bytes(buffer[0..8].try_into().unwrap());
            push_records(&buffer[8..returned as usize], true, |frn, parent, reason, attrs, name| {
                if accept_name(&name) {
                    records.push(Change {
                        frn,
                        parent,
                        attrs,
                        reason,
                        name,
                    });
                }
            });
            if next <= from {
                break;
            }
            from = next;
            if records.len() > 400_000 {
                return Delta::Reset;
            }
        }
        let Some(journal) = self.journal() else {
            return Delta::Reset;
        };
        Delta::Changes { records, journal }
    }
}

fn accept_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('\\') && !name.contains('/')
}

fn enum_input(start: u64) -> [u8; 28] {
    let mut input = [0u8; 28];
    input[0..8].copy_from_slice(&start.to_le_bytes());
    input[16..24].copy_from_slice(&i64::MAX.to_le_bytes());
    input[24..26].copy_from_slice(&2u16.to_le_bytes());
    input[26..28].copy_from_slice(&2u16.to_le_bytes());
    input
}

fn push_records(buf: &[u8], with_reason: bool, mut each: impl FnMut(u64, u64, u32, u32, String)) {
    let mut offset = 0usize;
    while offset + 60 <= buf.len() {
        let len = u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap()) as usize;
        if len < 60 || offset + len > buf.len() {
            break;
        }
        let rec = &buf[offset..offset + len];
        let major = u16::from_le_bytes([rec[4], rec[5]]);
        if major == 2 {
            if let Some(parsed) = parse_v2(rec, with_reason) {
                each(parsed.0, parsed.1, parsed.2, parsed.3, parsed.4);
            }
        }
        let step = len.max(8);
        offset += step;
        if step == 0 {
            break;
        }
    }
}

fn parse_v2(rec: &[u8], with_reason: bool) -> Option<(u64, u64, u32, u32, String)> {
    let frn = u64::from_le_bytes(rec[8..16].try_into().ok()?) & FRN_MASK;
    let parent = u64::from_le_bytes(rec[16..24].try_into().ok()?) & FRN_MASK;
    let reason = if with_reason {
        u32::from_le_bytes(rec[40..44].try_into().ok()?)
    } else {
        0
    };
    let attrs = u32::from_le_bytes(rec[52..56].try_into().ok()?);
    let name_len = u16::from_le_bytes([rec[56], rec[57]]) as usize;
    let name_off = u16::from_le_bytes([rec[58], rec[59]]) as usize;
    if name_len == 0 || name_off.saturating_add(name_len) > rec.len() || name_len % 2 != 0 {
        return None;
    }
    let mut wide = Vec::with_capacity(name_len / 2);
    for pair in rec[name_off..name_off + name_len].chunks_exact(2) {
        wide.push(u16::from_le_bytes([pair[0], pair[1]]));
    }
    let name = String::from_utf16_lossy(&wide);
    if frn == 0 || frn == parent {
        return None;
    }
    Some((frn, parent, reason, attrs, name))
}

fn win32_code(err: &windows::core::Error) -> u32 {
    let code = err.code().0 as u32;
    if code & 0xFFFF_0000 == 0x8007_0000 {
        code & 0xFFFF
    } else {
        code
    }
}

pub enum Wake {
    Quiet,
    Reset { vol: usize },
    Changes { vol: usize, records: Vec<Change>, next_usn: i64 },
}

pub struct Watcher {
    slots: Vec<Slot>,
}

struct Slot {
    vol: usize,
    handle: HANDLE,
    event: HANDLE,
    overlapped: Box<OVERLAPPED>,
    input: Box<READ_USN_JOURNAL_DATA_V0>,
    buffer: Vec<u8>,
    journal_id: u64,
    from: i64,
    pending: bool,
}

impl Watcher {
    pub fn open(cursors: &[(u8, u64, i64)]) -> Option<Self> {
        if cursors.is_empty() || cursors.len() > 64 {
            return None;
        }
        let mut slots = Vec::new();
        for (vol, (letter, journal_id, next_usn)) in cursors.iter().enumerate() {
            if *journal_id == 0 {
                continue;
            }
            let slot = Slot::open(vol, *letter, *journal_id, *next_usn)?;
            slots.push(slot);
        }
        if slots.is_empty() { None } else { Some(Self { slots }) }
    }

    pub fn wait(&mut self, timeout_ms: u32) -> Wake {
        let deadline = Instant::now() + std::time::Duration::from_millis(timeout_ms as u64);
        loop {
            let remain = deadline.saturating_duration_since(Instant::now()).as_millis() as u32;
            if remain == 0 {
                return Wake::Quiet;
            }
            match self.wait_once(remain) {
                Wake::Quiet => std::thread::sleep(std::time::Duration::from_millis(15)),
                other => return other,
            }
        }
    }

    fn wait_once(&mut self, timeout_ms: u32) -> Wake {
        let mut ready = None;
        let mut reset = None;
        let mut handles = Vec::with_capacity(self.slots.len());
        let mut which = Vec::with_capacity(self.slots.len());
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if !slot.pending {
                match slot.arm() {
                    Arm::Pending => {}
                    Arm::Ready => {
                        ready = Some(index);
                        break;
                    }
                    Arm::Reset => {
                        reset = Some(slot.vol);
                        break;
                    }
                }
            }
            handles.push(slot.event);
            which.push(index);
        }
        if let Some(vol) = reset {
            return Wake::Reset { vol };
        }
        if let Some(index) = ready {
            return self.collect(index);
        }
        if handles.is_empty() {
            return Wake::Quiet;
        }
        let status = unsafe { WaitForMultipleObjects(&handles, false, timeout_ms) };
        let code = status.0;
        if (WAIT_OBJECT_0.0..WAIT_OBJECT_0.0 + handles.len() as u32).contains(&code) {
            let index = which[(code - WAIT_OBJECT_0.0) as usize];
            return self.collect(index);
        }
        Wake::Quiet
    }

    pub fn commit(&mut self, vol: usize, next_usn: i64) {
        if let Some(slot) = self.slots.iter_mut().find(|slot| slot.vol == vol) {
            slot.from = next_usn;
        }
    }

    fn collect(&mut self, index: usize) -> Wake {
        let Some(parsed) = self.slots[index].take() else {
            return Wake::Reset { vol: self.slots[index].vol };
        };
        let vol = self.slots[index].vol;
        if parsed.0 < self.slots[index].from {
            return Wake::Reset { vol };
        }
        if parsed.1.is_empty() && parsed.0 <= self.slots[index].from {
            return Wake::Quiet;
        }
        Wake::Changes {
            vol,
            records: parsed.1,
            next_usn: parsed.0,
        }
    }
}

enum Arm {
    Pending,
    Ready,
    Reset,
}

impl Slot {
    fn open(vol: usize, letter: u8, journal_id: u64, from: i64) -> Option<Self> {
        let path: Vec<u16> = format!("\\\\.\\{}:", letter as char).encode_utf16().chain(Some(0)).collect();
        let handle = unsafe {
            CreateFileW(
                PCWSTR(path.as_ptr()),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                None,
            )
            .ok()?
        };
        let event = unsafe { CreateEventW(None, true, false, None).ok()? };
        Some(Self {
            vol,
            handle,
            event,
            overlapped: Box::new(unsafe { std::mem::zeroed() }),
            input: Box::new(READ_USN_JOURNAL_DATA_V0::default()),
            buffer: vec![0u8; 256 * 1024],
            journal_id,
            from,
            pending: false,
        })
    }

    fn arm(&mut self) -> Arm {
        unsafe {
            let _ = ResetEvent(self.event);
            *self.overlapped = std::mem::zeroed();
            self.overlapped.hEvent = self.event;
        }
        *self.input = READ_USN_JOURNAL_DATA_V0 {
            StartUsn: self.from,
            ReasonMask: REASON_CREATE | REASON_DELETE | REASON_RENAME_OLD | REASON_RENAME_NEW,
            ReturnOnlyOnClose: 1,
            Timeout: 5,
            BytesToWaitFor: 1,
            UsnJournalID: self.journal_id,
        };
        let read = unsafe {
            DeviceIoControl(
                self.handle,
                FSCTL_READ_USN_JOURNAL,
                Some(&*self.input as *const _ as *const _),
                std::mem::size_of::<READ_USN_JOURNAL_DATA_V0>() as u32,
                Some(self.buffer.as_mut_ptr().cast()),
                self.buffer.len() as u32,
                None,
                Some(self.overlapped.as_mut()),
            )
        };
        match read {
            Ok(()) => Arm::Ready,
            Err(err) if win32_code(&err) == 997 => {
                self.pending = true;
                Arm::Pending
            }
            Err(err) if matches!(win32_code(&err), 38 | 1178 | 1179 | 1181) => Arm::Reset,
            Err(_) => Arm::Reset,
        }
    }

    fn take(&mut self) -> Option<(i64, Vec<Change>)> {
        self.pending = false;
        let mut bytes = 0u32;
        let read = unsafe { GetOverlappedResult(self.handle, self.overlapped.as_ref(), &mut bytes, false) };
        if let Err(err) = read {
            let code = win32_code(&err);
            if code == 38 || code == 995 || code == 1460 {
                return Some((self.from, Vec::new()));
            }
            return None;
        }
        if bytes < 8 {
            return Some((self.from, Vec::new()));
        }
        let next = i64::from_le_bytes(self.buffer[0..8].try_into().ok()?);
        let mut records = Vec::new();
        if bytes > 8 {
            push_records(&self.buffer[8..bytes as usize], true, |frn, parent, reason, attrs, name| {
                if accept_name(&name) {
                    records.push(Change { frn, parent, attrs, reason, name });
                }
            });
        }
        Some((next, records))
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        for slot in &mut self.slots {
            unsafe {
                if slot.pending {
                    let _ = CancelIoEx(slot.handle, Some(slot.overlapped.as_mut()));
                    let _ = WaitForSingleObject(slot.event, 1000);
                    slot.pending = false;
                }
                let _ = CloseHandle(slot.event);
                let _ = CloseHandle(slot.handle);
            }
        }
    }
}

pub fn drive_letter(path: &Path) -> Option<u8> {
    let text = path.to_string_lossy();
    let mut chars = text.chars();
    let letter = chars.next()?.to_ascii_uppercase();
    if letter.is_ascii_uppercase() && chars.next() == Some(':') {
        Some(letter as u8)
    } else {
        None
    }
}
