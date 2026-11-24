use std::path::Path;
use std::process::Command;

use crate::hit::{Hit, Kind};

struct Spec {
    id: &'static str,
    title: &'static str,
    aliases: &'static [&'static str],
    guide: &'static str,
}

const SPECS: &[Spec] = &[
    Spec {
        id: "wt",
        title: "终端",
        aliases: &["终端", "wt", "terminal"],
        guide: "打开 Windows 终端。适合日常输入命令，一个窗口里可以开多个标签。",
    },
    Spec {
        id: "cmd",
        title: "命令提示符",
        aliases: &["命令提示符", "cmd", "command"],
        guide: "打开传统命令提示符。适合运行 .bat 和较早的系统命令。",
    },
    Spec {
        id: "powershell",
        title: "PowerShell",
        aliases: &["powershell", "ps"],
        guide: "打开 PowerShell。适合跑脚本，也适合管理文件和系统。",
    },
    Spec {
        id: "env",
        title: "环境变量",
        aliases: &["环境变量", "env", "path"],
        guide: "打开环境变量窗口，用来查看或修改 Path。",
    },
    Spec {
        id: "settings",
        title: "设置",
        aliases: &["设置", "settings"],
        guide: "打开 Windows 设置。",
    },
    Spec {
        id: "recycle",
        title: "回收站",
        aliases: &["回收站", "recycle"],
        guide: "打开回收站。",
    },
    Spec {
        id: "taskmgr",
        title: "任务管理器",
        aliases: &["任务管理器", "taskmgr"],
        guide: "打开任务管理器，查看正在运行的程序。",
    },
    Spec {
        id: "notepad",
        title: "记事本",
        aliases: &["记事本", "notepad"],
        guide: "打开记事本。",
    },
    Spec {
        id: "calc",
        title: "计算器",
        aliases: &["计算器", "calc", "calculator"],
        guide: "打开计算器。",
    },
    Spec {
        id: "snip",
        title: "截图",
        aliases: &["截图", "snip", "screenshot"],
        guide: "打开截图，框选屏幕上的一块区域。",
    },
    Spec {
        id: "control",
        title: "控制面板",
        aliases: &["控制面板", "control"],
        guide: "打开控制面板。",
    },
    Spec {
        id: "devices",
        title: "设备管理器",
        aliases: &["设备管理器", "devmgmt", "devices"],
        guide: "打开设备管理器，查看和管理这台电脑的硬件。",
    },
];

const SUGGEST: &[&str] = &["wt", "cmd", "powershell", "env"];

#[allow(dead_code)]
pub fn suggestions() -> Vec<Hit> {
    SUGGEST
        .iter()
        .filter_map(|id| SPECS.iter().find(|spec| spec.id == *id))
        .map(to_hit)
        .collect()
}

pub fn matching(query: &str) -> Vec<(i32, Hit)> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    let mut found = Vec::new();
    for spec in SPECS {
        let score = spec
            .aliases
            .iter()
            .map(|alias| alias_score(alias, query))
            .max()
            .unwrap_or(0);
        let score = score.max(if spec.title.contains(query) { 820 } else { 0 });
        if score > 0 {
            found.push((score, to_hit(spec)));
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found
}

pub fn run(id: &str, cwd: Option<&Path>) -> bool {
    let dir = cwd.map(|path| {
        if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent().unwrap_or(path).to_path_buf()
        }
    });
    match id {
        "wt" => spawn_terminal(&dir),
        "cmd" => spawn_cmd(&dir),
        "powershell" => spawn_powershell(&dir),
        "env" => Command::new("rundll32")
            .arg("sysdm.cpl,EditEnvironmentVariables")
            .spawn()
            .is_ok(),
        "settings" => Command::new("explorer").arg("ms-settings:").spawn().is_ok(),
        "recycle" => Command::new("explorer")
            .arg("shell:RecycleBinFolder")
            .spawn()
            .is_ok(),
        "taskmgr" => spawn(Command::new("taskmgr")),
        "notepad" => spawn(Command::new("notepad")),
        "calc" => spawn(Command::new("calc")),
        "snip" => Command::new("explorer").arg("ms-screenclip:").spawn().is_ok(),
        "control" => spawn(Command::new("control")),
        "devices" => crate::platform::open_device_manager(),
        _ => false,
    }
}

fn to_hit(spec: &Spec) -> Hit {
    Hit {
        kind: Kind::Command,
        title: spec.title.to_string(),
        detail: spec.guide.to_string(),
        target: spec.id.to_string(),
    }
}

fn alias_score(alias: &str, query: &str) -> i32 {
    if alias.eq_ignore_ascii_case(query) || alias == query {
        2500
    } else if starts_with(alias, query) {
        1800
    } else if alias.to_ascii_lowercase().contains(&query.to_ascii_lowercase()) || alias.contains(query)
    {
        900
    } else {
        0
    }
}

fn starts_with(text: &str, prefix: &str) -> bool {
    let text = text.as_bytes();
    let prefix = prefix.as_bytes();
    text.len() >= prefix.len() && text[..prefix.len()].eq_ignore_ascii_case(prefix)
}

fn spawn(mut command: Command) -> bool {
    command.spawn().is_ok()
}

fn spawn_terminal(dir: &Option<std::path::PathBuf>) -> bool {
    let mut command = Command::new("wt");
    if let Some(dir) = dir {
        command.arg("-d").arg(dir);
    }
    if command.spawn().is_ok() {
        return true;
    }
    spawn_cmd(dir)
}

fn spawn_cmd(dir: &Option<std::path::PathBuf>) -> bool {
    let mut command = Command::new("cmd");
    if let Some(dir) = dir {
        command.arg("/K").arg(format!("cd /d \"{}\"", dir.display()));
    }
    spawn(command)
}

fn spawn_powershell(dir: &Option<std::path::PathBuf>) -> bool {
    let mut command = Command::new("powershell");
    command.arg("-NoExit");
    if let Some(dir) = dir {
        let literal = dir.display().to_string().replace('\'', "''");
        command
            .arg("-Command")
            .arg(format!("Set-Location -LiteralPath '{literal}'"));
    }
    spawn(command)
}
