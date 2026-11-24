# findthing

Windows 上的轻量文件搜索：文件名即时检索、文件夹内定位、另存/打开对话框里挂搜索条，以及常用命令引导。

## 要求

- Windows 10 / 11（x64）
- [Rust](https://rustup.rs/)（开发时）

## 运行

```bat
cargo run --release --bin findthing
```

托盘里选 Quit 干净退出。启动后会有两个 `findthing.exe`（守护进程 + `--app` 子进程）。

## 使用

- 默认热键：双击 Ctrl（可在程序里改）
- 输入文件名即时出结果
- 在「另存为 / 打开」对话框底部会出现搜索条，用来跳转目录
- 托盘可切换外观主题

## 开源协议

MIT。详见 [LICENSE](LICENSE)。
