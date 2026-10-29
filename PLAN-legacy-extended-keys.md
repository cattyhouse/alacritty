# Plan: 未协商协议时保住 Shift/Ctrl 组合键（Legacy 消歧义）

## 背景与目标

在 tmux 里运行的 TUI 应用（例如 pi）拿不到 `Shift+Enter`、`Ctrl+Shift+Tab`、`Ctrl+Shift+数字` 之类的组合键：
传统编码把这些组合压成了和无修饰键完全相同的字节（`Shift+Enter` 就是裸 `\r`）。
应用要区分它们必须协商 kitty 键盘协议，而 **tmux 不会把应用的协商请求转给外层终端**，
外层 alacritty 于是停在传统（Legacy）模式。

目标：即使没有任何协商，也让这些组合键的修饰信息到达应用，同时**不改变**正常打字的字节流。

## 设计决策

1. **只兜底"传统编码必然丢信息"的键**，其余一字节不动。
   - 兜底集合：`Enter`（Shift/Ctrl 任一）、`Tab`（Ctrl）、`Escape`（Shift/Ctrl）、`Space`（Ctrl+Shift）、
     `Backspace`（Ctrl+Shift）、单码点 ASCII 字符键（Ctrl+Shift）。
   - 明确不兜底：单独 Shift（大写、上档符号）、`Shift+Space`、单独 Ctrl（`Ctrl+C` 仍是 `0x03`）、
     单独 Alt（仍是 ESC 前缀）、`Alt+Enter`（仍是 `ESC \r`）、`Shift+Tab`（仍是 `\x1b[Z`）、
     `Ctrl+Space`（仍是 `0x00`）。
2. **按键的类型选序列形状**，而不是一律用 kitty：
   - 命名键（Enter/Tab/Escape/Space/Backspace）用 xterm 的 `modifyOtherKeys` 形式
     `CSI 27;<mods>;<code>~`。这是 xterm 2005 年就有的形式，ghostty 在未协商时对同一批键用的就是它，
     旧式解析器认识的概率更高。
   - 字符键用 kitty 的 `CSI <codepoint>;<mods>u`。字母按小写基底 + Shift 位（`Ctrl+Shift+F` → `CSI 102;6u`），
     数字/符号按未上档的基底键（`Ctrl+Shift+1` → `CSI 49;6u`）。
3. **"是否兜底"和"兜底发什么"是同一个函数**（`legacy_sequence`）。
   `should_build_sequence` 只问它 `is_some()`，编码器直接用它，因此不存在
   "闸门放行、编码器却吐空"导致按键被吞的情况。
4. **字符键兜底限定单码点 ASCII**。IME 组合文本（多码点）和非 ASCII 字符继续走文本路径，
   不会被丢成空字节，也不会发出越界码点。
5. **最终字节可测试**：渲染逻辑抽成 `render_sequence`，兜底决策抽成 `legacy_sequence`，
   单元测试直接断言写到 pty 的字节。

## 与 ghostty 的差异（对比结论）

| 键 | 本实现 | ghostty（未协商协议） |
|---|---|---|
| Shift+Enter | `CSI 27;2;13~` | `CSI 27;2;13~`（相同） |
| Ctrl+Shift+字母 | `CSI 102;6u` | `CSI 102;6u`（相同） |
| Ctrl+Shift+数字/符号 | `CSI 49;6u`（基底键 + Shift） | `CSI 64;5u` 之类（上档字符 + 丢 Shift，fixterms 折中） |
| Shift+Escape | `CSI 27;2;27~` | `CSI 27;2;27~`（相同） |
| Ctrl+Shift+空格 | `CSI 27;6;32~` | `0x00`（ghostty 先走 Ctrl 映射，Shift 丢失） |
| Ctrl+Shift+Backspace | `CSI 27;6;127~` | `0x08`（Shift 丢失） |

结论：命名键与 ghostty 一致；字符键在"数字/符号"上比 ghostty 更保真（本实现的接收方看到的是
`Ctrl+Shift+1`，ghostty 的接收方看到的是 `Ctrl+@` 一类）；空格与退格比 ghostty 多覆盖两个组合。

## 验收标准（全部为已实测）

| 编号 | 场景 | 期望 | 证据 |
|---|---|---|---|
| AC1 | Legacy 下 `Shift+Enter` | 发出 `\x1b[27;2;13~`，经 tmux 到应用变成 `\x1b[13;2u`（pi 判定为 shift+enter） | 单测 `legacy_enter_keeps_modifiers`；tmux 探针 `z_enter_x`；**真机实测**：2026-10-29 在 tmux 里运行 pi，搜索界面按 `Shift+Enter` 触发 Previous（不再等同 Enter 的 Next） |
| AC2 | `Ctrl+Enter` / `Ctrl+Shift+Enter` / `Alt+Shift+Enter` | `\x1b[27;5;13~` / `\x1b[27;6;13~` / `\x1b[27;4;13~`，经 tmux 变 `\x1b[13;5u` / `\x1b[13;6u` / `\x1b[13;4u` | 单测 + 探针 `z_cent_x`/`z_csent_x`/`z_asent_x` |
| AC3 | `Ctrl+Tab` / `Ctrl+Shift+Tab` | `\x1b[27;5;9~` / `\x1b[27;6;9~` | 单测 + 探针 `z_ctab_x`（tmux 会把 `Ctrl+Shift+Tab` 归一成 `\x1b[Z`，属 tmux 自身行为） |
| AC4 | `Shift+Esc` | `\x1b[27;2;27~` | 单测 + 探针 `z_esc_x` |
| AC5 | `Ctrl+Shift+Space` | `\x1b[27;6;32~` | 单测 + 探针 `z_csspace` |
| AC6 | `Ctrl+Shift+Backspace` | `\x1b[27;6;127~` | 单测 + 探针 `z_csbs` |
| AC7 | `Ctrl+Shift+F` / `Ctrl+Shift+1` | `\x1b[102;6u` / `\x1b[49;6u` | 单测 + 探针 `z_csf`/`z_cs1` |
| AC8 | 正常打字不变 | `Enter`→`\r`、`Shift+F`→`F`、`Space`→` `、`Ctrl+F`→`0x06`（tmux 自己会转成 `\x1b[102;5u`）、`Ctrl+Space`→`0x00`、`Shift+Tab`→`\x1b[Z` | 单测；探针 `z_enter_p`/`z_shiftF`/`z_space`/`z_cf_p` |
| AC9 | 闸门与编码器一致 | 任意键×修饰组合下 `should_build_sequence()` 与 `legacy_sequence().is_some()` 相等 | 单测 `legacy_gate_matches_encoder` |
| AC10 | 不吞文本 | 多码点（IME）与非 ASCII 的 `Ctrl+Shift` 字符不发序列、不丢字节 | 单测 `legacy_character_fallback_never_swallows_text` |
| AC11 | 协商后行为不变 | 协商了 `DISAMBIGUATE_ESC_CODES` 时仍走原有逻辑 | 单测 `negotiated_protocol_behavior_is_unchanged` |

回归防线：把 `Enter` 分支改坏后，`legacy_enter_keeps_modifiers` 与
`ctrl_shift_letter_disambiguated_without_protocol` 立即失败（已实测）。

## tmux 实测方法（可复现）

在 pty 里跑一个 tmux 客户端（用本机真实配置 `~/.tmux.conf`：`extended-keys on`、
`extended-keys-format csi-u`），pane 里跑一个先发 `CSI >4;2m`（pi 实际使用的降级请求）
再裸读 stdin 的小程序，然后往 pty 主端写入"alacritty 会发出的字节"，看应用收到什么：

| 外层写入 | 应用收到 |
|---|---|
| `\x1b[27;2;13~` | `\x1b[13;2u` |
| `\x1b[27;5;13~` | `\x1b[13;5u` |
| `\x1b[27;6;13~` | `\x1b[13;6u` |
| `\x1b[27;4;13~` | `\x1b[13;4u` |
| `\x1b[27;5;9~` | `\x1b[9;5u` |
| `\x1b[27;2;27~` | `\x1b[27;2u` |
| `\x1b[27;6;32~` | `\x1b[32;6u` |
| `\x1b[27;6;127~` | `\x1b[127;6u` |
| `\x1b[102;6u` | `\x1b[102;6u` |
| `\x1b[49;6u` | `\x1b[49;6u` |

另测得：应用只用 kitty 的 `CSI >1u` 请求时，tmux 3.8 不认（仍按裸 `\r` 转发），
这解释了为什么必须由外层终端自己兜底。

## 未验证项

- 真键盘事件：末段已在真实环境确认（见 AC1）—— 在 tmux 里运行 pi，按 `Shift+Enter` 从 Next 变为 Previous，问题解决。
  自动化只能覆盖到"alacritty 会发出的字节"与"这些字节经 tmux 到达应用"两段，物理按键→字节这一段依赖人工确认。
- 其余组合（`Ctrl+Shift+Tab`、`Shift+Esc`、`Ctrl+Shift+Space`、`Ctrl+Shift+Backspace`、`Ctrl+Shift+数字/符号`）
  只验证到字节与 tmux 链路，未逐个在真实窗口按过。
- Windows / Wayland 等非 macOS 平台未实测。
