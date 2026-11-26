# Windows 文本上下文边界

- 进程级服务在 `app.rs` 启动和退出，后台任务由 Tauri 应用状态持有并统一取消。关闭面板、最小化和切换焦点不停止 agent presence。
- 用户关闭文本上下文开关时立即停止观察并上报 `agent=false`；重新开启时按最近确认的串流状态恢复，退出后的迟到事件不能重新启动服务。
- 文本观察器仅在视频串流活动期间运行；暂停、结束或确认 Core 断开时停止。新 Core 用 `sessions-v1` 排除纯控制会话，旧 Core 保留 streaming 状态兼容。
- `text_context/agent.rs` 持有观察器和唯一的网络任务，先提交 capability，再发送同一代观察结果；`agent_id/generation` 隔离重启、迟到回调和旧观察结果。COM 调用不占用生命周期状态锁；停止时撤销代次并发出取消，不等待提供方返回。

- UIA 在已有焦点、启用、非离屏及有效矩形检查通过后判定：明确 Value.IsReadOnly 拒绝优先；Edit 控件按 IsReadOnly 或活动光标确认；TextEditPattern 配合活动光标确认，Document 控件额外要求光标矩形；Chromium 等提供方把 contenteditable 暴露为 Custom 或 Group 控件时，TextEditPattern 即提供方的可编辑契约。读取的属性与模式、判定顺序和各应用的实测状态见 [text_context_uia_support.md](text_context_uia_support.md)。
- 原生通道补充 UIA 无文本模式的输入面：GUITHREADINFO 焦点/光标窗口类命中清单（Win32 Edit/RichEdit、WinForms Edit、Scintilla、mintty、conhost、Windows Terminal、Chromium 渲染窗口、Qt）且存在光标时确认；PuTTY、gVim、Sublime 与 Xshell 终端区这类自绘窗口本身就是打字面，仅按焦点类名确认。UIA 读取后复查焦点窗口，原生证据与当前焦点不一致时丢弃，等待下一次重扫。
- 只读与密码：UIA Value.IsReadOnly 优先，UIA 无值时原生 ES_READONLY 视为只读；原生回退对单行 Edit 读取 ES_PASSWORD，密码框按密码上报，不再固定为普通文本。
- TextEditPattern 可以存在于只读 Chrome Document，不作为可编辑证明；缺失/失败按未知处理，不读取输入内容。
- JetBrains（Swing 无 UIA 文本、无 Win32 光标）、Alacritty/WezTerm/ConEmu（类名未验证）、GTK/Avalonia/Flutter 自绘控件与 Office 家族仍可能漏报。类名清单是显式保守清单，不做进程名单或启发式扫描。
- InputPane 通道仍保留，不能据此宣称旧客户端不再误弹。应配合 Moonlight V+ #595 的手动键盘策略。
- 合并源码不改变已安装程序，需正常集成和发布；主机缓存、焦点身份与 InputPane 各布局仍不具备全覆盖保证。

回归测试位于 `text_context.rs` 和 `text_context/agent.rs`，覆盖可编辑边界、原生类名与自绘终端、只读/密码边界、传输顺序、失败重试及观察器换代。历史 Chrome 样本用于回归，不代表全应用和真实串流已完成验收。
