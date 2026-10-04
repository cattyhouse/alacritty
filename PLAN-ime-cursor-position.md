# 输入法候选窗不跟随光标：原因与修复

## 症状

在 tmux 多 pane 场景下，从别的 app 切回 alacritty 后，输入法候选窗不跟着光标走。

复现步骤：

1. 打开两个 tmux pane，光标停在 pane A
2. `cmd+tab` 切到别的 app，再切回 alacritty
3. 点 pane B，打字

拼音字母出现在 pane B（说明终端光标确实已经移过去），但候选窗弹在 pane A 的位置。

**只在切回后的第一次出现**。选完一个词之后，再打字候选窗就正常跟随了。

## 原因

alacritty 通过 `Window::update_ime_position` 把光标位置交给系统，winit 再转给 AppKit，AppKit 用它摆放候选窗。

这条路径只在**绘制过程中**被调用：

`display/mod.rs` `draw()`
```rust
if self.ime.is_enabled() {
    if let Some(point) = ime_position {
        self.draw_ime_preview(point, fg, bg, &mut rects, config);   // 内部调 update_ime_position
    }
}
```

也就是说：**IME 没启用时，一次位置都不更新。**

而 macOS 上 winit 的 `Ime::Enabled` 事件（`winit/src/platform_impl/macos/view.rs`）只在输入法真正开始输入时才发，不随视图重新成为 first responder 而发。

于是形成这个时序：

```
切走前     IME 启用 → 位置持续更新，记住 pane A 的坐标
切走       Ime::Disabled → alacritty 停止更新位置
           期间光标移到 pane B，但没有任何一帧推送过新坐标
切回       视图重新成为 first responder，但没有 Ime::Enabled，alacritty 依旧不更新
开始打字   Ime::Enabled 与第一个 preedit 同一毫秒到达
           IME 立刻按「最后已知的坐标」摆窗 → 落在 pane A
           1 ms 后 alacritty 才完成这一帧的推送，位置已经改对了，但窗已经摆完
选完词     IME 结束这次组合，下次组合会重新查询坐标 → 恢复正常
```

## 实测证据

在 `update_ime_position` 里埋点打印时间戳与网格坐标，MacBook M1 / macOS 27.2，120 列 30 行窗口：

```
T=1791119690.802  推送 grid=(2,2)    ← 焦点在左 pane，位置写进 view
T=1791119691.543  ==== Ime::Disabled ====   ← 切到别的 app
   ……5.6 秒，alacritty 一次位置都没推过……
T=1791119697.185  ==== Ime::Enabled ====
T=1791119697.185  preedit="n"        ← 同毫秒，IME 用存的旧坐标(列 2)摆窗
T=1791119697.185  preedit="ni"
T=1791119697.185  preedit="ni h"
T=1791119697.185  preedit="ni ha"
T=1791119697.185  preedit="ni hao"
T=1791119697.186  推送 grid=(2,69)   ← 正确位置，晚了一步
```

关键点：`Ime::Enabled` 之后紧跟着就是 preedit，中间**没有任何一帧推送**；正确坐标晚了 1 ms 才到。

同时可确认候选窗位置确实受 alacritty 推送的位置驱动 —— 不切 app 时，候选窗横坐标会跟着光标列号线性移动（左 pane `x≈1113`，右 pane `x≈1522`，对应 61 列 × 6.5 pt ≈ 396 pt）。

> 说明：上面前后各若干轮对比里，基线版本的失败率为 5/5 与 3/3；修复后测得 5/5 跟随。两者未在同一次会话中并排跑完，样本量偏小。真正的确认来自作者的人工复测。

## 修复

`alacritty/src/display/mod.rs`，12 行：

```rust
if let Some(point) = ime_position {
    if self.ime.is_enabled() {
        self.draw_ime_preview(point, fg, bg, &mut rects, config);
    } else {
        self.window.update_ime_position(point, &self.size_info);
    }
}
```

IME 停用期间也持续推送位置。这样 IME 一旦启用，读到的永远是最近一帧的坐标，而光标一动终端就会重绘，位置总是新鲜的。

代价是每帧多一次 `invalidateCharacterCoordinates()`（AppKit 调用，很轻；IME 启用时本来每帧都在调）。

## 验收标准

- **给定** tmux 两个 pane、光标在 pane A，**当**切走再切回、点 pane B 打字，**则**候选窗出现在 pane B 的光标处。
- **给定** 同样的场景，**当**连续输入多个词，**则**每次候选窗都跟随，不只在第一次。
- **给定** 单 pane、不切 app 的普通输入，**则**候选窗位置与修复前一致。
- **给定** 搜索模式（`/` 打开搜索栏），**则**候选窗仍跟随搜索栏光标，不回归到网格光标。

## 未验证的部分

- **窗口失焦期间是否照常重绘**。若失焦时不重绘，本修复就覆盖不到「失焦期间移动光标」这一路径 —— 这正是本缺陷的场景，理应被覆盖，但未单独测量。
- tmux 路径下 `cmd+tab` 的时序可能与用「activate 另一个 app」模拟的不完全一致。
- Linux / Wayland / X11 未测。该改动对所有平台生效，Wayland 上候选窗位置由 compositor 决定，可能无关。

## 复现方法

需要一块能读到窗口坐标的探针（`CGWindowList` 即可，无需录屏权限），因为候选窗是系统 UI，无法用单元测试覆盖。

```sh
# 1. 编译带探针的版本，在 update_ime_position 里 eprintln 时间戳与网格坐标
# 2. 起 tmux（独立 socket，勿用日常 session），开两个 pane
# 3. 记下焦点在左 pane，切到别的 app，切回，点右 pane，立刻打字
# 4. 对比：探针最后推送的列号 vs 候选窗实测横坐标
```

关键是**在注入按键前校验目标进程身份**，否则按键会落到前台那个窗口去。
