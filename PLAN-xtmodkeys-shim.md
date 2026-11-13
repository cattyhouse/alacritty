# Plan: Alacritty XTMODKEYS shim (tmux `Ctrl+Shift+F` fix)

> **状态：已废弃并移除（superseded）。**
>
> 本计划假设 tmux 会把内层应用的 `CSI > 4;2 m` 转发给外层终端。设备实测（tmux 3.7，
> 本机 tmux 3.8 复核）证明它不会：外层 pty 收不到任何 `>4;` 序列，因为 macOS 自带
> terminfo 没有 `Eneks` 一项，tmux 没有可发的东西。所以这个 shim 在目标链路上从不
> 触发，却把「外部请求 XTMODKEYS」错误映射成「打开 kitty 消歧义」，会改变别的程序
> 看到的行为。
>
> 现在的做法：`set_modify_other_keys` 只记录状态、`report_modify_other_keys` 按真实
> 状态回报，两者都不再改动 `TermMode`。真正解决 `Ctrl+Shift+letter` 的是
> `PLAN-legacy-extended-keys.md` 里的兜底编码。

## Acceptance criteria

- [ ] Given tmux requests `CSI > 4;2 m` (XTMODKEYS EnableAll), when the user presses
      `Ctrl+Shift+F`, then Alacritty emits a distinguishable sequence carrying
      Ctrl+Shift (not bare `0x06` / `CSI 102;5u`).
      Failure behavior: still emits `0x06` (legacy) or `CSI 102;5u` (Ctrl only).
      **未达成，已撤销。** 设备实测 tmux 3.7/3.8 不转发 XTMODKEYS，shim 从不触发；
      把它改成「只记录状态」后，`xtmodkeys_is_tracked_and_reported` 断言它不再
      影响 `TermMode` `[measured]`。
- [x] Given NO protocol negotiated (legacy mode, e.g. outer side of tmux 3.7),
      when the user presses `Ctrl+Shift+letter`, then Alacritty emits Kitty-style
      `CSI <lower>;6u` (Shift preserved) instead of bare `0x06`.
      Failure behavior: emits `0x06`, tmux forwards `CSI 102;5u` (Shift lost).
      Evidence: new tests `ctrl_shift_letter_*` — behavior test FAILED on old logic
      (TEMP-REVERT-CHECK run `[measured]`), passes after fix; guard tests keep
      Shift-only (capitals) and Ctrl-only (`0x06`) on legacy path.
- [x] Given tmux requests `CSI > 4;0 m` (XTMODKEYS Reset), when the user presses
      `Ctrl+Shift+F`, then Alacritty goes back to legacy behavior.
      Failure behavior: stays in disambiguate mode after reset.
      Evidence: covered by the same unit test (Reset asserts legacy) `[measured]`.
- [x] Given an app pushed Kitty keyboard mode before/after XTMODKEYS, when either is
      popped/reset, then the other one's request is preserved (no clobbering).
      Failure behavior: Kitty push clears XTMODKEYS flag or vice versa.
      Evidence: push/pop section of the same unit test `[measured]`.
- [x] Existing `alacritty_terminal` test suite still passes (no secondary regression).
      Evidence: `cargo test -p alacritty_terminal --lib` → 133 passed, 0 failed `[measured]`.
- [ ] Real product check (needs user: restart Alacritty): fresh ssh (no local tmux) to
      sid, inside remote tmux run `python3 /tmp/keycap2.py` / `keycap3.py`, press
      `Ctrl+Shift+F` → `key hex` shows `...;6u` (mods 6 = Ctrl+Shift), then pi
      `Ctrl+Shift+F` opens transcript search. Recorded as pending until user runs it.

## Tasks

1. Failing regression test in `alacritty_terminal` for `set_modify_other_keys`
   shim (Enable → DISAMBIGUATE on, Reset → off, Kitty stack preserved).
   Done = new test fails on old code (`set_modify_other_keys` is a no-op), passes
   after fix. Could fail at: test harness `Term::new` config defaults.
   Status: DONE `[measured]` — failed before, passes after.
2. `alacritty_terminal/src/term/mod.rs`: track XTMODKEYS state and report it back.
   **不再把 Enable 映射成 Kitty DISAMBIGUATE**（原计划已被实测否定）。
   Done = 状态记录与回报只影响回报内容，不影响 `TermMode`。
   Could fail at: 与 `keyboard_mode_stack` 的 Replace 语义互相干扰。
   Status: DONE `[measured]` — `xtmodkeys_is_tracked_and_reported` 与
   `xtmodkeys_reports_the_actual_state` 通过；lib 测试 138 全绿。
3. Build release binary + macOS bundle, install to `/Applications/Alacritty.app`.
   Done = `/Applications/Alacritty.app/Contents/MacOS/alacritty --version` reports
   the new local commit. Could fail at: long compile, code-sign/notarization
   (ad-hoc), running-app replacement.
   Status: DONE `[measured]` — `cargo build --release` rc=0, `make app` ok,
   bundle replaced 2026-10-04 (binary 3199008 bytes). No Alacritty process was
   running at replace time.
5. Legacy Ctrl+Shift+letter disambiguation in `alacritty/src/input/keyboard.rs`
   (pure helper `ctrl_shift_letter_codepoint`, free `should_build_sequence`,
   `SequenceBuilder::try_build_textual` early arm; ASCII letters only, bindings
   still take precedence).
   Done = 新测试通过 + 全量测试通过。
   Status: DONE `[measured]` — bin 106/106, lib 138/138。
   fresh ssh, remote tmux, keycap2/keycap3 show `6u`, pi search opens.
   Done = user pastes the 5-line/3-line outputs showing `6u`.
   Could fail at: tmux still not offering (then re-investigate tmux outer query).
   Status: PENDING USER — 仍未做。自动化只覆盖到"alacritty 会发出的字节"与
   "这些字节经 tmux 到达应用"两段；物理按键 → 字节这一段需要人工在真机确认。
