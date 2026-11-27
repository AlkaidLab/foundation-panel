# 文本编辑检测的 UIA 支持

远程文本上下文让客户端在主机焦点进入输入面时自动弹出键盘。面板侧的观察器读取 Windows UI Automation（UIA）和 Win32 焦点状态，判断当前焦点是否落在可编辑控件上，Core 再把这个结论和客户端的触摸关联。本文列出观察器读取的 UIA 属性与模式、判定规则、各应用和框架的覆盖情况以及已知边界。进程生命周期、传输和开关行为见 [text_context_boundaries.md](text_context_boundaries.md)。

实现在 `src-tauri/src/text_context.rs`，Core 侧的关联在 Sunshine 仓库的 `src/text_context/`。

## 覆盖一览

| 状态 | 含义 |
| --- | --- |
| 实测通过 | 本机焦点日志确认该元素被判为可编辑 |
| 实测元素 | 本机 UIA 扫描确认了元素类型和模式，未经过观察器聚焦验证 |
| 按规则覆盖 | 只由判定规则推出，未在真机验证 |
| 未验证 | 类名或 UIA 行为尚未采样 |
| 缺口 | 已知不会触发 |

| 应用 / 框架 | 焦点元素 | 命中规则 | 状态 |
| --- | --- | --- | --- |
| Chrome 地址栏 | Edit，`OmniboxViewViews`，Value.IsReadOnly=false | Edit + IsReadOnly | 实测通过 |
| Chromium / Electron 页面内带 textbox 角色的输入框 | Edit | Edit + IsReadOnly | 实测通过 |
| Chromium / Electron 页面内无 textbox 角色的 contenteditable | Group 或 Custom，有 TextEditPattern，无 ValuePattern | Group/Custom + TextEditPattern | 实测通过（QQ、飞书输入框） |
| Chromium 页面根 | Document，Value.IsReadOnly=true | 只读拒绝 | 实测不触发 |
| QQ 搜索框 | Edit | Edit + IsReadOnly | 实测元素 |
| Notepad++（Scintilla） | Pane，`Scintilla`，Win32 光标 | Scintilla + 光标 | 实测通过 |
| 微信 PC（Qt 5.15） | Window，`Qt51514QWindowIcon`，Win32 光标 | 原生类清单 + 光标 | 实测通过，矩形为整个窗口 |
| mintty | Window，焦点类 `mintty`，Win32 光标 | 原生类清单 + 光标 | 实测通过 |
| Win11 记事本 | 焦点子窗口 `RichEditD2DPT`，Win32 光标 | Edit + IsReadOnly | 实测光标读取，判定按规则 |
| Win32 Edit / RichEdit（经典对话框、WordPad） | Edit | Edit + IsReadOnly | 按规则覆盖 |
| WinForms TextBox | `WindowsForms10.EDIT.*` | Edit + IsReadOnly | 按规则覆盖 |
| WPF TextBox | Edit，框架自带 UIA | Edit + IsReadOnly | 按规则覆盖 |
| WinUI / UWP（系统设置等） | Edit | Edit + IsReadOnly | 按规则覆盖 |
| 其他 Qt 应用（Telegram Desktop 等） | `Qt*QWindowIcon` | 原生类清单 + 光标 | 按规则覆盖 |
| conhost | `ConsoleWindowClass` | 原生类清单 + 光标 | 未验证 |
| Windows Terminal | `CASCADIA_HOSTING_WINDOW_CLASS` | 原生类清单 + 光标 | 未验证 |
| Xshell 终端区 | `Xshell*::CoreApp` | 自绘类名 | 类名实测，判定按规则 |
| PuTTY、gVim、Sublime Text | `PuTTY`、`Vim`、`PX_WINDOW_CLASS` | 自绘类名 | 类名来自公开资料，未验证 |
| Chromium 渲染窗口的原生通道 | `Chrome_RenderWidgetHostHWND` | 原生类清单 + 光标 | 实测无 Win32 光标，不触发。Chromium 由上面的 UIA 规则覆盖 |
| ARIA combobox（Chromium） | ComboBox，无光标证据 | 无 | 缺口 |
| JetBrains 系列（Java Swing） | 无 UIA 文本模式，无 Win32 光标 | 无 | 缺口 |
| Alacritty、WezTerm、ConEmu、MobaXterm | 类名未采样 | 无 | 未验证 |
| GTK、Avalonia、Flutter | 自绘控件 | 无 | 未验证 |
| Office、WPS | 有 TextPattern，未采样 | 待定 | 未验证 |

## 读取的 UIA 属性与模式

| 属性 / 模式 | 用途 |
| --- | --- |
| `UIA_ControlTypePropertyId` | 控件类型：Edit、Document、Group、Custom 等 |
| `UIA_ClassNamePropertyId` | 识别 Scintilla。Chromium 网页元素的类名是 DOM `class` |
| `UIA_HasKeyboardFocusPropertyId` | 前置过滤：元素须持有键盘焦点 |
| `UIA_IsEnabledPropertyId` | 前置过滤：禁用控件不是输入面 |
| `UIA_IsOffscreenPropertyId` | 前置过滤 |
| `UIA_BoundingRectanglePropertyId` | 元素矩形，上报给 Core 做点击关联 |
| `UIA_IsPasswordPropertyId` | 密码框上报 |
| `UIA_ProcessIdPropertyId` | 只用于日志中的进程名 |
| `ValuePattern` 的 `UIA_ValueIsReadOnlyPropertyId` | 明确的只读结论，优先级最高 |
| `TextEditPattern` | 提供方声明的可编辑契约 |
| `TextPattern2` 的 `GetCaretRange` | 光标是否活动及其矩形 |

观察器不读取输入内容。它不调用 `TextPattern` 的选区和文本范围接口，也不使用 MSAA / IAccessible2、`LegacyIAccessiblePattern` 或 TSF。

## 判定规则

每次快照依次执行，命中即停止。

1. **前置过滤**。元素须 `HasKeyboardFocus`、`IsEnabled`、非 `IsOffscreen`，且矩形宽高都大于 0，否则按无焦点处理。
2. **只读拒绝**。`Value.IsReadOnly == true` 时判为不可编辑。UIA 没有只读值而原生 Edit 窗口带 `ES_READONLY` 时同样拒绝。
3. **Group / Custom + TextEditPattern**。Chromium 把没有 textbox 角色的 contenteditable 暴露为 Group 或 Custom，没有 ValuePattern 也没有光标，TextEditPattern 是它唯一的可编辑声明。
4. **Scintilla + 活动光标**。
5. **原生窗口类**。焦点窗口类命中自绘清单，或焦点、光标窗口类命中原生文本清单且存在 Win32 光标矩形。
6. **Edit 控件**。`Value.IsReadOnly == false` 或光标活动。
7. **TextEditPattern + 活动光标**。Document 控件额外要求光标有矩形，避免浏览器光标浏览被当成编辑。
8. 以上都不满足时判为不可编辑。不可编辑的结论同样上报，Core 据此撤销之前的激活。

密码标记独立于可编辑结论。UIA 的 `IsPassword` 为 true，或焦点窗口是带 `ES_PASSWORD` 的单行 Edit 时，观察结果按密码上报。

光标优先取 `TextPattern2`，取不到时读 Win32 `GUITHREADINFO` 的光标。UIA 读取完成后会用 `GetGUIThreadInfo` 复查焦点窗口，原生证据与当前焦点不一致时丢弃。

Scintilla 和 Qt 编辑器只靠 Win32 光标确认，而光标每次闪烁会消失约 530 ms。同一个元素在带光标确认后的 1 秒内失去光标按闪烁处理，观察器保持原结论，不发布撤销。

## 原生窗口类清单

类名不区分大小写，是显式清单，不做进程名单或启发式扫描。

| 类名 | 额外要求 |
| --- | --- |
| `edit`、`richedit*`、`windowsforms10.edit*` | Win32 光标 |
| `scintilla` | Win32 光标 |
| `mintty` | Win32 光标 |
| `consolewindowclass`、`cascadia_hosting_window_class` | Win32 光标 |
| `chrome_renderwidgethosthwnd*` | Win32 光标 |
| `qt*` 且以 `qwindowicon` 结尾 | Win32 光标 |
| `putty`、`vim`、`px_window_class`、`xshell*::coreapp` | 无。这些窗口本身就是打字面，只匹配焦点窗口类名 |

只有 `edit`、`richedit*`、`windowsforms10.edit*` 读取 `ES_READONLY` 和 `ES_PASSWORD`，终端和自绘控件不响应这两个样式。

## 与 Core 的关联

观察器上报 `editable`、`password`、`element_rect` 和 `caret_rect`。Core 只在以下条件同时满足时向客户端发送激活。

- 客户端的一次轻点已经抬起，按下到抬起的位移不超过 24 px。
- 轻点后 1200 ms 内收到可编辑的焦点结论，或抬起时最近一次焦点快照已是可编辑。
- 点击坐标落在 `element_rect` 内，容差 12 px。

由此带来几个可以预期的现象。

- Chrome 地址栏的矩形覆盖整个工具栏宽度，点在地址栏下沿也算命中。
- 焦点进入了输入框但点击坐标不在它的矩形内时不会激活。
- Qt 应用的焦点元素是顶层窗口，矩形覆盖整个窗口。微信输入框聚焦时，窗口内任意位置的一次轻点都会关联到它。

## 诊断

面板日志在 `%LOCALAPPDATA%\Sunshine GUI\logs\panel.log`。每次焦点变化写一行，例如：

```text
text context focus: source=uia active=true editable=true password=false control_type=50026 caret=false process=QQ.exe class=ProseMirror ExEditor-qq-msg-editor is-empty ProseMirror-focused rect=1280,870-2714,1045 text_edit=true value_read_only=n/a
```

| 字段 | 含义 |
| --- | --- |
| `editable` | 观察器给出的结论 |
| `control_type` | UIA 控件类型。常见值：50004 Edit、50026 Group、50025 Custom、50030 Document、50032 Window、50033 Pane |
| `caret` | 是否取得了光标矩形 |
| `process` | 焦点元素所属进程的可执行文件名 |
| `class` | UIA 类名。原生回退时是焦点窗口类名 |
| `rect` | 元素矩形，与 Core 日志 `Remote text context touch input` 的坐标同一坐标系 |
| `text_edit` | 元素是否提供 TextEditPattern |
| `value_read_only` | Value.IsReadOnly。`n/a` 表示没有 ValuePattern |

排查一次误弹或漏弹时，取 Core 日志里该次触摸的坐标，对照同一时刻焦点行的 `process`、`rect` 和 `editable`。

探针 `text-context-probe` 输出当前焦点窗口和 UIA 焦点元素的类名、控件类型、模式和光标矩形，不注入也不修改被检视的进程：

```text
cargo run --features text-context-probe --bin text-context-probe [--once] [--interval-ms N]
```

要给一个新的应用补支持，先用探针确认它的焦点类名、Win32 光标和 UIA 模式，再选择对应的规则、补充测试，并在上面的覆盖表登记实测状态。
