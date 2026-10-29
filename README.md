# Alacritty (cattyhouse fork)

基于上游 [alacritty/alacritty](https://github.com/alacritty/alacritty)，针对 **macOS / Apple Silicon** 修五个具体问题。

上游文档（安装、配置项、功能列表）见 `INSTALL.md` 与 [docs/features.md](docs/features.md)；本文只写这个 fork 相对上游多出来的东西。

需要恢复上游 README：

```sh
git show origin/master:README.md > README.md
```

---

## 修复一：大量中文时渲染卡死

### 症状

在 macOS 上打开含大量中文的内容会几乎无法动弹。本地、ssh、ssh + tmux 三种场景都会：

```sh
cat testdata/cjk_bucket.txt
```

窗口越大、内容越长越严重。

### 原因

字形贴图是一个固定 $1024\times1024$ 的贴图池。一个贴图约装 2300 个不重复汉字，中文密集的屏幕会横跨好几张贴图。

而绘制是按屏幕行列顺序遍历的，字形却按「首次出现」顺序分配进不同贴图 —— 两者顺序不一致，于是**屏幕上相邻的格子经常属于不同贴图**。每次跨越贴图边界就中断一次批次，一帧本该 1 次的绘制被拆成几十上百次小批次。

在 macOS 上 OpenGL 走 AppleMetalOpenGLRenderer 转译层，每次 `glBufferSubData` 都会强制 flush 资源、等 Metal 命令队列，主线程直接被卡住。实测一次 60 秒的连续滚动中，主线程约有 15 秒阻塞在这个等待上；在 `/tree` 那种 500 多条消息的场景里，最长一帧达到 271 ms。

### 做法

绘制前先把每个格子的字形解析出来，按所属贴图分桶，再逐桶提交。

批次打断次数从「随内容增长而暴增」收敛为「当帧用到的贴图张数」。

因为格子在屏幕上互不重叠，重排绘制顺序不会改变画面。

另外把「每个格子分配一个 `vec![glyph]`」也去掉了：主字形直接放在结构体里，组合符号留在不分配的 `Vec::new()` 里。绝大多数格子根本没有组合符号，原来那一小块堆分配纯属白付。实测连续滚动 500×140 的中文网格 22 秒，分配器采样从 51 降到 4。

### 实测

Apple M1 / macOS 27.2，`testdata/cjk_bucket.txt`（3199 个不重复汉字），500×140 网格：

| 指标 | 上游 | 本 fork |
|---|---|---|
| 纹理切换/帧 | 9.9 | **1.0** |
| 绘制批次/帧 | 10.9 | **2.0** |
| `glBufferSubData` 采样占主线程 | 卡在 Metal 队列上 | **1.6%** |

上两行取自同一台机器、同一份语料、500×140 网格的直接对比。第三行是另一组经 ssh+tmux 的采样，两个数字来自不同场景，只用于说明阻塞量级的变化，不作严格横比。

三个场景（本地 / ssh / ssh+tmux）人工对照：上游全部卡死，本 fork 全部正常。

### 改动范围

只改 `alacritty/src/renderer/text/mod.rs` 一个文件。`atlas.rs`、`gles2.rs`、`glsl3.rs`、`glyph_cache.rs` **完全未动**，贴图的尺寸、装箱策略与生命周期都还是上游那套。

### 已知局限

- 批次是 2.0 而非理论下限 1.0：跨贴图的组合符号（基础字形 + 零宽重音）无法分桶，会退回原序绘制。现实中近乎不出现。
- 斜体字形可能向右悬挑 1~2 像素进相邻格子。分桶后这个覆盖关系不再有保证，个别位置可能有 1~2 像素差异。

详见 [`PLAN-bucket-sort.md`](PLAN-bucket-sort.md)。

---

## 修复二：Shift/Ctrl 组合键在 tmux 下收不到

### 症状

在 tmux 里运行的 TUI 应用（例如 pi）收不到这些组合键：

- `Shift+Enter`（pi 搜索界面里本该是 Previous，却和 Enter 一样触发 Next）
- `Ctrl+Shift+F`、`Ctrl+Shift+数字/符号`
- `Ctrl+Tab` / `Ctrl+Shift+Tab`、`Shift+Esc`、`Ctrl+Shift+Space`、`Ctrl+Shift+Backspace`

### 原因

传统终端编码压掉了修饰键：`Shift+Enter` 与 `Enter` 都只能是裸 `\r`（`0x0D`），
`Ctrl+Shift+F` 与 `Ctrl+F` 都只能是 `0x06`。

应用要区分它们必须协商键盘协议，但 tmux 不会把 pane 里应用的协商请求转给外层终端 ——
实测应用只发 kitty 的 `CSI >1u` 时，tmux 3.8 完全不认，仍按裸 `\r` 转发；
应用于是降级发 xterm 的 `CSI >4;2m`，而 tmux 只据此决定「自己怎么向内层应用回编码」，
对外层终端依旧只当传统终端。

### 做法

不依赖协商，在传统模式下就为「必然丢信息」的键发显式序列（详见
[`PLAN-legacy-extended-keys.md`](PLAN-legacy-extended-keys.md)）：

| 键 | 发出的字节 |
|---|---|
| `Shift+Enter` / `Ctrl+Enter` / `Ctrl+Shift+Enter` / `Alt+Shift+Enter` | `\x1b[27;2;13~` / `\x1b[27;5;13~` / `\x1b[27;6;13~` / `\x1b[27;4;13~` |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | `\x1b[27;5;9~` / `\x1b[27;6;9~` |
| `Shift+Esc` | `\x1b[27;2;27~` |
| `Ctrl+Shift+Space` / `Ctrl+Shift+Backspace` | `\x1b[27;6;32~` / `\x1b[27;6;127~` |
| `Ctrl+Shift+字母` / `Ctrl+Shift+数字或符号` | `\x1b[102;6u` / `\x1b[49;6u`（基底键 + Shift 位） |

命名键用 xterm 的 `modifyOtherKeys` 形式（ghostty 在未协商时对同一批键用的就是它），
字符键用 kitty 的 `CSI <码点>;<修饰>u`。经 tmux 实测，这些字节都会被 tmux 解析后
以 `CSI …u` 交给内层应用。

**不变的**：单独 Shift（大写字母、上档符号）、`Shift+Space`、单独 Ctrl（`Ctrl+C` 仍是 `0x03`）、
`Ctrl+Space`（`0x00`）、单独 Alt（ESC 前缀）、`Alt+Enter`、`Shift+Tab`（`\x1b[Z`）；
键位绑定仍然优先，因为绑定是先匹配的。

输入法提交的多码点文本和非 ASCII 字符不会被兜底吞掉，仍按普通文本发送。

详见 [`PLAN-legacy-extended-keys.md`](PLAN-legacy-extended-keys.md)。

### 需要注意

**alacritty 默认把 `Ctrl+Shift+F` 绑给了自己的搜索功能**，按键会被吞掉，发不到应用。需要在自己的 `alacritty.toml` 里解绑：

```toml
[keyboard]
bindings = [
{ action = "ReceiveChar", key = "F", mods = "Control|Shift" }
]
```

注意 `action = "None"` **不能**解绑 —— 它同样会吞掉按键，只是让 alacritty 什么都不做。唯一能透传的动作是 `ReceiveChar`。

---

## 修复三：输入法候选窗不跟随光标

### 症状

从别的 app 切回 alacritty 后，输入法候选窗不跟着光标走。

复现：

1. 打开两个 tmux pane，光标停在 pane A
2. `cmd+tab` 切走再切回
3. 点 pane B 打字

拼音字母出现在 pane B（终端光标确实移过去了），候选窗却弹在 pane A 的位置。**只在切回后第一次出现**，选完一个词之后再打字就正常了。

### 原因

alacritty 把光标位置交给系统用于摆放候选窗，这条路径只在绘制过程中被调用，且被 `ime.is_enabled()` 挡着 —— **IME 没启用时一次位置都不更新**。

而 macOS 上 winit 的 `Ime::Enabled` 只在输入法真正开始打字时才发，不随窗口重新获得焦点而发。于是切走后位置被冻结在旧处，切回后一打字，IME 立刻用那个旧坐标摆窗；1 毫秒后 alacritty 才推送正确位置，但窗已经摆完了。

埋点时间戳（MacBook M1 / macOS 27.2）：

```
T=…690.802  推送 grid=(2,2)      ← 焦点在 pane A
T=…691.543  Ime::Disabled        ← 切走
   ……5.6 秒，一次位置都没推过……
T=…697.185  Ime::Enabled
T=…697.185  preedit="n" …        ← 同毫秒，按旧坐标(列 2)摆窗
T=…697.186  推送 grid=(2,69)     ← 正确位置，晚了一步
```

### 做法

IME 停用期间也持续推送位置（`display/mod.rs`，12 行）。这样 IME 一旦启用，读到的永远是最近一帧的坐标。

代价是每帧多一次 `invalidateCharacterCoordinates()`，很轻。

详见 [`PLAN-ime-cursor-position.md`](PLAN-ime-cursor-position.md)。

---

## 修复四：点链接后鼠标形状卡在「手形」

### 症状

点一下超链接（焦点跳到浏览器），切回 alacritty 后，鼠标**仍然显示手形**，必须动一下鼠标才恢复。

### 原因

光标形状被窗口系统缓存，而且**只在鼠标移动时**才重新解析；窗口不是前台时，系统不派发鼠标移动。点链接那一刻形状被记成手形，切回来时形状「没变」，缓存继续生效 → 手形留在屏幕上。

### 做法

- 窗口重新获得焦点、或鼠标重新进入窗口时，强制重新应用一次光标形状（`display/window.rs`）。
- macOS 上额外调 `invalidateCursorRectsForView`，让系统按当前鼠标位置**重新解析**形状。这一步是必须的，否则「形状没变就跳过」的缓存还在。
- 同时清掉三个鼠标按键的按下状态：点按中途焦点被别的应用抢走时，松开事件会被吞掉，切回来后状态仍是「左键按着」，一动鼠标就被当成拖拽。

### 已知局限

视觉确认已做（2026-10-22 真机手动点一次超链接、切回确认）：光标形状立刻按当前鼠标位置重新计算并生效，不再停留在点击瞬间的手形。代码路径也已在真实 App 里跑过且无 panic（`--print-events` 能看到 `Focused(false)` / `Focused(true)` / `CursorEntered`）。

---

## 修复五：终端模式残留 & 一键软复位

### 症状

1. 应用同时开启 1000 + 1002 + 1003 三种鼠标上报模式时，后一个模式会把前一个清掉 —— tmux 的 `set -g mouse on` 正是这么设的，于是点击上报丢失。
2. 程序异常退出（典型如 ssh 断线）没恢复终端状态：点击变成转义序列而不是选文字、滚轮滚应用而不是回滚历史、停在备用屏出不来。

### 原因

- 鼠标追踪模式在协议上是**累积**的（1002 扩展 1000，1003 扩展 1002），上游实现却是「最后一个覆盖前面」。
- 终端模式残留**没法自动检测**：`alacritty_terminal` 只在 **pty 的直接子进程**（登录 shell 本身）退出时才会收到 `ChildEvent::Exited`。`ssh` 是 shell 的子进程，它死掉时 alacritty 收不到任何事件 —— 屏幕不重绘、鼠标位置不变、也没有退出通知。任何基于猜测的自动复位都会误伤 `vim` / `less` / `git` 这类会切换前台进程组的正常程序。

### 做法

1. `alacritty_terminal`：鼠标上报模式改为累积式；`1002l` 只清自己那一位，不再连带清掉 1003。
2. 新增 `Term::soft_reset()` 与 `ResetState` 按键动作：重置所有终端模式、退出备用屏，但**不清显示缓冲、不清回滚历史** —— 与 `ESC c` 不同，后者会把屏幕擦掉。

### 用法

```toml
[[keyboard.bindings]]
key = "R"
mods = "Alt"
action = "ResetState"
```

用来替掉 `sshcls` 这类 printf 式 shell 包装函数。

### 覆盖情况

| 回归测试 | 覆盖的行为 |
|---|---|
| `mouse_tracking_modes_are_cumulative` | 三个模式同时保持；旧逻辑下失败于 `term.mode.contains(TermMode::MOUSE_REPORT_CLICK)` |
| `soft_reset_restores_modes_and_keeps_history` | 模式复位、退出备用屏，且 `history_size` 不变、可见内容仍含原文本 |
| `reset_state_action_is_configurable` | TOML 里写 `"ResetState"` 能被解析、按键能触发 |

---

## 仓库内容

| 路径 | 说明 |
|---|---|
| `PLAN-bucket-sort.md` | 中文渲染方案：设计、验收标准、实测数据、复现步骤 |
| `PLAN-xtmodkeys-shim.md` | modifyOtherKeys 请求 → Kitty DISAMBIGUATE 的兜底 |
| `PLAN-legacy-extended-keys.md` | 未协商协议时 Shift/Ctrl 组合键的消歧义：设计、与 ghostty 对比、tmux 实测 |
| `PLAN-ime-cursor-position.md` | 输入法候选窗位置方案：根因、实测时间线、未验证项 |
| `PLAN-mouse-cursor-and-modes.md` | 鼠标形状与终端模式方案：验收标准、实测证据、已知边界 |
| `docs/cjk.fix.discuss.md` | 方案演进与评审记录（含一个被否决的动态扩容方案，勿照此实现） |
| `docs/features.md` | 上游功能文档，本 fork 追加了「从损坏的终端状态恢复」一节 |
| `testdata/cjk_bucket.txt` | 渲染卡顿复现语料：240 行 × 140 字，3199 个不重复汉字 |
| `FORK.md` | 分支与远端工作流 |

## 构建

```sh
make app      # 产出 target/release/osx/Alacritty.app
```

Apple Silicon 单架构。上游构建说明见 `INSTALL.md`。