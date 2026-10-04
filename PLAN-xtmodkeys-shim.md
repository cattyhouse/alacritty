# Plan: Alacritty XTMODKEYS shim (tmux `Ctrl+Shift+F` fix)

## Acceptance criteria

- [ ] Given tmux requests `CSI > 4;2 m` (XTMODKEYS EnableAll), when the user presses
      `Ctrl+Shift+F`, then Alacritty emits a distinguishable sequence carrying
      Ctrl+Shift (not bare `0x06` / `CSI 102;5u`).
      Failure behavior: still emits `0x06` (legacy) or `CSI 102;5u` (Ctrl only).
      Evidence: new unit test `xtmodkeys_shim_enables_disambiguate` failed before
      fix (`assertion failed: DISAMBIGUATE`), passes after `[measured]`.
      NOTE 2026-10-04: superseded — measured on-device that tmux 3.7 never forwards
      XTMODKEYS to the outer terminal (inner `CSI ? u` query gets only DA), so the
      shim never triggers in this chain. Kept as harmless fallback; the real fix is
      the legacy Ctrl+Shift+letter disambiguation below.
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
2. Implement shim in `alacritty_terminal/src/term/mod.rs`: track XTMODKEYS state,
   map Enable (both mode 1 and mode 2) to Kitty DISAMBIGUATE, implement report
   reply, preserve across Kitty stack push/pop/set.
   Done = unit test from task 1 passes + full module suite passes.
   Could fail at: interaction with `keyboard_mode_stack` Replace semantics.
   Status: DONE `[measured]` — 133/133 lib tests pass.
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
   Done = 4 new tests pass + full `alacritty` bin suite (89) and terminal lib
   suite (133) pass.
   Status: DONE `[measured]` — bin 89/89, lib 133/133.
   fresh ssh, remote tmux, keycap2/keycap3 show `6u`, pi search opens.
   Done = user pastes the 5-line/3-line outputs showing `6u`.
   Could fail at: tmux still not offering (then re-investigate tmux outer query).
   Status: PENDING USER — exact steps in the final report.
