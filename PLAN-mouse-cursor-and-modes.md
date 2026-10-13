# Plan: 卡住的鼠标形状（#6947）与残留的终端模式（#6880）

## Acceptance criteria

### #6947 点击链接后鼠标停在"手形"

- [ ] Given 鼠标停在超链接上（Alacritty 显示手形），when 点击链接把焦点交给
      浏览器、再切回 Alacritty 且鼠标没有移动，then 光标形状立刻按当前鼠标位置
      重新计算并生效，不再停留在点击瞬间的手形。
      Failure behavior: 切回来后仍是手形，必须移动鼠标或敲一下 Shift 键才恢复。
      Evidence: 见任务 3，需在真机 GUI 上确认（本环境无法合成点击）。
- [ ] Given 点击时鼠标按键在焦点被抢走前处于按下状态，when 窗口失焦，
      then Alacritty 清掉三个按键的按下状态，避免"一直按着左键"的假状态。
      Failure behavior: 切回窗口后 `left_button_state` 仍是 `Pressed`，
      移动鼠标会被当成拖拽。
      Evidence: 任务 3 `[source]` 代码路径 + 真机检查。
- [ ] 现有 `alacritty` 单元测试全部通过（无副作用回归）。

### #6880 程序异常退出后终端模式残留

- [x] Given 应用同时开启 1000 + 1002 + 1003（tmux `set -g mouse on` 就是这样），
      when 解析这些序列，then 三个模式**同时**保持开启（1002 扩展 1000，
      1003 扩展 1002），而不是"最后一个覆盖前面"。
      Failure behavior: `1002h` 把 `1000h` 设的点击上报清掉（当前行为，已测出）。
      Evidence: 回归测试 `mouse_tracking_modes_are_cumulative` — 旧逻辑下
      失败于 `assertion failed: term.mode.contains(TermMode::MOUSE_REPORT_CLICK)`
      （`cargo test -p alacritty_terminal --lib mouse_tracking_modes_are_cumulative`），
      改后通过 `[measured]`。
- [x] Given 应用开了 1002 + 1003，when 执行 1002l，then 1003 保持开启（只清自己那一位）。
      Failure behavior: 1002l 把 1003 也清了。
      Evidence: 同一个回归测试 `[measured]`。
- [x] Given 终端处于残留状态（鼠标上报 + 备用屏幕 + 括号粘贴 + 插入模式…），
      when 触发"软复位"，then 所有模式回到初始值、备用屏幕退出，
      但**屏幕内容和回滚缓冲不清空**。
      Failure behavior: 要么模式没清，要么像 `ESC c` 一样把缓冲区擦掉。
      Evidence: `soft_reset_restores_modes_and_keeps_history` 断言了
      `history_size` 不变、可见内容仍含 "line one"/"line two" `[measured]`。
- [x] Given 用户想把软复位绑到按键，when 在 `key_bindings` 里写 `"ResetState"`，
      then 配置能被解析、按键能触发软复位。
      Failure behavior: 配置报未知 action，或按下没反应。
      Evidence: `reset_state_action_is_configurable` 解析 TOML 后断言
      `Action::ResetState` 在绑定列表里 `[measured]`。
- [x] 现有 `alacritty_terminal` 全部 lib 测试通过。
      Evidence: `cargo test -p alacritty_terminal --lib` → 135 passed, 0 failed `[measured]`。
- [x] 现有 `alacritty` 测试全部通过。
      Evidence: `cargo test -p alacritty` → 91 passed, 0 failed `[measured]`。

## Tasks

1. 回归测试先行（#6880 协议部分）。
   Done = 新测试在旧逻辑上失败。
   Could fail at: 断言写错、把"互斥"当成了预期行为。
   Status: DONE `[measured]` — 探针输出 `after tmux setup: click=false drag=true`，
   证明旧逻辑下 `1002h` 清掉了 `1000h`；正式测试在旧逻辑上失败、新逻辑上通过。

2. `alacritty_terminal`：鼠标上报模式改为累积式；新增 `Term::soft_reset()`。
   Done = 新测试通过 + 该 crate 全量测试通过。
   Could fail at: `mouse_report` 的判断依赖 `MOUSE_MODE` 掩码，改动后上报行为变化；
   与 `swap_alt` 里的键盘模式栈交换互相影响。
   Status: DONE `[measured]` — 见上方验收清单。

3. `alacritty`：焦点回到窗口时强制刷新光标形状 + 清掉卡住的按键状态。
   Done = 代码路径可追溯，真机点一次链接确认。
   Could fail at: winit 与 AppKit 都有"形状没变就跳过"的缓存，必须绕过。
   Status: 代码 DONE `[source]`；真机 GUI 确认 PENDING（本机 osascript 驱动点击
   超时、无 GUI 自动化权限，无法合成 Cmd+Click）。

4. `ResetState` 按键动作 + 文档（manpage / features.md / CHANGELOG）。
   Done = 配置能解析并触发。
   Could fail at: `Action` 枚举加了变体后有 `match` 没覆盖。
   Status: DONE `[measured]` — 见上方验收清单。

5. 构建 release + 打包安装到 `/Applications/Alacritty.app`。
   Done = `--version` 报告新的本地 commit。
   Could fail at: 编译失败、替换时 Alacritty 正在运行。
   Status: 进行中 — `cargo build --release` rc=0、`make app` rc=0。

## 说明：为什么 #6880 不能做到"全自动"

`alacritty_terminal/src/tty/unix.rs` 只在 **pty 的直接子进程**（也就是登录 shell 本身）
退出时才会发 `ChildEvent::Exited`。`ssh` 是 shell 的子进程，它死掉时 Alacritty
收不到任何事件——屏幕不会重绘、鼠标位置不会变、也没有退出通知 `[source]`。
所以"程序异常退出后自动复位模式"在终端侧无法实现，任何基于猜测的自动复位都会
误伤 `vim` / `less` / `git` 这类会切换前台进程组的正常程序。

能提供的是**一条按键就能复位**的通道（`ResetState`），替掉 `sshcls` 这类
shell 包装函数。已知边界，见验收清单。