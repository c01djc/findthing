use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::process::Command;

use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, HANDLE, LPARAM, WPARAM};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenMutexW, OpenProcessToken, SYNCHRONIZATION_ACCESS_RIGHTS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    PostMessageW, RegisterWindowMessageW, HWND_BROADCAST,
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const TASK_NAME: &str = "Findthing";
const _: () = assert!(std::mem::size_of::<ShellExecInfo>() == 112);

#[repr(C)]
struct ShellExecInfo {
    cb_size: u32,
    mask: u32,
    hwnd: isize,
    verb: *const u16,
    file: *const u16,
    params: *const u16,
    dir: *const u16,
    show: i32,
    inst: isize,
    id_list: *mut std::ffi::c_void,
    class: *const u16,
    key: isize,
    hotkey: u32,
    icon: isize,
    process: isize,
}

#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteExW(info: *mut ShellExecInfo) -> i32;
}

/// Starts one elevated copy when this process cannot read the NTFS file table.
/// Returns true when this process should exit and let the other copy run.
pub fn hand_off_for_fast_index() -> bool {
    if instance_is_running() {
        summon_running();
        return true;
    }
    if is_elevated() {
        install_task();
        return false;
    }
    if task_exists() && run_task() {
        return true;
    }
    relaunch_elevated()
}

pub fn is_elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        );
        let _ = CloseHandle(token);
        ok.is_ok() && elevation.TokenIsElevated != 0
    }
}

fn instance_is_running() -> bool {
    unsafe {
        match OpenMutexW(
            SYNCHRONIZATION_ACCESS_RIGHTS(0x0010_0000),
            false,
            w!("Local\\Findthing.Single"),
        ) {
            Ok(handle) => {
                let _ = CloseHandle(handle);
                true
            }
            Err(err) => win32_code(&err) == 5,
        }
    }
}

fn summon_running() {
    let message = unsafe { RegisterWindowMessageW(w!("Findthing.Show")) };
    if message == 0 {
        return;
    }
    for _ in 0..8 {
        unsafe {
            let _ = PostMessageW(Some(HWND_BROADCAST), message, WPARAM(0), LPARAM(0));
        }
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
}

fn relaunch_elevated() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let file: Vec<u16> = exe.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let verb: Vec<u16> = "runas\0".encode_utf16().collect();
    let mut info = ShellExecInfo {
        cb_size: std::mem::size_of::<ShellExecInfo>() as u32,
        mask: 0x140,
        hwnd: 0,
        verb: verb.as_ptr(),
        file: file.as_ptr(),
        params: std::ptr::null(),
        dir: std::ptr::null(),
        show: 1,
        inst: 0,
        id_list: std::ptr::null_mut(),
        class: std::ptr::null(),
        key: 0,
        hotkey: 0,
        icon: 0,
        process: 0,
    };
    debug_assert_eq!(std::mem::size_of::<ShellExecInfo>(), 112);
    let started = unsafe { ShellExecuteExW(&mut info) } != 0;
    if info.process != 0 {
        unsafe {
            let _ = CloseHandle(HANDLE(info.process as *mut std::ffi::c_void));
        }
    }
    started
}

fn task_exists() -> bool {
    schtasks(&["/Query", "/TN", TASK_NAME])
}

fn run_task() -> bool {
    schtasks(&["/Run", "/TN", TASK_NAME])
}

pub fn remove_task() {
    let _ = schtasks(&["/Delete", "/F", "/TN", TASK_NAME]);
}

fn install_task() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let command = xml_escape(&exe.display().to_string());
    let user = std::env::var("USERNAME").unwrap_or_default();
    let user_xml = if user.is_empty() {
        String::new()
    } else {
        format!("<UserId>{}</UserId>", xml_escape(&user))
    };
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <Principals>
    <Principal id="Author">
      {user_xml}
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{command}</Command>
      <Arguments>--elevated-task</Arguments>
    </Exec>
  </Actions>
</Task>
"#
    );
    let path = std::env::temp_dir().join("findthing-task.xml");
    let mut encoded = vec![0xFF, 0xFE];
    for unit in xml.encode_utf16() {
        encoded.extend_from_slice(&unit.to_le_bytes());
    }
    let Ok(mut file) = std::fs::File::create(&path) else {
        return;
    };
    if file.write_all(&encoded).is_err() {
        return;
    }
    drop(file);
    let spec = path.to_string_lossy().to_string();
    let _ = schtasks(&["/Create", "/F", "/TN", TASK_NAME, "/XML", &spec]);
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn schtasks(args: &[&str]) -> bool {
    Command::new("schtasks")
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn win32_code(err: &windows::core::Error) -> u32 {
    let code = err.code().0 as u32;
    if code & 0xFFFF_0000 == 0x8007_0000 {
        code & 0xFFFF
    } else {
        code
    }
}
