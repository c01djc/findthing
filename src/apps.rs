use std::sync::OnceLock;

use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSIDFromString, CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Shell::Common::{ITEMIDLIST, STRRET};
use windows::Win32::UI::Shell::{
    IEnumIDList, IShellFolder, SHGetDesktopFolder, SHGetKnownFolderPath, SHParseDisplayName,
    StrRetToBufW, KF_FLAG_DEFAULT, SHCONTF_FOLDERS, SHCONTF_NONFOLDERS, SHGDN_FORPARSING,
    SHGDN_NORMAL,
};

use crate::hit::{Hit, Kind};
use crate::search::score_title;

struct App {
    title: String,
    target: String,
}

static APPS: OnceLock<Vec<App>> = OnceLock::new();

pub fn warm() {
    std::thread::Builder::new()
        .name("findthing-apps".into())
        .spawn(|| {
            let _ = APPS.get_or_init(load);
        })
        .ok();
}

pub fn matching(query: &str) -> Vec<(i32, Hit)> {
    let Some(apps) = APPS.get() else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for app in apps {
        let score = score_title(&app.title, query);
        if score <= 0 {
            continue;
        }
        found.push((
            score,
            Hit {
                kind: Kind::File,
                title: app.title.clone(),
                detail: "应用".to_string(),
                target: app.target.clone(),
            },
        ));
    }
    found
}

fn load() -> Vec<App> {
    unsafe { collect().unwrap_or_default() }
}

unsafe fn collect() -> windows::core::Result<Vec<App>> {
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    let mut root = std::ptr::null_mut();
    let display: Vec<u16> = "shell:AppsFolder".encode_utf16().chain(Some(0)).collect();
    SHParseDisplayName(PCWSTR(display.as_ptr()), None, &mut root, 0, None)?;
    if root.is_null() {
        return Ok(Vec::new());
    }
    let desktop = SHGetDesktopFolder()?;
    let folder: IShellFolder = desktop.BindToObject(root, None)?;
    CoTaskMemFree(Some(root as *const _));
    let mut enumerator: Option<IEnumIDList> = None;
    let flags = (SHCONTF_FOLDERS.0 | SHCONTF_NONFOLDERS.0) as u32;
    if folder
        .EnumObjects(HWND(std::ptr::null_mut()), flags, &mut enumerator)
        .is_err()
    {
        return Ok(Vec::new());
    }
    let Some(enumerator) = enumerator else {
        return Ok(Vec::new());
    };
    let mut apps = Vec::new();
    loop {
        let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
        let mut fetched = 0u32;
        let more = enumerator.Next(std::slice::from_mut(&mut child), Some(&mut fetched));
        if more.is_err() || fetched == 0 || child.is_null() {
            break;
        }
        if let Some(app) = read_app(&folder, child) {
            if !apps
                .iter()
                .any(|saved: &App| saved.title == app.title && saved.target == app.target)
            {
                apps.push(app);
            }
        }
        CoTaskMemFree(Some(child as *const _));
    }
    Ok(apps)
}

unsafe fn read_app(folder: &IShellFolder, child: *const ITEMIDLIST) -> Option<App> {
    let title = folder_name(folder, child, SHGDN_NORMAL)?;
    let parsing = folder_name(folder, child, SHGDN_FORPARSING)?;
    let target = launch_target(&parsing)?;
    Some(App { title, target })
}

fn launch_target(parsing: &str) -> Option<String> {
    let parsing = parsing.trim();
    if parsing.is_empty() || parsing.contains("://") {
        return None;
    }
    if let Some(path) = expand_known_folder(parsing) {
        return Some(path);
    }
    let bytes = parsing.as_bytes();
    if bytes.len() >= 3 && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/') {
        return Some(parsing.to_string());
    }
    if parsing.starts_with("\\\\") {
        return Some(parsing.to_string());
    }
    if parsing.to_ascii_lowercase().starts_with("shell:") {
        return Some(parsing.to_string());
    }
    Some(format!("shell:AppsFolder\\{parsing}"))
}

fn expand_known_folder(parsing: &str) -> Option<String> {
    let rest = parsing.strip_prefix('{')?;
    let (guid_text, tail) = rest.split_once('}')?;
    let wide: Vec<u16> = format!("{{{guid_text}}}").encode_utf16().chain(Some(0)).collect();
    let guid = unsafe { CLSIDFromString(PCWSTR(wide.as_ptr())).ok()? };
    let folder = unsafe { SHGetKnownFolderPath(&guid, KF_FLAG_DEFAULT, None).ok()? };
    let root = unsafe { folder.to_string().ok() };
    unsafe { CoTaskMemFree(Some(folder.0 as *const _)) };
    let root = root?;
    let tail = tail.trim_start_matches(['\\', '/']);
    if tail.is_empty() {
        return Some(root);
    }
    Some(format!("{root}\\{tail}"))
}

unsafe fn folder_name(
    folder: &IShellFolder,
    child: *const ITEMIDLIST,
    flags: windows::Win32::UI::Shell::SHGDNF,
) -> Option<String> {
    let mut packed = STRRET::default();
    folder.GetDisplayNameOf(child, flags, &mut packed).ok()?;
    let mut buffer = [0u16; 1024];
    StrRetToBufW(&mut packed, Some(child), &mut buffer).ok()?;
    let end = buffer.iter().position(|unit| *unit == 0).unwrap_or(buffer.len());
    let name = String::from_utf16_lossy(&buffer[..end]);
    let name = name.trim().to_string();
    if name.is_empty() { None } else { Some(name) }
}
