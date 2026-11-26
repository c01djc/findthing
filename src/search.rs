use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::apps;
use crate::commands;
use crate::hit::{Hit, Kind};
use crate::index::Index;

pub struct Recents {
    paths: std::sync::Mutex<Vec<String>>,
}

impl Recents {
    pub fn load() -> Self {
        let text = std::fs::read_to_string(recent_path()).unwrap_or_default();
        let paths = text.lines().map(str::trim).filter(|line| !line.is_empty()).take(32).map(str::to_string).collect();
        Self {
            paths: std::sync::Mutex::new(paths),
        }
    }

    pub fn remember(&self, path: &str) {
        if path.is_empty() {
            return;
        }
        let mut paths = self.paths.lock().unwrap_or_else(|err| err.into_inner());
        paths.retain(|saved| !saved.eq_ignore_ascii_case(path));
        paths.insert(0, path.to_string());
        paths.truncate(32);
        let body = paths.join("\n");
        drop(paths);
        let _ = std::fs::write(recent_path(), body);
    }

    pub fn snapshot(&self) -> Vec<String> {
        self.paths.lock().unwrap_or_else(|err| err.into_inner()).clone()
    }
}

pub fn query(index: &Index, raw: &str, limit: usize, folder: &str, recent: &[String]) -> Vec<Hit> {
    let parsed = parse_query(raw);
    if limit == 0 || (parsed.name.is_empty() && parsed.ext.is_none()) {
        return Vec::new();
    }
    let visit = if parsed.name.is_empty() {
        parsed.ext.clone().unwrap_or_default()
    } else {
        parsed.name.clone()
    };

    let mut best: BinaryHeap<Reverse<(i32, u32)>> = BinaryHeap::new();
    let prefixes = index.prefix_ids(&visit);
    let one_letter = parsed.ext.is_none() && one_ascii(&parsed.name);
    if one_letter {
        // A single letter's score depends only on the name length, which is already on the node.
        if let Some(ids) = prefixes {
            for &id in ids {
                if let Some((len, dir)) = index.brief(id) {
                    push_best(&mut best, score_one(len, true, dir), id, limit);
                }
            }
        }
    } else if let Some(ids) = prefixes {
        for &id in ids {
            consider(index, id, &parsed, &visit, &mut best, limit, true, false);
        }
    }
    // A name that starts with the query always outranks one that only contains it.
    // Once the list is full of those, the rest of the disk does not need to be scored.
    let prefixes_done = prefixes.is_some();
    let skip_contains = prefixes_done && best.len() >= limit && worst_score(&best) >= PREFIX_FLOOR;
    if !skip_contains && one_letter {
        index.visit_matches(&visit, |id| {
            if let Some((len, dir)) = index.brief(id) {
                push_best(&mut best, score_one(len, false, dir), id, limit);
            }
        });
    } else if !skip_contains {
        index.visit_matches(&visit, |id| {
            consider(index, id, &parsed, &visit, &mut best, limit, false, prefixes_done);
        });
    }

    let mut hits: Vec<(i32, Hit)> = best
        .into_iter()
        .map(|Reverse((score, id))| {
            let kind = if index.is_dir(id) { Kind::Dir } else { Kind::File };
            let title = index.name(id).to_string();
            let path = index.path(id);
            let score = score + folder_bonus(folder, &path) + recent_bonus(recent, &path);
            let detail = parent_path(&path);
            (
                score,
                Hit {
                    kind,
                    title,
                    detail,
                    target: path,
                },
            )
        })
        .collect();

    if !parsed.name.is_empty() {
        hits.extend(commands::matching(&parsed.name));
        hits.extend(apps::matching(&parsed.name));
    }
    // Apps and ordinary files stay ahead of folders; pictures and media sink.
    hits.sort_by(|a, b| {
        type_tier(&b.1)
            .cmp(&type_tier(&a.1))
            .then_with(|| b.0.cmp(&a.0))
            .then_with(|| a.1.title.len().cmp(&b.1.title.len()))
    });
    hits.truncate(limit);
    hits.into_iter().map(|(_, hit)| hit).collect()
}

pub fn folders(index: &Index, raw: &str, limit: usize) -> Vec<Hit> {
    let name = raw.trim();
    if name.is_empty() || limit == 0 {
        return Vec::new();
    }
    let parsed = Parsed {
        name: name.to_string(),
        ext: None,
    };
    let mut best: BinaryHeap<Reverse<(i32, u32)>> = BinaryHeap::new();
    let prefixes = index.prefix_ids(name);
    if let Some(ids) = prefixes {
        for &id in ids {
            if index.is_dir(id) {
                consider(index, id, &parsed, name, &mut best, limit, true, false);
            }
        }
    }
    let prefixes_done = prefixes.is_some();
    let skip_contains = prefixes_done && best.len() >= limit && worst_score(&best) >= PREFIX_FLOOR;
    if !skip_contains {
        index.visit_matches(name, |id| {
            if index.is_dir(id) {
                consider(index, id, &parsed, name, &mut best, limit, false, prefixes_done);
            }
        });
    }
    let mut hits: Vec<(i32, Hit)> = best
        .into_iter()
        .map(|Reverse((score, id))| {
            let path = index.path(id);
            let detail = parent_path(&path);
            (
                score,
                Hit {
                    kind: Kind::Dir,
                    title: index.name(id).to_string(),
                    detail,
                    target: path,
                },
            )
        })
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.title.len().cmp(&b.1.title.len())));
    hits.truncate(limit);
    hits.into_iter().map(|(_, hit)| hit).collect()
}

pub fn highlight_of(raw: &str) -> String {
    let parsed = parse_query(raw);
    if !parsed.name.is_empty() {
        parsed.name
    } else {
        parsed.ext.unwrap_or_default()
    }
}

struct Parsed {
    name: String,
    ext: Option<String>,
}

fn parse_query(raw: &str) -> Parsed {
    let raw = raw.trim();
    let mut parts: Vec<&str> = raw.split_whitespace().collect();
    if parts.len() >= 2 && is_ext_token(parts[parts.len() - 1]) {
        let ext = parts.pop().unwrap().trim_start_matches('.').to_ascii_lowercase();
        return Parsed {
            name: parts.join(" "),
            ext: Some(ext),
        };
    }
    if let Some(token) = parts.first() {
        if parts.len() == 1 && token.starts_with('.') && is_ext_token(token) {
            return Parsed {
                name: String::new(),
                ext: Some(token.trim_start_matches('.').to_ascii_lowercase()),
            };
        }
    }
    Parsed {
        name: raw.to_string(),
        ext: None,
    }
}

fn is_ext_token(token: &str) -> bool {
    let bare = token.trim_start_matches('.');
    if bare.is_empty() || bare.len() > 8 || !bare.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return false;
    }
    token.starts_with('.') || COMMON_EXT.binary_search(&bare.to_ascii_lowercase().as_str()).is_ok()
}

const COMMON_EXT: &[&str] = &[
    "7z", "avi", "bat", "bmp", "c", "cpp", "csv", "dll", "doc", "docx", "exe", "gif", "gz", "h",
    "html", "iso", "java", "jpeg", "jpg", "js", "json", "log", "md", "mkv", "mov", "mp3", "mp4",
    "pdf", "png", "ppt", "pptx", "ps1", "py", "rar", "rs", "rtf", "svg", "tar", "tif", "tiff",
    "txt", "wav", "webp", "xls", "xlsx", "xml", "zip",
];

fn has_ext(name: &str, ext: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, found)| !found.is_empty() && found.eq_ignore_ascii_case(ext))
}

fn folder_bonus(folder: &str, path: &str) -> i32 {
    let folder = folder.trim_end_matches(['\\', '/']);
    if folder.is_empty() || !path.is_char_boundary(folder.len()) {
        return 0;
    }
    let same = path.len() == folder.len() && path.eq_ignore_ascii_case(folder);
    let inside = path.len() > folder.len()
        && path.as_bytes().get(folder.len()) == Some(&b'\\')
        && path[..folder.len()].eq_ignore_ascii_case(folder);
    if same || inside { 700 } else { 0 }
}

fn recent_bonus(recent: &[String], path: &str) -> i32 {
    match recent.iter().position(|saved| saved.eq_ignore_ascii_case(path)) {
        Some(0) => 520,
        Some(index) if index < 8 => 420,
        Some(_) => 320,
        None => 0,
    }
}

fn recent_path() -> std::path::PathBuf {
    let mut path = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    path.push("findthing");
    let _ = std::fs::create_dir_all(&path);
    path.push("recent.txt");
    path
}

fn parent_path(path: &str) -> String {
    match path.rsplit_once('\\') {
        Some((parent, _)) if !parent.is_empty() => parent.to_string(),
        _ => path.to_string(),
    }
}

const PREFIX_FLOOR: i32 = 900;

fn worst_score(best: &BinaryHeap<Reverse<(i32, u32)>>) -> i32 {
    best.peek().map(|Reverse((score, _))| *score).unwrap_or(0)
}

fn consider(
    index: &Index,
    id: u32,
    parsed: &Parsed,
    needle: &str,
    best: &mut BinaryHeap<Reverse<(i32, u32)>>,
    limit: usize,
    prefix_phase: bool,
    prefixes_done: bool,
) {
    if !index.alive(id) || index.parent(id) == u32::MAX {
        return;
    }
    let name = index.name(id);
    if parsed.ext.as_deref().is_some_and(|ext| !has_ext(name, ext)) {
        return;
    }
    if prefix_phase && !index.has_query_chars(id, needle) {
        return;
    }
    let score = score_name(name, needle, index.is_dir(id));
    if score <= 0 {
        return;
    }
    if prefix_phase && score < PREFIX_FLOOR {
        return;
    }
    if !prefix_phase && prefixes_done && score >= PREFIX_FLOOR {
        return;
    }
    push_best(best, score, id, limit);
}

fn push_best(best: &mut BinaryHeap<Reverse<(i32, u32)>>, score: i32, id: u32, limit: usize) {
    if best.len() >= limit && score <= best.peek().map(|Reverse((worst, _))| *worst).unwrap_or(0) {
        return;
    }
    if best.iter().any(|Reverse((_, existing))| *existing == id) {
        return;
    }
    if best.len() < limit {
        best.push(Reverse((score, id)));
        return;
    }
    best.pop();
    best.push(Reverse((score, id)));
}

fn one_ascii(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(ch) if chars.next().is_none() && ch.is_ascii_alphanumeric())
}

fn score_one(len: u32, starts: bool, dir: bool) -> i32 {
    if len == 0 {
        return 0;
    }
    let bonus = kind_score_bump("", dir);
    let extra = len as i32 - 1;
    if starts {
        if len == 1 {
            2000 + bonus
        } else {
            1400 - extra.min(500) + bonus
        }
    } else {
        760 - (extra / 6).min(250) + bonus
    }
}

pub(crate) fn score_title(name: &str, needle: &str) -> i32 {
    score_name(name, needle, false)
}

fn ext_of(name: &str) -> &str {
    name.rsplit_once('.')
        .map(|(_, ext)| ext)
        .filter(|ext| !ext.is_empty() && ext.len() <= 8)
        .unwrap_or("")
}

fn is_app_name(name: &str) -> bool {
    let ext = ext_of(name);
    ext.eq_ignore_ascii_case("exe") || ext.eq_ignore_ascii_case("lnk")
}

fn is_media_name(name: &str) -> bool {
    let ext = ext_of(name);
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "ico" | "tif" | "tiff"
            | "mp4" | "mkv" | "mov" | "avi" | "wmv" | "webm" | "mp3" | "wav" | "flac" | "aac" | "m4a"
    )
}

/// Higher stays nearer the top of the result list.
fn type_tier(hit: &Hit) -> i8 {
    match hit.kind {
        Kind::Command => 5,
        Kind::Dir => 2,
        Kind::File if hit.detail == "应用" || is_app_name(&hit.title) => 4,
        Kind::File if is_media_name(&hit.title) => 0,
        Kind::File => 3,
    }
}

fn kind_score_bump(name: &str, dir: bool) -> i32 {
    if dir {
        20
    } else if name.is_empty() {
        50
    } else if is_media_name(name) {
        -120
    } else if is_app_name(name) {
        80
    } else {
        50
    }
}

fn score_name(name: &str, needle: &str, dir: bool) -> i32 {
    let bytes = name.as_bytes();
    let query = needle.as_bytes();
    if query.is_empty() || bytes.is_empty() {
        return 0;
    }
    let bonus = kind_score_bump(name, dir);
    if bytes.len() == query.len() && bytes.eq_ignore_ascii_case(query) {
        return 2000 + bonus;
    }
    if bytes.len() >= query.len() && bytes[..query.len()].eq_ignore_ascii_case(query) {
        let extra = (bytes.len() - query.len()) as i32;
        return 1400 - extra.min(500) + bonus;
    }
    if match_at(bytes, query).is_some() {
        let extra = (bytes.len().saturating_sub(query.len())) as i32;
        return 760 - (extra / 6).min(250) + bonus;
    }
    if let Some(score) = score_runs(name, needle) {
        return score + bonus;
    }
    0
}

/// "uu远程" is two runs. A name may put them next to each other, or with a
/// space, dash, or extra letters between, as in "UU-远程" or "UU加速器".
fn score_runs(name: &str, needle: &str) -> Option<i32> {
    let runs = script_runs(needle);
    if runs.len() < 2 {
        return None;
    }
    let folded = name.to_lowercase();
    let mut rest = folded.as_str();
    for run in &runs {
        let piece = run.to_lowercase();
        let at = rest.find(&piece)?;
        rest = &rest[at + piece.len()..];
    }
    let extra = (name.len().saturating_sub(needle.len())) as i32;
    Some(640 - (extra / 8).min(200))
}

fn script_runs(needle: &str) -> Vec<&str> {
    let mut runs = Vec::new();
    let mut start = None;
    let mut ascii = false;
    for (index, ch) in needle.char_indices() {
        let kind = if ch.is_ascii_alphanumeric() {
            Some(true)
        } else if !ch.is_ascii() {
            Some(false)
        } else {
            None
        };
        match (start, kind) {
            (None, Some(next)) => {
                start = Some(index);
                ascii = next;
            }
            (Some(_), Some(next)) if next == ascii => {}
            (Some(from), Some(next)) => {
                runs.push(&needle[from..index]);
                start = Some(index);
                ascii = next;
            }
            (Some(from), None) => {
                runs.push(&needle[from..index]);
                start = None;
            }
            (None, None) => {}
        }
    }
    if let Some(from) = start {
        runs.push(&needle[from..]);
    }
    runs
}

fn match_at(hay: &[u8], needle: &[u8]) -> Option<usize> {
    let n = needle.len();
    if n == 0 || n > hay.len() {
        return None;
    }
    let first = needle[0];
    let mut offset = 0;
    if first.is_ascii_alphabetic() {
        let lower = first.to_ascii_lowercase();
        let upper = lower.to_ascii_uppercase();
        while offset + n <= hay.len() {
            let found = memchr::memchr2(lower, upper, &hay[offset..])?;
            let at = offset + found;
            if at + n <= hay.len() && hay[at..at + n].eq_ignore_ascii_case(needle) {
                return Some(at);
            }
            offset = at + 1;
        }
        return None;
    }
    while offset + n <= hay.len() {
        let found = memchr::memchr(first, &hay[offset..])?;
        let at = offset + found;
        if at + n <= hay.len() && hay[at..at + n].eq_ignore_ascii_case(needle) {
            return Some(at);
        }
        offset = at + 1;
    }
    None
}

pub struct Shown {
    pub lead: String,
    pub hit: String,
    pub tail: String,
}

/// Split a filename so the matched letters can be drawn on their own.
/// The full name stays intact. A row that cannot fit it clips the end.
pub fn shown_parts(title: &str, query: &str) -> Shown {
    let Some(at) = match_at(title.as_bytes(), query.trim().as_bytes()) else {
        return Shown {
            lead: String::new(),
            hit: String::new(),
            tail: title.to_string(),
        };
    };
    let end = at + query.trim().len();
    Shown {
        lead: title[..at].to_string(),
        hit: title[at..end].to_string(),
        tail: title[end..].to_string(),
    }
}

#[cfg(test)]
fn shown_title(title: &str, query: &str) -> String {
    let shown = shown_parts(title, query);
    let mut text = shown.lead;
    text.push_str(&shown.hit);
    text.push_str(&shown.tail);
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::Index;

    fn ask(index: &Index, query_text: &str) -> Vec<Hit> {
        query(index, query_text, 5, "", &[])
    }

    #[test]
    fn a_single_letter_keeps_exact_then_prefix_then_contains() {
        let mut index = Index::default();
        let drive = index.push(u32::MAX, "D:", true);
        index.push(drive, "za.txt", false);
        index.push(drive, "ab.txt", false);
        index.push(drive, "a", false);
        let files = |query_text: &str| {
            query(&index, query_text, 40, "", &[])
                .into_iter()
                .filter(|hit| hit.kind != crate::hit::Kind::Command)
                .map(|hit| hit.title)
                .collect::<Vec<_>>()
        };
        assert_eq!(files("a"), vec!["a", "ab.txt", "za.txt"]);
        assert_eq!(files("A"), vec!["a", "ab.txt", "za.txt"]);
    }

    #[test]
    fn prefix_ranks_above_a_longer_contains_match() {
        let mut index = Index::default();
        let drive = index.push(u32::MAX, "D:", true);
        index.push(drive, "报告-年度.docx", false);
        index.push(drive, "年度报告.docx", false);
        let hits = ask(&index, "年度");
        assert_eq!(hits[0].title, "年度报告.docx");
    }

    #[test]
    fn apps_and_files_outrank_folders_and_pictures() {
        let mut index = Index::default();
        let drive = index.push(u32::MAX, "D:", true);
        index.push(drive, "demo.png", false);
        index.push(drive, "demo", true);
        index.push(drive, "demo.docx", false);
        index.push(drive, "demo.exe", false);
        let titles = ask(&index, "demo")
            .into_iter()
            .filter(|hit| hit.kind != Kind::Command)
            .map(|hit| hit.title)
            .collect::<Vec<_>>();
        assert_eq!(
            titles,
            vec![
                "demo.exe".to_string(),
                "demo.docx".to_string(),
                "demo".to_string(),
                "demo.png".to_string()
            ]
        );
    }

    #[test]
    fn a_command_word_outranks_a_similar_file() {
        let mut index = Index::default();
        let drive = index.push(u32::MAX, "C:", true);
        index.push(drive, "powershell.txt", false);
        let hits = ask(&index, "powershell");
        assert_eq!(hits[0].title, "PowerShell");
        assert_eq!(hits[0].kind, crate::hit::Kind::Command);
    }

    #[test]
    fn a_letter_near_the_end_does_not_crash() {
        let mut index = Index::default();
        let drive = index.push(u32::MAX, "C:", true);
        index.push(drive, "codex-d", false);
        index.push(drive, "文档.doc", false);
        let hits = ask(&index, "doc");
        assert_eq!(hits[0].title, "文档.doc");
    }

    #[test]
    fn a_name_of_letters_and_chinese_matches_the_mixed_query() {
        let mut index = Index::default();
        let drive = index.push(u32::MAX, "C:", true);
        index.push(drive, "UU远程.exe", false);
        index.push(drive, "网易UU远程 setup.exe", false);
        index.push(drive, "uuyc_4.43.0.exe", false);
        index.push(drive, "向日葵远程控制.lnk", false);
        let titles = |index: &Index, text: &str| {
            ask(index, text)
                .into_iter()
                .filter(|hit| hit.kind != crate::hit::Kind::Command)
                .map(|hit| hit.title)
                .collect::<Vec<_>>()
        };
        assert_eq!(titles(&index, "uu远程"), vec!["UU远程.exe", "网易UU远程 setup.exe"]);
        assert_eq!(titles(&index, "UU远程"), vec!["UU远程.exe", "网易UU远程 setup.exe"]);
        assert!(titles(&index, "远程").iter().any(|title| title.contains("远程")));
        assert!(titles(&index, "uu").iter().any(|title| title.eq_ignore_ascii_case("UU远程.exe") || title.contains("uuyc")));
        let shown = shown_parts("UU远程.exe", "uu远程");
        assert_eq!(shown.hit, "UU远程");
        assert_eq!(shown.tail, ".exe");
        assert!(titles(&index, "uu 远程").iter().any(|title| title == "UU远程.exe"));
        index.push(drive, "UU-远程 setup.exe", false);
        let gapped = ask(&index, "uu远程");
        assert!(gapped.iter().any(|hit| hit.title == "UU-远程 setup.exe"));
    }

    #[test]
    fn finds_ascii_names_without_matching_case() {
        let mut index = Index::default();
        let drive = index.push(u32::MAX, "C:", true);
        index.push(drive, "gongyixunqin_RHYKM.zip", false);
        let hits = ask(&index, "Gong");
        assert_eq!(hits[0].title, "gongyixunqin_RHYKM.zip");
        assert!(hits[0].target.ends_with("gongyixunqin_RHYKM.zip"));
    }

    #[test]
    fn a_buried_match_keeps_the_whole_filename() {
        let name = "微信文件缓存目录里的年度报告2025最终版.docx";
        assert_eq!(shown_title(name, "2025"), name);
        assert_eq!(shown_title("ridecircle_final_v49.zip", "zip"), "ridecircle_final_v49.zip");
    }

    #[test]
    fn a_prefix_match_keeps_the_real_filename() {
        let name = "2025-06-微信附件.docx";
        assert_eq!(shown_title(name, "2025"), name);
        assert_eq!(shown_title("报告2025.docx", "2025"), "报告2025.docx");
    }

    #[test]
    fn an_extension_word_keeps_only_that_kind_of_file() {
        let mut index = Index::default();
        let drive = index.push(u32::MAX, "E:", true);
        index.push(drive, "报告.pdf", false);
        index.push(drive, "报告.docx", false);
        let hits = ask(&index, "报告 pdf");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "报告.pdf");
        assert_eq!(highlight_of("报告 pdf"), "报告");
        assert_eq!(highlight_of(".pdf"), "pdf");
    }

    #[test]
    fn the_current_folder_and_a_recent_file_rank_first() {
        let mut index = Index::default();
        let drive = index.push(u32::MAX, "D:", true);
        let here = index.push(drive, "desktop", true);
        let elsewhere = index.push(drive, "archive", true);
        index.push(here, "笔记.txt", false);
        index.push(elsewhere, "笔记.txt", false);
        let recent = vec!["D:\\archive\\笔记.txt".to_string()];
        let hits = query(&index, "笔记", 5, r"D:\desktop", &recent);
        assert_eq!(hits[0].target, r"D:\desktop\笔记.txt");
        let hits = query(&index, "笔记", 5, "", &recent);
        assert_eq!(hits[0].target, r"D:\archive\笔记.txt");
    }

    #[test]
    fn the_matched_letters_keep_the_filename_casing() {
        let shown = shown_parts("gongyixunqin_RHYKM.zip", "Gong");
        assert_eq!(shown.hit, "gong");
        assert_eq!(shown.tail, "yixunqin_RHYKM.zip");
        let year = shown_parts("报告2025最终版.docx", "2025");
        assert_eq!(year.lead, "报告");
        assert_eq!(year.hit, "2025");
        assert!(year.tail.starts_with("最终版"));
    }

    #[test]
    fn a_large_index_answers_within_a_frame() {
        if cfg!(debug_assertions) {
            return;
        }
        let mut index = Index::default();
        let drive = index.push(u32::MAX, "D:", true);
        for i in 0..250_000 {
            if i % 80 == 0 {
                index.push(drive, &format!("方案-{i}.docx"), false);
            } else if i == 40_001 {
                index.push(drive, "gongyixunqin_RHYKM.zip", false);
            } else {
                index.push(drive, &format!("notes-{i}.txt"), false);
            }
        }
        for needle in ["方案", "Gong", "notes", "e"] {
            let start = std::time::Instant::now();
            let hits = query(&index, needle, 80, "", &[]);
            let elapsed = start.elapsed();
            eprintln!("{needle}: {} hits in {elapsed:?}", hits.len());
            assert!(!hits.is_empty(), "{needle}");
            assert!(
                elapsed < std::time::Duration::from_millis(8),
                "{needle} took {elapsed:?}"
            );
        }
    }
}
