# Alacritty (cattyhouse fork)

基于上游 [alacritty/alacritty](https://github.com/alacritty/alacritty)，针对 **macOS / Apple Silicon** 修两个具体问题。

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

## 修复二：Ctrl+Shift+字母在 tmux 下收不到

### 症状

在 tmux 里运行的 TUI 应用（例如 pi）收不到 `Ctrl+Shift+F` 之类的组合键。

### 原因

终端的传统编码无法表达 Shift 与字母的组合 —— `Ctrl+Shift+F` 和 `Ctrl+F` 只能发出同一个字节 `0x06`。

应用需要 kitty 键盘协议才能区分，但 pi 这类应用拿不到 kitty 协商时会降级发 `CSI > 4;2 m`（xterm modifyOtherKeys），而 **tmux 3.7 只对外层终端说 modifyOtherKeys，从不转发 kitty**，终端于是停在传统模式。

ghostty 的做法是不依赖协商，直接无条件发出可区分的序列。

### 做法

1. 收到 modifyOtherKeys 请求时改为开启 Kitty DISAMBIGUATE，借用已有的 Kitty 编码器保住修饰键；该状态能跨 Kitty 栈的 push/pop 存活，模式查询也会应答。
2. 作为兜底，即使没协商任何协议，`Ctrl+Shift+<ASCII 字母>` 也按 Kitty 格式编码 —— 与 ghostty 的做法一致。

单独 Shift（大写输入）和单独 Ctrl（`0x06`）行为不变；键位绑定仍然优先，因为绑定是先匹配的。

详见 [`PLAN-xtmodkeys-shim.md`](PLAN-xtmodkeys-shim.md)。

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

## 仓库内容

| 路径 | 说明 |
|---|---|
| `PLAN-bucket-sort.md` | 中文渲染方案：设计、验收标准、实测数据、复现步骤 |
| `PLAN-xtmodkeys-shim.md` | Ctrl+Shift+字母方案 |
| `docs/cjk.fix.discuss.md` | 方案演进与评审记录（含一个被否决的动态扩容方案，勿照此实现） |
| `testdata/cjk_bucket.txt` | 渲染卡顿复现语料：240 行 × 140 字，3199 个不重复汉字 |
| `FORK.md` | 分支与远端工作流 |

## 构建

```sh
make app      # 产出 target/release/osx/Alacritty.app
```

Apple Silicon 单架构。上游构建说明见 `INSTALL.md`。