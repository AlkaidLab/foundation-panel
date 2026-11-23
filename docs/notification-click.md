# 通知点击与全屏 PIN 输入

## 通知点击

每一条原生通知（配对请求、客户端连接/断开、虚拟显示器、其他核心通知）都能点。Windows 把通知留在通知中心，面板重启之后还能点，所以点击的答案不能放在发出通知的进程里（`Toast::on_activated` 这种进程内回调会随进程消失），而是写在通知本身：

- 通知的 `launch` 是一个本应用的地址 `sunshine-panel://notify/...`，`activationType="protocol"`（`tray/notifications/toast.rs`，来源 `panel-ms-toast-activation-schema`、`panel-ms-toast-element-toast`）。
- 面板每次启动把这个协议登记到当前用户（`HKCU\Software\Classes\sunshine-panel`，`URL Protocol` 与 `shell\open\command` 带引号的 `%1`，来源 `panel-ms-uri-scheme`），只在内容不同时才写；测试隔离实例不登记，免得把用户的点击指到测试实例。
- 点击后 Windows 用这个地址启动本程序的第二个进程。面板正在运行：单实例插件把参数交给运行中的那个（`app::handle_single_instance`）；面板没在运行：新进程读自己的启动参数（`app::setup_application`）。两条路都走 `NotifyAction::from_args`（`notify_action.rs`）。

| 地址 | 动作 |
| --- | --- |
| `sunshine-panel://notify/pin` | 全屏 PIN 输入；请求已结束时（主机状态不再是 pairing）改开主面板的「配对」页 |
| `sunshine-panel://notify/main` | 主面板 |
| `sunshine-panel://notify/page/<名字>` | 主面板的一个页面，名字必须在 `commands::PANEL_PAGES` 里 |

地址来自进程外，任何程序或网页都能触发协议处理程序，所以只认上面三种形状，不接受路径、命令或要加载的 URL。

### 没有选 COM 激活

官方推荐的非打包桌面应用做法是 AUMID + 快捷方式上的 `ToastActivatorCLSID` + 进程外 COM 服务器（`panel-ms-toast-quickstart`）。这里没有采用：面板是便携 exe，随 Foundation Sunshine 安装包分发，不保证有开始菜单快捷方式；COM 服务器还得单独注册、实现回调，而且在没有真实桌面会话的环境里无法验证。协议激活需要的只是一个注册表项和已经存在的单实例交接。

### 没有验证的部分

从通知中心点击一条已经离开屏幕的通知，必须在真实桌面会话里验证；自动化测试覆盖的是地址映射、XML 的转义和启动参数的解析。

## 全屏 PIN 输入

`pin_window.rs` 与 `src/renderer/pin/`：覆盖整个显示器、置顶、透明背景的独立窗口，显示是哪个客户端在请求，键盘、触摸、手柄（方向键或左摇杆移动，A 按下，B 取消）都能输入；Esc、取消按钮或点击背景关闭。

- 窗口不在主线程的事件处理里创建，创建时隐藏，页面加载完成后才显示（带上限），关闭走 `closing::close_window`：先藏后毁（`windows/building.rs`、`windows/closing.rs`）。
- 生命周期跟着托盘状态：主机报告 `pairing` 后又不再是 `pairing`（成功、过期、取消）、或主机不可达，页面先提示再关闭；主机没有报告过配对时，等 10 秒的宽限（`PAIRING_GRACE`）。
- 面板里的「配对」页（`open_main_panel_page(app, "pairing")`）保留。
