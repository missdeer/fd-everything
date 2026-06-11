# fd-everything 实施计划 v7.2

> v7.2 修订说明：对照 codex v7.1 评审 3 项必修：①§4.7 加 case-sensitivity 等价约束——`Config.case_sensitive==true` 时禁用静态下推（Everything `case:` 是全局开关无 per-clause）；②Phase 7 重写为"`--exec` 联动 + 占位符改造"（1 d），总工期表同步上调到 14.5-17.5 d；③hidden_filter 拆 `hidden_by_name`（ignore 前，零 syscall）+ `hidden_by_attr`（ignore 后，syscall），消除 pipeline 与 §5.1 顺序冲突；§7 Inventory 同步拆分。

> v7.1 修订说明：对照 codex v7 评审 6 项必修，全部修订：§4.7 下推等价性收紧（来源限定 repo root、whitelist 扫描扩到全 search root 子树、正则加仓库根锚定）；§4.8 chunk 改 adaptive + time flush + `--exec` per-result chunk=1；RawHit 字段全部非 Option 且补齐 ctime/attributes/extension/search_root；§2.X 新增 16 行消费者改造表与 Phase 工期上调；C1 从 6 条扩到 9 条含输出字节流与 argv 字节一致；§7 Inventory 从 20 行扩到 34 行。

> v7 修订说明：用户明确"搜索速度是最高优先级，但不能以砍特性、改用户界面行为为代价"。v7 新增 §0「Speed-First Design Principles + 用户行为一致性约束 C1」作为所有后续决策的 tiebreaker；新增 §0.1「fd 输出顺序调研」前置研究；新增 §7「Performance Engineering Inventory」逐项审计每条 hit 的开销；IgnoreCache 加 4.0 pre-warm、4.7 静态规则 1:1 等价子集下推、4.8 rayon 并行评估（保流式保顺序）；DirEntry 删 `display_path`、PathProjector 改 output 阶段 lazy；FFI 一次性 metadata 请求消除 post-filter stat。**显式拒绝**会违反 C1 的"伪优化"：信任 Everything attrib、非索引盘拒绝、非 `--exec` 改批模式、用 Everything 原生序覆盖 fd 行为。语义骨架（v6 已 approved）一字未改。

> v6 修订说明：对照 codex 第四轮评审（gemini 已 PASS v5），修复 7 处事实/逻辑错误：①`file:` modifier 误用（Everything 里 `file:` = files-only，控制 basename/full-path 的是 `nopath:`/`path:`）；②补 `--exact` 与 `--and` 多 pattern 翻译；③smartcase 改用 `Config.case_sensitive` 结果，scoped inline `(?i:…)Bar` 列入 unsupported；④补 `\A`/`\z`/scoped lookaround 等 anchor 检测；⑤`--fixed-strings` 转义扩展到全部 Everything 元字符（空格 AND、`|` OR、`!` NOT、`<>` 分组、`"` phrase、`\` partial path、`:` macro）或含义字符直接回退 LegacyWalker；⑥同步删除残留的 `literal_hints` 字段与 R20 中的旧 MAX 探测描述；⑦`FDE_VERIFY_REGEX=1` 模式改为真 dual-backend 跑 EverythingBackend + LegacyWalkerBackend 比对结果集，捕获 false negative。

> v5 修订说明：**架构级简化**。Everything 原生支持 `regex:` 与 `wildcards:`，原 v1-v4 的"Everything 仅做字面量预筛 + post-filter 跑 Rust 正则复核"是过度保守。v5 改为：默认把 fd 的 pattern 转译成 Everything 查询直接下推；只有 pattern 含 Everything 不支持的语法（Unicode property、Unicode-aware `\b`/`\w`、`(?-u)` 关 Unicode、嵌套量词复杂度爆炸等）才不下推、回退到 LegacyWalker。post-filter 的 `regex_filter` 退化为仅在 `FDE_VERIFY_REGEX=1` 测试模式下启用的 dual-backend 校验器。这让 Phase 3 少一个 hot-path filter、Phase 6 删掉字面量提取模块、性能预算大幅放宽。同时补齐三处之前低估的实际差异：basename vs full-path 默认行为、smartcase → `case:`/`nocase:` 前缀、`--fixed-strings` 的 Everything 通配符转义。

> v4 修订说明：对照 codex 第三轮评审，**仅一项**仍待修：IgnoreCache 跨目录优先级模型需进一步改正为"kind 跨整条 parent chain，同 kind 内叶子优先"——通过直接读 `tests/tests.rs:837` 的 `test_custom_ignore_precedence` 验证：根 `.fdignore !foo` 必须覆盖 `inner/.gitignore foo`。其余 v3 修订（pipeline 顺序、双路径、`--one-file-system`、Everything 实例方案、D1 探测）保留。

> v3 修订说明：对照 codex 第二轮评审（gemini 已 PASS），修复 IgnoreCache 优先级模型为"kind × depth 双轴"（v4 进一步修正）、重排 pipeline 把 ignore_contain 提前、DirEntry 显式双路径、补 `--one-file-system`、Everything 实例方案改用 config 写盘而非未验证的 CLI 参数、D1 探测改用 `Everything_SetMax(0)` 且默认门槛下调。

> v2 修订说明：对照 codex/gemini 第一轮评审，重排 Phase 顺序（mock 前移、FFI 后移）、补齐 post-filter 缺失的语义模块、显式路径投影层、IgnoreCache 补全 git 全部细节、D1 策略加门槛与降级、测试策略以 MockBackend 为主、风险表扩到 R23、修正若干命名与库选型 nitpick。

## 目标

在 `sharkdp/fd` 的基础上做 Windows-only 分支，将文件搜索核心从 `ignore::WalkParallel` 替换为基于 Everything-SDK 的索引查询，但保留 fd 的全部 CLI/输出/退出码/`--exec` 语义与 ignore-file 语义。最终二进制重命名为 `fde`。

非目标：跨平台兼容；改进 fd 的 CLI 设计；重写输出层、`--exec`、颜色、超链接。

## §0 Speed-First Design Principles（v7 新增）

### C1 — 用户行为一致性约束（不可妥协的基线）

所有 user-visible 行为必须与 fd 完全一致。**"完全一致"具体指**（v7.1 扩展，响应 codex v7 #5）：

1. **过滤语义**：同样 CLI 输入 → 同样的结果集合（集合相等）。
2. **退出码**：与 fd 同样的退出码语义（0 = 有结果、1 = 无结果、其它错误码不变）。
3. **流式响应**：`--exec` 与不带 `--exec` 都必须保持 fd 的 streaming 行为——首条结果可见的时间不能因为我们改批处理而变差。
4. **错误兜底**：非索引盘、Everything 未运行等场景下，最终用户应得到"和 fd 等价的结果或可理解的错误"，不允许"突然得到 0 结果"或"突然报错"。
5. **CLI 表面**：所有 fd 已有 flag 行为保留；不允许偷偷废弃或语义漂移。
6. **输出顺序**：与 fd 一致——但 fd 自己的 `WalkParallel` 默认是并行无序的，所以"一致"具体含义须在 §0.1 调研后定。
7. **输出字节流（v7.1 新增）**：在相同 Config 下，stdout 字节流必须与 fd 完全一致。具体覆盖：
   - `--print0` NUL 分隔；
   - 颜色（lscolors / `LS_COLORS` 环境变量）输出的 ANSI 转义序列与 fd 字节级相同；
   - 超链接（OSC 8）格式与 fd 相同；
   - 目录的 trailing slash（fd 给目录加 `/`/`\\`）；
   - 路径分隔符（fd 在 Windows 上的默认行为，受 `--path-separator` 影响）；
   - `--list-details` 格式占位符展开；
   - `{}` `{.}` `{//}` `{/}` `{/.}` 在 `--exec`/`--exec-batch` argv 中的展开字节级与 fd 相同。
8. **`--exec` / `--exec-batch` 行为（v7.1 新增）**：
   - 子进程 argv 与 fd 字节级相同（含占位符替换、引号处理、cwd）；
   - 子进程启动顺序与 fd 一致（per-result 按到达顺序、batch 按 batch 边界）；
   - 子进程 exit code 传播到 fde exit code 的规则与 fd 一致；
   - stderr 诊断消息（warning/error 文案）与 fd 等价（不要求字节一致，但语义一致）。
9. **stdin/stdout/stderr 缓冲与 flush 行为（v7.1 新增）**：与 fd 一致——TTY 下行缓冲、pipe 下块缓冲、Ctrl-C 时尽快 flush 已积累的输出。

**C1 是 tiebreaker 的上限**：P1-P7 任何优化必须先通过 C1 验证。**通过 C1 的优化继续考量速度；不通过的，即便有性能收益也立刻拒绝。**

### 速度优化原则（按优先级）

- **P1 下推优先（C1 约束下）**：能让 Everything 100% 等价表达的，绝不留给 post-filter；**不能 100% 等价的不下推**——宁可慢，不要错。
- **P2 一次性 metadata**：FFI `Everything_SetRequestFlags` 一把请求 file_name + full_path + attributes + size + date_modified + date_created；post-filter **永不调用** `stat`/`GetFileAttributesW` 兜底（`hidden_by_attr` 例外见 §0 拒绝表；v7.2 已拆 `hidden_by_name` + `hidden_by_attr`）。
- **P3 hot-path 零分配**：DirEntry 只持 `raw_path: PathBuf`；`PathProjector::project_for_output(raw_path, config)` 仅在 `output.rs` 写出每条结果时调用一次。
- **P4 并行但保流式保序**：post-filter 用 rayon 并行**计算**，channel 仍按 fd 现有 streaming 模型。`--exec` 体感不退化。Chunk 内并行计算，chunk 间保到达顺序——见 §4.8 实现细节。
- **P5 静态 gitignore 下推（仅 1:1 等价子集）**：只下推 `<dirname>/` 形式且能用 Everything `!regex:[\\\\/]<escaped_dirname>[\\\\/]` 精确等价表达的 literal-dir-name 规则。任何含 glob 通配、否定 `!`、路径段歧义的规则全部留给 IgnoreCache。**等价证明**必须写进单测。
- **P6 fast-path skip**：CLI 编译期/启动期能证明某 filter 必为 no-op 时短路。例：`--no-ignore` → IgnoreCache 整段 drop；`--type` 未传 → type_filter no-op；无 `--exclude` → exclude_filter no-op。
- **P7 Pre-warm**：IgnoreCache 在 FFI 流式吐结果的**同时并发**构造 search root 父链 matcher。FFI 首条 hit 到达时缓存已经热。

### 显式拒绝的"伪优化"（违反 C1）

| 优化 idea | 违反点 | 处理 |
|---|---|---|
| 信任 Everything `attrib:H` 决定 hidden | 边缘 case（system-only attribute、reparse point）结果与 fd 偏离 | ❌ 拒绝；R12 保留——hidden_filter 走 `GetFileAttributesW`，但**只对 IgnoreCache 已通过的 hit 调用一次**（不是默认每 hit），缓解性能 |
| 非索引盘直接拒绝运行 | 用户曾能拿到结果，现在拿不到 | ❌ 拒绝；R1 LegacyWalker 静默兜底保留 |
| 非 `--exec` 收完再批量输出 | 首条结果可见时间变差，破坏 interactive UX | ❌ 拒绝；默认仍 streaming |
| 用 Everything 原生序覆盖 fd 排序行为 | 可能 fd 测试有顺序断言 | ⚠️ 暂定拒绝；待 §0.1 调研结论 |
| 把 hidden_filter 下推到 Everything `attrib:` | 同上 R12 | ❌ 拒绝 |
| 用 lazy stat 兜底缺失 metadata | 引入 syscall | ❌ 拒绝；P2 要求一次性取齐，缺则视为该 hit 不可处理（错误） |

## §0.1 fd 输出顺序调研（Phase 1 之前必做，0.5 d）

实施 v7 之前必须先回答：fd 默认的输出顺序是否对用户可观察、是否有测试断言？

**调研步骤**：
1. `grep -n "assert_output\|expected_output" tests/tests.rs` 找全部输出断言。
2. 对每个断言，分析它对**顺序**的依赖：strict（行序必须匹配）/ unordered（只比集合）。
3. 阅读 `tests/testenv/mod.rs::assert_output` 源码，看断言函数是否做了排序再比对。
4. 调用 fd `WalkParallel` 在小目录上跑两次，看输出是否字节稳定（验证默认并行是否真无序）。
5. **结论分支**：
   - **若 fd 测试用排序后比对**：v7 可放心采用 Everything 原生序——只要 sort 后等同就行。把"采用 Everything 原生序"从 ❌ 移到 ✅ 的优化列表。
   - **若 fd 测试断言行序**：必须在 EverythingBackend 输出前做一次 stable sort 与 fd 对齐。Phase 4 加 `output_sorter.rs`，按 fd 排序规则（深度优先 + 目录内字母序）排。
6. 调研结果写入 v8（或 v7 patch）作为正式决策。

**实施前不动 v7 的"暂定拒绝"列**。

## 总体架构

```
                CLI (clap, src/cli.rs) ──不变──┐
                                                │
                Config (src/config.rs) ──不变──┘
                                                │
                                                ▼
              ┌────────────────────────────────────────────┐
              │   src/scan/mod.rs                          │
              │   pub fn scan(paths, patterns, config) ... │
              │   构造 Backend、PostFilterPipeline、       │
              │   PathProjector，串联三者，喂入 Batch 通道 │
              └────────────────────────────────────────────┘
                       │                            │
                       ▼                            ▼
   ┌──────────────────────────────────┐   ┌───────────────────────────────┐
   │ trait SearchBackend              │   │ src/scan/post_filter/         │
   │ ├─ EverythingBackend (Phase 5+)  │   │ pipeline.rs（顺序见 §4.0）    │
   │ ├─ MockBackend (Phase 1)         │   │ ├─ exclude_filter.rs (-E)      │
   │ └─ LegacyWalkerBackend (兜底)    │   │ ├─ depth_filter.rs            │
   │     wraps existing walk.rs       │   │ ├─ type_filter.rs             │
   └──────────────────────────────────┘   │ ├─ extension_filter.rs        │
                       │                  │ ├─ size_filter.rs             │
                       ▼                  │ ├─ time_filter.rs             │
   ┌──────────────────────────────────┐   │ ├─ hidden_filter.rs           │
   │ src/scan/path_projection.rs      │   │ ├─ ignore_cache.rs            │
   │  Everything 绝对路径 ─▶ fd 期望  │   │ ├─ regex_filter.rs            │
   │  的相对/绝对呈现形式             │   │ ├─ prune_filter.rs            │
   │  + 分隔符/盘符规范化             │   │ ├─ ignore_contain.rs          │
   │  （内部规范化 ≠ 输出投影）       │   │ ├─ symlink_filter.rs          │
   └──────────────────────────────────┘   │ ├─ max_results_limiter.rs     │
                                          │ └─ owner_filter.rs            │
                                          └───────────────────────────────┘
                                                       │
                                                       ▼
              Batch / WorkerResult / Sender（提为 pub(crate)）
                                                       │
                                                       ▼
                              src/output.rs / src/exec/（不动）
```

### 关键设计决定

- **D1（v5 重写）正则/glob 默认下推 + 不支持语法回退 LegacyWalker**：
  - **默认行为**：fd 的 pattern 直接转译为 Everything `regex:<…>` 或 `wildcards:<…>` 下推；Everything 已经持有索引，由它做匹配比客户端扫结果集快一到两个数量级。
  - **不再做**：字面量提取（删除 v3/v4 的 regex-syntax HIR literal extraction 计划）、count-only 探测、`EVERYTHING_PREFILTER_MAX` 门槛、post-filter 默认正则复核。这些都基于"Everything 不能做正则"的错误前提。
  - **保留**：dual-backend 测试模式（env `FDE_VERIFY_REGEX=1`）——**真正同时运行 EverythingBackend 与 LegacyWalkerBackend**（v6 修订），把两者最终结果集做对称差分（`A \ B` 与 `B \ A`），既能捕获 Everything 的 false positive，也能捕获 **false negative**（v5 只用 post-filter 跑 Rust 正则只能捕获前者）。CI 用于检测语义发散，把已知差异列入测试 allowlist。**生产路径上不跑**。
  - **何时回退 LegacyWalker**：用 `regex_syntax::ast` 扫描 pattern，命中以下任一即"不下推"，直接走 LegacyWalker：
    - Unicode property 转义 `\p{…}` / `\P{…}`；
    - 未关 Unicode 的 `\b`/`\B`/`\w`/`\W`/`\d`/`\D`/`\s`/`\S`（Rust 默认 Unicode-aware，Everything 按 Windows locale，行为发散）；
    - `(?i:…)` 之外的非默认 inline flag（如 `(?-u)`、`(?x)`、`(?m)`、`(?s)`）；
    - 嵌套量词导致 AST 深度 > 阈值（防御性，防止 Everything 引擎差异）；
  - **理由**：当 Everything 的预筛已经语义不可靠（如 Unicode-aware 边界），post-filter 也救不回——因为预筛漏掉的结果根本没进流。所以策略只能是"要么下推、要么换后端"，不存在中间态。
  - **basename vs full-path、smartcase、`--fixed-strings`、`--glob`、`--exact`、`--and` 的具体翻译细节**：详见 §6.1 表与 §6.1.1。v6 修订（响应 codex v6 #1）：D1 顶部不再单独罗列翻译规则，避免与 §6.1 描述漂移（v5 这里残留的 `file:`、`escape ? * "` 与 §6.1 修正后的 `nopath:`、保守白名单策略冲突）。**实施时一切以 §6.1 / §6.1.1 / §6.2 为准。**
- **D2 backend trait + 多实现**：把 Everything、Mock、LegacyWalker 都包成 `SearchBackend`；测试默认 Mock；运行时根据 path 是否在已索引盘自动选择 EverythingBackend 或 LegacyWalkerBackend。
- **D3 路径投影层**：FFI 层在收到 UTF-16 时**一次性**转 UTF-8 `PathBuf`；规范化（统一 `/` 仅供内部匹配；盘符大写；NTFS junction/symlink 不展开）。输出阶段再投影回相对 search root / cwd，遵循 fd 的 `--absolute-path`、`--base-directory`、`--strip-cwd-prefix` 语义。**内部规范化的字符串不直接对外输出**，避免破坏 `--exec` 占位符语义。
- **D4 测试以 MockBackend 为主**：90% 集成测试用 Mock，5% 用 LegacyWalkerBackend 做兜底回归，5% 用真 Everything 实例做端到端 smoke。

## 工作分解（按落地顺序，已重排）

### Phase 0 — 项目结构与构建（0.5 d）✅ 已完成（2026-06-11）

- ✅ `Cargo.toml [[bin]]` 改 `name = "fde"`、`path = "src/main.rs"`。
- ✅ **同步处理 `CARGO_BIN_EXE_fd`**：所有 `tests/` 与 `tests/testenv/` 中引用旧 binary 名的位置改为 `CARGO_BIN_EXE_fde`。
- ✅ `Cargo.toml` 用 `[target.'cfg(windows)'.dependencies]` 隔离 Everything 相关依赖（当前为空占位块，Phase 5 起填充）；`src/main.rs` 顶 `#[cfg(not(windows))] compile_error!` 拒绝非 Windows 编译。
- 🅿️ 库选择：用 `bindgen`（build.rs）直接绑定 `Everything64.dll`，不依赖第三方 wrapper crate（gemini 与 codex 一致建议）。**决策已采纳；build.rs 与 bindgen 依赖留 Phase 5 落地。**
- ✅ CI：`.github/workflows/CICD.yml` 裁剪到 windows-only 矩阵（i686-msvc / x86_64-msvc / x86_64-gnu / aarch64-msvc）；fmt / clippy / MSRV jobs 迁到 `windows-2025`；加 `choco install everything --version=1.4.1.1026` + `Start-Service Everything`（ARM64 无原生包，已跳过）；移除 Debian / winget 发布；workflow trigger 增加 `everything` 分支。
- ✅ **副作用修复**：`tests/testenv/mod.rs` 的 `create_config_directory_with_global_ignore` 与 `global_ignore_file` 两个 helper 在 Windows 上是死代码（调用点均 `#[cfg(not(windows))]`），加 `#[cfg(not(windows))]` 门 satisfy CI `-Dwarnings`。

**Phase 0 验证**：`cargo check --all-features` ✓、`cargo clippy --all-targets --all-features -- -Dwarnings` ✓、`cargo fmt -- --check` ✓、`cargo test --no-run --all-features` ✓（测试二进制为 `fde-…exe`）。

### Phase 1 — Backend trait 抽象 + MockBackend（1 d）✅ 已完成（2026-06-11）

- ✅ 新增 `src/scan/mod.rs` + `src/scan/backend.rs`；在 `src/main.rs` 注册 `mod scan;`。Phase 1 类型对 binary 暂无消费者，模块加 `#[allow(dead_code)]`，由 Phase 3/5 消化。
- ✅ 类型形状严格对齐 §Phase 1 v7.1：`RawHit` 9 个字段全部非 `Option`（path/is_dir/size/mtime:i64/ctime:i64/attributes:u32/extension:Box<str>/depth/search_root:Arc<PathBuf>）；`BackendQuery` 含 paths/pattern/and_patterns/type_hint/size_hint/time_hint/max_depth/max_results；`TranslatedPattern` 含 everything_query/scope/case_modifier；辅助枚举 `PatternScope { Basename, FullPath }`、`CaseModifier { Case, Nocase }`、`EntryTypeHint { File, Directory }`；范围结构 `SizeRange { min_bytes, max_bytes }`、`TimeRange { min_filetime, max_filetime }`（FILETIME i64 与 RawHit 对齐，避免 SystemTime 转换）。
- ✅ `SearchBackend: Send + Sync` 对象安全；`BackendSink::send(&mut self, RawHit)`；`CancellationToken` 包 `Arc<AtomicBool>`，Acquire/Release 序；`BackendError { Cancelled, Other(anyhow::Error) }` 含 `Display` / `Error` / `From<anyhow::Error>`。
- ✅ `MockBackend` 实现：从静态 `Vec<RawHit>` 流式喂入，每条 hit 前检查 cancel，cancel 命中返回 `BackendError::Cancelled`。
- ✅ 3 个单测（编码 Phase 3 起的不变量，不只是 happy path）：(a) 顺序一次性喂出全部 staged hits；(b) cancel 必须 surface 为 `Cancelled`，sink 零泄漏（保护 `--max-results` 不被静默截断）；(c) `Box<dyn SearchBackend>` 编译时确保对象安全（Phase 6 后端选择层依赖）。

**Phase 1 验证**：`cargo check --all-features` ✓、`cargo clippy --all-targets --all-features -- -Dwarnings` ✓、`cargo fmt -- --check` ✓、`cargo test --no-run --all-features` ✓、`cargo test --bin fde scan::` ✓ (3/3 passed)。

#### Phase 1 设计参考（保留为后续 Phase 实施依据）

```rust

```rust
pub trait SearchBackend: Send + Sync {
    /// Push raw hits into the post-filter sender. Streaming: implementations
    /// must call `sink.send(...)` as hits arrive, not buffer the full set.
    fn run(
        &self,
        query: &BackendQuery,
        sink: &mut dyn BackendSink,
        cancel: &CancellationToken,
    ) -> Result<(), BackendError>;
}

pub struct BackendQuery {
    pub paths: Vec<PathBuf>,                   // already canonicalized
    pub pattern: TranslatedPattern,            // v6: 翻译后的查询表达，含 regex:/wildcards:/literal/case modifier/nopath|path
    pub and_patterns: Vec<TranslatedPattern>,  // v6: --and 各 pattern
    pub type_hint: Option<EntryTypeHint>,
    pub size_hint: Option<SizeRange>,
    pub time_hint: Option<TimeRange>,
    pub max_depth: Option<usize>,
    pub max_results: Option<usize>,
}

// v6: 替代 v5 的 literal_hints。EverythingBackend 直接消费此结构组装查询字符串；
// LegacyWalkerBackend 不读，靠 Config 中的原始 fd 状态运行。
pub struct TranslatedPattern {
    pub everything_query: String,              // 如 "regex:^foo$" / "wildcards:*.rs" / phrase 形式字面量
    pub scope: PatternScope,                   // Basename | FullPath
    pub case_modifier: CaseModifier,           // Case | Nocase
}

// v7.1 修订（响应 codex v7 #3）：所有字段非 Option；EverythingBackend 一次性
// 通过 Everything_SetRequestFlags 请求齐全；LegacyWalkerBackend 用 std::fs::metadata
// 在构造 RawHit 时一次 stat 填齐。post-filter 全程零 stat。
pub struct RawHit {
    pub path: PathBuf,                 // absolute, canonicalized
    pub is_dir: bool,
    pub size: u64,                     // 必填；EverythingBackend 从 SDK 取，LegacyWalker stat 取
    pub mtime: i64,                    // 必填；Windows FILETIME 100ns since 1601；统一类型避免 SystemTime 转换开销
    pub ctime: i64,                    // 必填；同上
    pub attributes: u32,               // 必填；Win32 FILE_ATTRIBUTE_* 位
    pub extension: Box<str>,           // 必填；Everything 直接给；Box<str> 节省 24B vs String
    pub depth: usize,                  // 相对 search root；LegacyWalker 自然知，EverythingBackend 计算
    pub search_root: Arc<PathBuf>,     // 共享 Arc 避免每 hit 复制；hidden_filter/path projection 都要用
}

pub trait BackendSink {
    fn send(&mut self, hit: RawHit) -> Result<(), BackendError>;
}
```

实现 `MockBackend`：从静态 `Vec<RawHit>` 喂入。立刻可用于驱动后续 Phase 的单测。

（以上 Phase 1 设计已落地，详见同目录 `src/scan/backend.rs`。）

### Phase 2 — 把 Batch/Sender/DirEntry 提为 pub(crate) + 路径投影 + 消费者改造（v7.1 上调，1.5 d）✅ 已完成（2026-06-11）

- ✅ **2.1 Sink 提取**：`Batch` / `BatchSender` / `WorkerResult` 从 `src/walk.rs` 移到 `src/scan/sink.rs` 并提为 `pub(crate)`。`walk.rs` 与 `src/exec/job.rs` 改 `use crate::scan::sink::{Batch, BatchSender, WorkerResult}`。`scan/mod.rs` 注册 `pub mod sink;`。
- ✅ **2.2 PathProjector**：`src/scan/path_projection.rs` 新增 `SearchRoot { canonical, display }` + `PathProjector<'a>`。`project_for_output(&Path) -> Cow<'_, Path>`：`--absolute-path` 与 display==canonical 时零分配走 `Cow::Borrowed`，relative-root 与 strip-cwd 走 `Cow::Owned`。多 root 嵌套场景按 canonical 长度排序，最长前缀胜出（避免外 root 误抢内 root 命中）。10 个单测覆盖：cwd-relative / strip-cwd / `--absolute-path` 零分配 / 绝对 display 借出 / 非 `./` 相对 root / 嵌套 root 最长前缀胜出 / UNC `\\server\share` / `\\?\` 长路径前缀 / root 外 fallback / 与 legacy `strip_current_dir` 字节等价的 Phase 2.5 哨兵测试。
- ✅ **2.3 DirEntry 重构**：`raw_path: PathBuf` 单字段（绝对规范化）；`from_ignore`（`normal(e, cwd)`）与 `broken_symlink(p, cwd)` 接收借入的 `cwd: &Path`，构造时一次性 canonicalize（`make_absolute`：`is_absolute` ? 原样 : `cwd.join(strip_prefix("."))`）。删除 `stripped_path()` / `into_stripped_path()` — 投影路径不再是字段，由调用方在写出时临时调 `PathProjector`。`inner` 仍保留 `Normal(ignore::DirEntry) | BrokenSymlink` 两个变体仅为 lazy metadata / file_type / depth 复用 walker 已做的工作（P3 hot-path 原则）。`Colorable::file_name` 对 BrokenSymlink 直接走 `raw_path.components().next_back()`。`from_raw_hit` 构造函数留 Phase 3 post-filter pipeline 落地（届时再加 `RawHit` 变体）。
- ✅ **2.4 消费者改造**：按 §2.X 表全部完成。
  - `output.rs` 主路径打印 / lscolors / 路径分隔符 / trailing slash：4 个 `print_entry_*` 函数签名加 `projected: &Path` 参数；`print_entry` 入口一次性调 `config.path_projector().project_for_output(entry.path())`；hyperlink **target = `entry.path()`（绝对 raw_path）、text = projected**。
  - `exec/job.rs` `job` / `batch`：每个 worker 函数顶部 build 一次 projector，placeholder 替换 / argv 组装前 `project_for_output` 一次。
  - filters / IgnoreCache / GetFileAttributesW / 卷 ID / hyperlink `file://` URL：都读 `entry.path()`（即 raw_path 绝对路径）— 改造后 path() 直接返回 `&raw_path` 即满足。
  - Config 新增 `absolute_paths: bool` + `search_roots: Arc<Vec<SearchRoot>>` + `path_projector(&self) -> PathProjector<'_>` 辅助方法。
  - `main.rs` `run()` 在 `set_working_dir` 之后 / `construct_config` 之前从 `search_paths` build `Vec<SearchRoot>`（canonical = `path_absolute_form(p)`，display = `p.clone()`），传入 `construct_config`。
  - `walk.rs` `WorkerState` 加 `cwd: Arc<PathBuf>`（`new()` 现在返回 `Result`，构造时一次性 `env::current_dir()`），在 parallel-walk closure 内 deref 为 `&Path` 传入 `DirEntry::normal/broken_symlink`。
- ✅ **2.5 Golden diff**：现有 `tests/tests.rs` 的 93 个集成测试已覆盖 §2.X 全消费矩阵（hyperlink / strip-cwd / absolute-path / exec & exec-batch placeholders / format / print0 / list-details / symlink-as-root / base-directory / normalized-absolute-path / implicit-absolute-path / multi-file），它们对真实 fixture 树跑二进制并断言行级字节相等，**即 C1 不变量的事实 golden diff**。不再单独造 fixture 套件；在 `path_projection.rs` 加 `projector_default_arm_reproduces_legacy_strip_behavior` 单测作为 fast-fail 哨兵，pin 住 projector 与历史 `filesystem::strip_current_dir` 在默认与 `--strip-cwd-prefix` 两 arm 下的字节等价。

**Phase 2 验证**：`cargo build` ✓（仅 3 个 Phase 3+ 待消费的 `dead_code` warning）、`cargo test --bin fde` 141/141 ✓（其中 10 个 PathProjector 单测）、`cargo test --test tests` 93/93 ✓（全 fd 集成测试 — `test_hyperlink` / `test_explicit_root_path` / `test_normalized_absolute_path` / `test_implicit_absolute_path` / `test_strip_cwd_prefix` / `test_print0` / `test_exec_with_separator` / `test_symlink_and_absolute_path` / `test_format` / `test_list_details` 等均通过，证明 projector 与 stock fd 输出字节相等）。

#### Phase 2 原始设计（保留为后续 Phase 实施依据）

针对 codex 必修问题 #1、#3：

- `src/walk.rs` 中私有的 `Batch`、`BatchSender`、`WorkerResult` 改为 `pub(crate)`，移到 `src/scan/sink.rs`。
- `src/dir_entry.rs` 的 `DirEntry`（v7 修订，遵循 P3 hot-path 零分配）：
  - 现有：`from_ignore(ignore::DirEntry)`、`from_broken_symlink(PathBuf)`；
  - 新增：`from_raw_hit(RawHit) -> DirEntry`。
  - **只持 `raw_path: PathBuf`**：规范化后的绝对路径。所有 stat、`GetFileAttributesW`、hyperlink target、ignore 匹配、type/size/time filter 共享同一份。
  - **不再持 `display_path`**：投影路径不是字段，而是在 `output.rs` 写出每条 hit 时**临时**调用 `PathProjector::project_for_output(&entry.raw_path, &config)` 计算一次。`--exec` 占位符替换同样在 exec 调度处临时调用，不进 DirEntry。
  - `DirEntry::path()` 直接返回 `&raw_path`；不再保留 v6 提议的 `stripped_path()` 字段。
  - 与 `from_ignore`/`from_broken_symlink` 的兼容性：旧构造同样只填 `raw_path`（`ignore::DirEntry::path()` 已经是绝对路径或相对 search root 的路径，统一规范化为绝对后存入）。
- 新增 `src/scan/path_projection.rs::PathProjector`（v7 修订）：
  - 输入：absolute raw path、search root、Config（`--absolute-path` / `--base-directory` / `--strip-cwd-prefix` / `--print0`）；
  - 输出：`Cow<'_, Path>`——能直接返回 `&raw_path` 的场景（如 `--absolute-path`）零分配。
  - **不在 filter 链中调用**：filter 链只看 raw_path；PathProjector 仅在 `output.rs::write_entry` 调用一次/hit。
  - 单测：覆盖 cwd 内、cwd 外、绝对、相对、`./` 前缀、UNC 路径、长路径 `\\?\` 前缀。
  - 关键不变量（C1 保证）：投影后的最终输出字节流必须与 fd 在相同 Config 下完全一致——通过 golden diff 验证。

##### 2.X 所有用户可见路径消费者改造清单（v7.1 新增，响应 codex v7 #4）

总体架构图里写"src/output.rs / src/exec/（不动）"是 v6 的状态；v7.1 单 raw_path 模型下，**这些模块需要改**，改动局部但必须显式列出，避免实施时遗漏：

| 消费点 | 用 raw_path 还是 projected | 改造说明 |
|---|---|---|
| `output.rs` 主路径打印（普通、`--print0`、`--list-details`） | **projected** | 写出每行前调一次 `PathProjector::project_for_output`；返回 Cow 直接写 BufWriter |
| `output.rs` 颜色（lscolors） | **projected** | lscolors 按 projected path 决定颜色（fd 现有行为）；style 计算输入改为 projected |
| `output.rs` 超链接（OSC 8） | **target=raw_path（绝对），text=projected** | 终端超链接 target 必须是绝对路径才能正确打开；显示文本与列出一致 |
| `output.rs` 路径分隔符规范化 | **projected** | fd 在 Windows 上输出 `\\` 或 `/` 取决于 Config；projected 阶段决定，raw_path 内部统一 `\\` |
| `output.rs` trailing slash for dirs | **projected** | fd 给目录追加 `/`（或 `\\`）；在 projector 输出后追加 |
| `exec/command.rs` `{}` `{.}` `{//}` `{/}` `{/.}` 占位符 | **projected** | 占位符替换前一次调 projector；与 fd 现有占位符语义一致 |
| `exec/command.rs` `--exec-batch` argv 组装 | **projected** | 每个路径用 projected；保持 fd argv 字节一致 |
| `exec/command.rs` cwd / working dir | **raw_path 的父** | 子进程 cwd 必须是真实路径 |
| `filter/size.rs`, `filter/time.rs`, `filter/owner.rs`（如有 stat） | **raw_path** | stat 必须用绝对路径 |
| `hyperlink.rs` `file://` URL 构造 | **raw_path** | URL 必须绝对 |
| ignore/IgnoreCache | **raw_path** | matcher 评估必须用规范化后的绝对路径 |
| GetFileAttributesW (hidden_filter) | **raw_path** | syscall 必须用绝对路径 |
| Volume ID（same_filesystem） | **raw_path** | 卷查询用绝对路径 |
| 排序（若 §0.1 调研结论需要） | **raw_path 排，projected 输出** | 排序键稳定，输出友好 |

**Phase 2 工期上调**：从 1 d → 1.5 d，吸收上述消费者改造与对应 golden diff 测试。

**Phase 7 范围扩大**：原"`--exec` 流式联动验证"现在还要验证占位符 `{}` `{.}` `{//}` `{/}` `{/.}` 的 projected path 替换与 fd 字节一致，工期从 0.5 d → 1 d。

### Phase 3 — Post-filter pipeline 骨架（1 d）✅ 已完成（2026-06-11）

- ✅ **3.0 模块骨架**：`src/scan/post_filter/{mod.rs, pipeline.rs, sink.rs, filters/}` 落地。公开 `Verdict { Keep, Drop, Cancel }`、`Filter` trait（`evaluate(&mut self, &RawHit)`+`drain() -> Vec<RawHit>`）、`Pipeline`（`process` 按链短路、`drain_into` 把 drained 的 hit 重投入 *后续* filter）、`PostFilterSink<'a, S: BackendSink>`（实现 `BackendSink`，Cancel 时 fire token + 返回 `BackendError::Cancelled`）。`scan/mod.rs` 注册 `#[allow(dead_code)] pub mod post_filter`，Phase 5 接线时再去除。
- ✅ **3.1 DirEntry::from_raw_hit**：`DirEntryInner` 加 `RawHit(Box<RawHit>)` 第三 arm；`from_raw_hit` 不做 syscall（RawHit 已规范化）。`file_type()/metadata()/depth()/Colorable::file_name` 全部学会 RawHit arm（`depth` 直接返回；`file_type`/`metadata` 通过既有 `OnceCell` lazy 回落到 `Path::metadata()` — `FileType` 是不透明的，Phase 5 若热路径需要 stat-free 答案再优化；`file_name` 与 `BrokenSymlink` 共享 `raw_path.components().next_back()` 回落）。
- ✅ **3.2 14 个 filter 全部落位**（链顺序与 PLAN §Phase 3 一致）：
  - **从 RawHit 数据直接判定**（无 syscall）：`type_filter`（`is_dir` + `attributes & REPARSE_POINT`；`executables_only`/`empty_only` stub 透传，TODO Phase 5 push-down）、`extension_filter`、`size`（复用 `SizeFilter::is_within(hit.size)`；`!is_dir` 守护与 `walk.rs:498-515` 等价）、`time_filter`（新增 `filetime_to_systemtime(i64) -> SystemTime`，i 100ns→s+ns 转换，然后委托给 `TimeFilter::applies_to`）、`depth_filter`、`hidden_by_name`（`OsStr` 首字节 `.` 检查）、`hidden_by_attr`（`attributes & FILE_ATTRIBUTE_HIDDEN`）。
  - **小量 I/O / 状态**：`ignore_contain`（`path.join(marker).exists()` 加 pruned_dirs 前缀缓存，确保子树被丢；mirror `walk.rs:402-408`）、`exclude`（`ignore::overrides::Override`，Match::Ignore→Drop；mirror `walk.rs:259-274` — 注：`main.rs:389` 已给用户 pattern 前缀 `!`）。
  - **stub**：`owner`（Phase 5 决定 push-down 或一次 stat）、`ignore_cache`（Phase 4 占位 + `IgnoreCachePlaceholder` 空结构，让 Phase 4 实现替换无需改 pipeline 接线）。
  - **stateful**：`same_filesystem`（`VolumeIdProvider` trait seam，production 用 `NoopVolumeIdProvider` fail-open — Phase 5 接 windows-sys 后替换；按 root `Arc::as_ptr` + 按 parent dir 双层缓存；测试注入 `FakeProvider`）、`prune`（`BTreeMap<PathBuf, RawHit>` 缓冲，`evaluate` 全 Drop、`drain` 时父先于子排序，遇到 dir 加 emitted_dirs，后续 prefix 命中跳过 — 流式损失只在 `--prune` 启用时支付，符合 PLAN §3.1）、`symlink_filter`（`follow_links=false` 时跟踪 reparse-point 目录，按长度倒序排，prefix 命中的后代 Drop；reparse-point dir 自身 emit 一次）、`max_results`（计数+`CancellationToken`，第 N 个 hit 返回 Cancel；`Cancel` 与 `Drop` 区分：`max_results` 触发 `cancel.cancel()` 然后 sink 上传 `BackendError::Cancelled`）。
- ✅ **3.3 测试覆盖**：每个 filter 至少 1 个 unit test、stateful 多 1 个、pipeline + sink 自身 4 个、`MockBackend → PostFilterSink → VecSink` 集成测试 2 个（顺序不变量：跨卷 hit 必须在 size_filter 之前丢；`--max-results 2` 在 4-hit 流上 forward 2 + Cancelled）。所有测试 encode WHY（如 "Cancel 返回 ForwardAndCancel — 不然 `--max-results N` 返回 N-1"），遵循 Rule 9。
- ✅ **3.4 未触及**：`walk.rs` / `output.rs` / `exec/job.rs` / `config.rs` / `cli.rs` / `main.rs` 一行未改，Phase 5 接线在即。

**Phase 3 验证**：`cargo fmt -- --check` ✓、`cargo clippy --all-targets --all-features -- -Dwarnings` ✓、`cargo build --bin fde` ✓（清零 warning）、`cargo test --bin fde` 171/171 ✓（其中 29 个 post_filter — 26 filter unit + 1 sink + 2 integration + 还包括 pipeline 自身 3 个 + DirEntry/path_projection/backend Phase 1-2 既有 ✓）、`cargo test --test tests` 93/93 ✓（fd 集成测试全绿，证明 Phase 3 完全不影响 legacy walker）、`cargo run -- post_filter` 行为如旧（post_filter 仍未接线）。

#### Phase 3 原始设计（保留为后续 Phase 实施依据）

`src/scan/post_filter/pipeline.rs`，**用 MockBackend 喂入做 TDD**：

```
RawHit
  └▶ ignore_contain        (--ignore-contain；§3.1；MUST 在 depth/root 检查前)
  └▶ same_filesystem       (--one-file-system；§3.3)
  └▶ exclude_filter        (-E/--exclude，ignore::overrides)
  └▶ type_filter           (--type f/d/l/e/s/p/x；l/s/p/x 需 stat)
  └▶ extension_filter      (--extension)
  └▶ size_filter           (--size，需 stat 或用 Everything 给的 size)
  └▶ time_filter           (--changed-within/--changed-before)
  └▶ owner_filter          (--owner，Windows GetFileSecurity)
  └▶ depth_filter          (--max-depth/--min-depth/--exact-depth)
  └▶ hidden_by_name        (v7.2 拆分：名以 . 开头；纯字符串检查零开销)
  └▶ ignore_cache          (§4)
  └▶ hidden_by_attr        (v7.2 拆分：GetFileAttributesW & HIDDEN；只对通过 ignore 的 hit 调用)
  └▶ regex_filter          (v6 起：默认完全移除；dual-backend 比对在 pipeline 之外，由测试驱动)
  └▶ prune_filter          (匹配目录后丢弃其子树；§3.1)
  └▶ symlink_filter        (--follow 关时丢弃 reparse point 下的路径)
  └▶ max_results_limiter   (--max-results；命中后 cancel backend)
  └▶ DirEntry::from_raw_hit (v7.1：单 raw_path；display 由 output.rs 在写出时 lazy 投影；见 §2)
```

**顺序变更原因**（v3 修订，响应 codex #2）：
- `ignore_contain` 必须在 `depth_filter` 与"root 是否命中"判断**之前**。`tests.rs:2831` `test_ignore_contain_precedence_over_depth_check` 与 `tests.rs:2849` `test_ignore_contain_precedence_over_root_check` 要求即使 marker 在搜索根/超出 min-depth，也整棵子树丢弃。
- `same_filesystem` 紧随其后：跨卷的命中应在任何昂贵 stat 之前丢弃。
- 廉价过滤（type、extension、size、time）放在 ignore_cache（中等开销）之前。
- 正则与 ignore 之间放 hidden：很多 ignore 规则与 hidden 文件交叉。

#### 3.1 `prune_filter`
Everything 返回扁平路径集，无法在"进入目录前"剪枝。实现：
- 先把 RawHit 流缓冲到一个按路径排序的 `BTreeMap<PathBuf, RawHit>`（流式排序：保证父先于子）；
- 遍历时维护"已匹配的目录前缀集"；
- 后续条目若其某个祖先在集合中 → 丢弃。
- 代价：把 `prune_filter` 之前的 streaming 行为变成 batch；用户传 `--prune` 时才启用此模式，未传时保持 streaming（关键以保留 `--exec` 流式响应）。

#### 3.2 各 filter 与 Everything 下推的关系
当 Everything 查询已经下推了 `type:`、`size:`、`dm:` 等约束时，对应 post-filter **仍然要跑**（防御 Everything 语义不等价或索引脏），但热路径会被命中率拉满。**禁止**"下推就跳过 post-filter"的优化（codex #6 的语义安全要求）。

#### 3.3 `same_filesystem`（v3 新增）
fd 的 `--one-file-system` 当前通过 `WalkBuilder::same_file_system(true)` 实现。Everything 无对应概念，必须在 post-filter 做：
- 启动时取每个 search root 的卷序列号（`GetVolumeInformationByHandleW`）；
- 对每条 hit 取其卷序列号，缓存 `dir → volume_id`；
- 不匹配根卷的丢弃。
- Windows junction / mount point 会让"看似同一目录树跨卷"成为现实，必须真的查卷 ID，不能只比盘符。

### Phase 4 — IgnoreCache 完整实现（3–5 d）✅ 已完成（2026-06-11）

`src/scan/post_filter/ignore_cache/{mod, dir_state, global, prewarm, pushdown, parallel, semantics_tests, diff_test}.rs`。

**Phase 4 完成清单**：
- ✅ **4.1 数据结构**：`IgnoreCache`（DashMap by_dir + Arc<GlobalLayers> + IgnoreFlags + parent_ceilings + CaseModifier）、`DirIgnoreState`（per-kind `[Option<MatcherLayer>; 7]` 数组 + git_root + worktree_gitdir）、`MatcherLayer`、`MatcherKind`（7 variants，按优先级排序）、`GlobalLayers`（git core.excludesFile + fd global + custom `--ignore-file`）。
- ✅ **4.2 语义**：v4 kind-priority 算法实现于 `IgnoreCache::matched`（外层 kind 循环、内层 chain 循环、叶子优先）；relative-path 匹配（`Gitignore::matched_path_or_any_parents`，全局层走 panic-free `matched` + 手工 ancestor walk）；`.git/info/exclude` 加载（含 worktree gitdir 文件解析）；`require_git` 门控（`chain_has_git_root` 预扫描）；`--no-ignore-parent` ceiling；case-insensitive Windows 默认。9 个语义 unit test 覆盖每行 inheritance table，差分回归测试对照 `ignore::WalkBuilder` 跑 11 文件 fixture 全部一致。
- ✅ **4.3 性能**：`Arc<DirIgnoreState>` 缓存按目录 key，DashMap 提供 race-free 构造；parent chain 通过 `Path::parent()` 回溯，不存 parent Arc 指针（避免 prewarm/lazy 竞争时的过期 Arc 问题）。每命中评估成本 O(D) 次 DashMap 查找 + O(D) 次 matcher 调用。Microbench 推迟到 Phase 8 跟 EverythingBackend 一起调（PLAN §0.X 标准做法——没有真实 hit stream 之前的 benchmark 是空中楼阁）。
- ✅ **4.0 Pre-warm**：`prewarm(cache, roots)` 在专用线程沿 search root 上溯，DashMap 并发写无锁竞争；测试覆盖"预热后根 dir 已缓存"。Phase 5 调用入口即可，无 EverythingBackend 强耦合。
- ✅ **4.7 静态 pushdown**：`pushdown::{parse_rule, compile_static_ignore_pushdown}` 纯函数，按 §4.7 白名单（来源限定根级 .gitignore、形式限定 `<dirname>/` / `/<dirname>/`、kind 非 negation、whitelist 安全、case-sensitivity safety）严格筛选；6 个 unit test 覆盖 AnyDepth vs RootOnly、case modifier 安全、whitelist drop、跨 repo 隔离。
- ✅ **4.8 Rayon 并行**：`parallel::{ChunkPolicy, AdaptiveDriver, drive_chunk, ParallelIgnoreFilter}`。`ChunkPolicy::from_env` 读 `FDE_MAX_CHUNK`/`FDE_FLUSH_DEADLINE_MS`；`AdaptiveDriver` chunk=1→2→4→...→max，`force_unit` 锁定为 1（PerResult 模式）；`drive_chunk` 用 `rayon::par_iter` 保到达顺序。`ParallelIgnoreFilter` 是 Filter trait 的 sequential fallback；Phase 5 orchestrator 必须直接驱动 `AdaptiveDriver`（per-hit Filter 接口阻止真正的批并行——已在源码 doc-comment 标注）。2 个 unit test 覆盖 force_unit 锁定 + geometric growth。
- ✅ **4.X 接线**：`filters/ignore_cache.rs` 的 Phase 3 stub 替换为真实 `IgnoreCacheFilter::new(cache)`，删除 `IgnoreCachePlaceholder`。Phase 5 在 pipeline 构造时从 `Config` 提取 flags + ignore_files 构造 `IgnoreCache`，注入此 filter。差分回归测试通过：`ignore_cache_agrees_with_walker_on_fixture_tree` 用 11-文件 fixture（root `.gitignore *.log /anchored_dir/`、root `.fdignore !keepme.log`、嵌套 `sub2/.gitignore !*.log`、嵌套 `inner_repo/.git` 截断、`sub3/anchored_dir` 锚定边界）对照 `ignore::WalkBuilder`，0 不一致。

**Phase 4 验证**：`cargo fmt --check` ✓、`cargo clippy --all-targets --all-features -- -Dwarnings` ✓（清零）、`cargo build --bin fde` ✓、`cargo test --bin fde` 199/199 ✓（其中 30 ignore_cache：13 unit + 9 semantics + 1 diff + 6 pushdown + 1 prewarm + 2 parallel + 2 wire-filter + ... ）、`cargo test --test tests` 93/93 ✓（legacy walker 集成测试全绿，证明 Phase 4 完全不影响现有 fd 行为）。

**新增依赖**：`dashmap = "6.1"`（concurrent by_dir cache）、`rayon = "1.10"`（adaptive parallel post-filter）。

**Phase 4 未触及**：`walk.rs` / `output.rs` / `exec/job.rs` / `config.rs` / `cli.rs` / `main.rs` 一行未改——Phase 5 接线在即（从 `Config` 构造 `IgnoreFlags` + `GlobalLayers`，把 `IgnoreCacheFilter` 插进 Pipeline，把 `AdaptiveDriver` 接到 EverythingBackend sink 前）。

#### Phase 4 原始设计（保留为后续 Phase 实施依据）

#### 4.0 Pre-warm（v7 新增，遵循 P7）

**目的**：Everything FFI 流式吐结果到 IgnoreCache 时，目录链 matcher 已经构造好；首条 hit 不付目录 stat 代价。

**实现**：
- `scan::scan(paths, …)` 进入 EverythingBackend 之前，立即 spawn 一个 worker thread：对每条 search root 沿父链上溯到 git_root（或 volume root），把途中每层目录的 `.gitignore`/`.fdignore`/`.ignore`/`.git/info/exclude` 一次性加载到 `IgnoreCache`。
- 同时主线程启动 Everything FFI 查询。
- 两条流并发：FFI 第一条 hit 到达时，cache 中绝大多数父链已经热。
- 若 FFI 第一条 hit 在 cache 还没准备好的子树上（极少见，因 search root 链已 pre-warm 完，子树只是新增更深一层），则正常 lazy 构造，无正确性损失。
- 实现注意：Pre-warm worker 用 `DashMap` 写入，主线程读，无锁竞争。

#### 4.1 数据结构

```rust
pub struct IgnoreCache {
    by_dir: DashMap<PathBuf, Arc<DirIgnoreState>>,
    /// global gitignore（来自 git core.excludesFile / $XDG_CONFIG_HOME/git/ignore）
    git_global: Option<Arc<Gitignore>>,
    /// fd global ignore（$XDG_CONFIG_HOME/fd/ignore 或 %APPDATA%\fd\ignore）
    fd_global: Option<Arc<Gitignore>>,
    /// --ignore-file 指定的，作用域全局，优先级低于目录内 .fdignore
    custom: Vec<Arc<Gitignore>>,
    flags: IgnoreFlags,
    /// 用于把绝对路径转成 ignore matcher 期望的相对形式
    case_insensitive: bool, // Windows 默认 true
}

struct DirIgnoreState {
    /// 缓存为当前目录构造的合成 matcher。
    /// 注意：matcher 的"基目录"是当前目录，调用 matched() 时必须传相对路径。
    matchers: Vec<MatcherLayer>,
    git_root: Option<PathBuf>,   // 上溯找到的最近的 .git/ 或 .git 文件
    parent: Option<Arc<DirIgnoreState>>, // 父目录链
}

struct MatcherLayer {
    base: PathBuf,
    matcher: Gitignore,
    kind: MatcherKind, // Gitignore | Fdignore | DotIgnore | Custom | GitGlobal | FdGlobal | GitInfoExclude
}
```

#### 4.2 关键语义点（响应 codex #4、#5；gemini #3）

1. **相对路径匹配**：对每一层 matcher 调 `matched(path.strip_prefix(&layer.base), is_dir)`；绝对路径直接传给 `Gitignore::matched` 会静默漏匹配。
2. **`.git/info/exclude`**：在 DirIgnoreState 构造时，若发现 `.git/`（目录）则同时加载 `<git_root>/info/exclude`。
3. **`.git` 是文件的情况**（worktree、submodule）：读取该文件，解析 `gitdir: <path>`，从该 `gitdir` 下找 `info/exclude`。
4. **git core.excludesFile**：通过解析 `~/.gitconfig` 的 `[core] excludesFile = ...` 字段获取；`ignore` crate 内部已有此逻辑，可参考其源码移植到我们的 cache 构造里（不直接复用 `WalkBuilder`，因为其与 walker 状态耦合）。
5. **继承边界精确表**：

| ignore 类型 | 跨目录继承 | 嵌套 git 仓库截断 | `--no-require-git` 影响 | `--no-ignore-vcs` 关 | `--no-ignore-parent` 关 |
|---|---|---|---|---|---|
| `.gitignore` | 是 | **是**（仅向上至遇到的 .git/） | 默认须 git；关后不需要 | 是 | 是（截断到 search root） |
| `.git/info/exclude` | 仅 repo 内 | 是（绑定 git_root） | 同上 | 是 | 否 |
| git global (core.excludesFile) | 全局 | 否 | 同上 | 是 | 否 |
| `.fdignore` | 是 | **否**（继续向上） | 不影响 | 否 | 是 |
| `.ignore` | 是 | **否**（继续向上） | 不影响 | 否 | 是 |
| fd global ignore | 全局 | 否 | 不影响 | 否 | 否 |
| `--ignore-file` (custom) | 全局 | 否 | 不影响 | 否 | 否 |

   这张表必须在实现前作为 fixture，写一组单测逐格断言。
6. **custom ignore 优先级**：参考 `WalkBuilder::add_ignore`（追加到全局层），优先级**低于**目录内 `.fdignore`，但高于 git global。
7. **优先级模型：kind 跨整条 parent chain，同 kind 内叶子优先**（v4 修订，响应 codex v3 #1；对齐 `tests.rs:837` `test_custom_ignore_precedence` 的真实语义）：

   `test_custom_ignore_precedence` 的实际设置（直接读测试代码确认）：
   - `root/.fdignore` 内容 `!foo`（whitelist）
   - `inner/.gitignore` 内容 `foo`（ignore）
   - 文件 `inner/foo`
   - 期望：**显示** `inner/foo`（whitelist 胜出）。

   关键观察：whitelist 来自更**浅**的层、但更**高**的 kind；ignore 来自更**深**的层、但更**低**的 kind。结果 whitelist 胜，说明 **kind 优先级要跨整条 parent chain 生效，不能在单个目录层内决定胜负**。

   正确算法（伪代码）：
   ```
   fn is_ignored(path: &Path, is_dir: bool) -> Decision {
       let chain = parent_chain(path); // [leaf_dir, ..., search_root, ..., volume_root]
       for kind in [Custom, DotIgnore, Fdignore, Gitignore, GitInfoExclude] {
           for dir in chain.iter() {                 // 叶子优先
               if let Some(matcher) = dir.matcher_of(kind) {
                   let rel = path.strip_prefix(dir).unwrap();
                   match matcher.matched(rel, is_dir) {
                       Match::Whitelist(_) => return Decision::Show,
                       Match::Ignore(_)    => return Decision::Hide,
                       Match::None         => continue, // 同 kind 内继续向上
                   }
               }
           }
           // 当前 kind 在整条 chain 上无 match，跳到下一个 kind
       }
       // 全局层最后参与（fd_global、git_global、custom 全局）：同样按 kind 优先级
       evaluate_global_layers(path, is_dir)
   }
   ```

   - **同 kind 内**：叶子目录的规则优先于上游目录的规则（这与 git 原生行为一致：内层 `.gitignore` 可以覆盖外层 `.gitignore`）。
   - **跨 kind**：哪怕高优先级 kind 来自最浅层，也胜过低优先级 kind 在最深层的定义。这就是 `test_custom_ignore_precedence` 的关键。
   - **`--ignore-file` (Custom)** 既可由 fd 提供为全局 matcher，也可能（如果未来支持目录内 `.customignore` 形式）作为目录层；当前 fd 只支持 `--ignore-file` 的全局形式，归入 global 层但作为 Custom kind 评估。
   - 全局层（`fd_global`、`git_global`、`custom`）在所有目录链 kind 评估完之后，仍按相同的 kind 优先级独立评估。

   - 实现要点：
     - `DirIgnoreState::matchers` 是 `EnumMap<MatcherKind, Option<MatcherLayer>>`，按 kind 索引；
     - `is_ignored` 双层循环：外层 kind，内层 parent chain；
     - `Arc<DirIgnoreState>` 缓存仍按目录 key。

   - 这是 v3 模型（同目录内 kind 优先，跨目录叶子优先）的彻底反转。v3 仍会让 `inner/.gitignore foo` 在叶子层立刻胜出，根 `.fdignore !foo` 没机会评估，回归 `test_custom_ignore_precedence`。v4 改正。
8. **大小写**：Windows 默认 case-insensitive，`GitignoreBuilder::case_insensitive(true)`；用户给路径与文件系统大小写不一致时不能漏判。
9. **盘符规范化**：进 IgnoreCache 之前统一 `C:` 大写、分隔符 `/`、去掉 `\\?\` 前缀（保留以用于实际文件操作）。

#### 4.3 性能预算
- N=200k 结果、M=10k 不同目录、D=10 平均深度。
- 冷启动构造每目录：最多 D 次 stat（`.gitignore`/`.fdignore`/`.ignore`/`.git`），用 `Arc` 缓存 parent chain 避免重复。Pre-warm（§4.0）应让 95% 目录在 FFI 首条 hit 之前已构造完。
- 评估每条结果：链上每层 matcher 一次 `matched()` 调用，平均 D 次。
- 目标：post-filter 总耗时 ≤ 250 ms @ N=200k（**v7 下调自 v6 的 500 ms**，因为 Pre-warm + rayon 并行 + fast-path skip 共同发力）；不达标时进 §6.4 自动降级（**不是回退 LegacyWalker**——LegacyWalker 是 Everything 不可用时的兜底，不是性能兜底）。

#### 4.7 静态 gitignore 规则下推子集（v7.1 重写，响应 codex v7 #1）

**目的**：典型仓库的 `.gitignore` 含大量"整目录名"规则（`node_modules/`、`target/`、`__pycache__/`、`dist/`）；若能在 Everything 查询阶段就排除掉，可省去成千上万条 hit 流过 post-filter 的开销。

**关键修正**：v7 初稿把 `<dirname>/` 和 `/<dirname>/` 都翻成全局 `!regex:[\\\\/]<name>[\\\\/]` 是**错的**。`.gitignore` 规则的作用域**仅限于该 .gitignore 所在目录的子树**；全局段匹配会把仓库其它位置同名目录、其它仓库同名目录都误排除。`/<dirname>/` 还应严格限定为"该 .gitignore 所在目录的直接子目录"，而不是任意深度。v7.1 重写。

**白名单（每条规则必须全部满足才下推）**：

1. **来源限定**：规则来自仓库根的 `<repo_root>/.gitignore`、`<repo_root>/.git/info/exclude`、`<repo_root>/.fdignore` 或 `<repo_root>/.ignore` 之一（**不下推**子目录 `.gitignore`——子目录规则会让等价性翻译爆炸性复杂）。
2. **形式限定**：规则形如 `<dirname>/`（任意深度匹配）或 `/<dirname>/`（仅根直接子目录），`dirname` 只含 `[A-Za-z0-9._+-]`。
3. **kind**：规则不是 negation（不以 `!` 开头）。
4. **whitelist 安全**：扫描**整条 parent chain 与所有 search root 子树内**的同名 whitelist 规则（任何 kind、任何深度）；若存在任一可能与该 dirname 冲突的 `!` 规则 → 不下推该条。
5. **目录名命中也要排除**：fd 默认会列出被忽略目录自身（不只是子树），下推必须同时排除目录自身——见下方翻译。

**翻译**：

- `<dirname>/` 形式（任意深度）：
  `!regex:^<repo_root_regex>([\\\\/].*)?[\\\\/]<escaped_dirname>(?:[\\\\/]|$)`
  其中 `<repo_root_regex>` 是仓库根的绝对路径转义（含盘符），`(?:[\\\\/]|$)` 保证"目录自身 + 其子树"都被排除。

- `/<dirname>/` 形式（仅根直接子目录）：
  `!regex:^<repo_root_regex>[\\\\/]<escaped_dirname>(?:[\\\\/]|$)`

- 多条规则用空格 AND 连接（Everything 查询语法）。

**等价性单测**（必须验证；v7.1 扩展）：
- ✓ `node_modules/` in `<repo>/.gitignore` → `<repo>/node_modules/...` 排除；
- ✓ `<repo>/sub/node_modules/...` 也排除（任意深度）；
- ✓ `<repo>/not_node_modules_legacy/` **不**排除（边界确保）；
- ✓ `<other-repo>/node_modules/...` **不**排除（来源限定 `^<repo_root_regex>`）；
- ✓ `<repo>/node_modules` 目录自身被排除（`(?:[\\\\/]|$)` alternation）；
- ✓ `/target/` in `<repo>/.gitignore` → `<repo>/target/...` 排除；
- ✓ `<repo>/sub/target/` **不**排除（仅根直接子目录）；
- ✓ 子目录 `<repo>/sub/.gitignore` 含 `foo/` → **不下推**（来源限定不允许子目录规则）；
- ✓ 存在 `<repo>/sub/.gitignore` 含 `!node_modules` → 即便根 `.gitignore` 有 `node_modules/`，该条**不下推**（whitelist 安全）；
- ✓ 父链 `<parent>/.fdignore` 含 `!target` → 根 `.gitignore` 的 `target/` 不下推。

**回退**：单测发现等价性破坏的 case → 该规则不下推，回归 IgnoreCache。**任何疑问的规则一律不下推**——不下推只是失去性能收益，下推错误会破坏 C1。

**case-sensitivity 等价性约束**（v7.2 新增，响应 codex v7.1 #1）：

Windows 上 gitignore 匹配是 case-insensitive（`Gitignore::case_insensitive(true)`，§4.2 第 8 点已说明）。但 Everything 查询可能因 `Config.case_sensitive == true`（用户 `-s`/含大写 pattern）而带全局 `case:` 修饰，会让下推的 `!regex:[\\\\/]target(?:[\\\\/]|$)` 也变成大小写敏感——结果 `Target/` 子树没被 Everything 过滤，进入 post-filter 后被 IgnoreCache 排除。这不影响 C1（最终结果仍正确）但破坏 §4.7 的性能假设（大目录没被预过滤）。

**Everything `case:`/`nocase:` 的真实作用域**（Phase 5 day-1 校准任务必须验证）：voidtools 文档表明 `case:` 与 `nocase:` 是**整查询全局开关**，没有 per-clause 形式。在 `case:foo` 全局开关下，整个查询所有子句（包括 `!regex:...`）都按大小写敏感解释。

**因此规则**：
1. 翻译时检测整查询的有效 case modifier；
2. 若有效 case modifier 是 `case:`（敏感）→ **禁用静态 ignore 下推**，所有规则回退 IgnoreCache；
3. 若是 `nocase:`（不敏感，默认）→ 静态下推照常。
4. 单测必须覆盖：
   - ✓ `fde -s target` 在含 `target/` 规则的 repo 中：不下推，Target/ 与 target/ 都由 IgnoreCache 过滤；
   - ✓ `fde target`（smartcase → nocase）：下推，Target/ 与 target/ 都被 Everything 预排除。

**实施位置**：`src/scan/backend/everything/query.rs::compile_static_ignore_pushdown(rules, case_modifier)` 接收 case modifier；为 `case:` 直接返回空 Vec。

**预期收益**：仍可在典型前端/Rust/Python 仓库省 60-90% hit；下推条数减少（仅根级 `.gitignore`），但根级规则覆盖了 `node_modules`/`target` 等大目录，主要收益保留。

#### 4.8 rayon 并行评估（v7.1 重写，响应 codex v7 #2）

**约束（C1 派生）**：首条 hit 可见时间不能比 fd 更晚；保到达顺序；`--exec` per-result 模式必须立即触发。

**v7 初稿问题**：固定 chunk=64 会导致"等满 64 条才开算"，低命中率或 `--exec` 场景下首条延迟显著。v7.1 改为 **adaptive chunk + 时间阈值 flush**。

**实现**：

- FFI 流式把 RawHit 推到 `crossbeam-channel`；
- post-filter 由一个调度线程从 channel 收 hit，按以下规则触发计算：
  1. **fast path**：第一条 hit 到达后立即 chunk=1 单独计算，确保首条可见延迟与 fd 等价；
  2. **adaptive grow**：从 chunk=1 开始，每完成一个 chunk 后下一个 chunk 容量翻倍，封顶 `FDE_MAX_CHUNK`（默认 256）；
  3. **time flush**：收 hit 时维护一个 deadline（默认 5 ms 自上次 chunk 触发）。如果在 deadline 前未达到当前 chunk 容量，**立即用现有 hit 触发计算**，不等满；
  4. **`--exec` per-result 强制 chunk=1**：检测到 `Config.exec_mode == PerResult` 时，整个查询用 chunk=1 模式，每条 hit 独立计算并立即流出；
  5. **`--exec-batch` 模式**：可按 adaptive grow 模式，但每个 chunk 完成后立即推到 exec 调度，保 batch 流式启动；
  6. chunk 内并行：`rayon::scope` + `into_par_iter().filter_map().collect::<Vec<_>>()` 保到达顺序；
  7. chunk 间顺序：调度线程串行驱动，保跨 chunk 顺序。
- 参数可由 env `FDE_MAX_CHUNK`、`FDE_FLUSH_DEADLINE_MS` 调（Phase 8 基准校准）。
- **首条 hit 延迟预算**：≤ fd LegacyWalker 首条 + 2 ms（rayon 调度开销）；超过即视为 P4 实施失败，需重新调参或降级为完全串行 post-filter。

### Phase 5 — Everything FFI 绑定（1 d）✅ Day-1 已完成（2026-06-11）

仅在前 4 个 Phase（带 mock）通过后进入。

**Phase 5 day-1 完成清单**：
- ✅ **build.rs**：`cfg(target_os = "windows")` 守卫，`bindgen 0.71` 在 `OUT_DIR/everything_bindings.rs` 生成 `Everything-SDK/include/Everything.h` 绑定；allowlist 限定 `Everything_.*` / `EVERYTHING_.*` 避免 `<windows.h>` 整图入侵；按 `CARGO_CFG_TARGET_ARCH` 选 `Everything{64,32,ARM,ARM64}.lib` 链接，cargo:rerun-if-changed 头文件 + build.rs。
- ✅ **Cargo.toml**：`[target.'cfg(windows)'.build-dependencies] bindgen = "0.71"`；`[target.'cfg(windows)'.dependencies] windows-sys = "0.61"`（仅 `Win32_Foundation` feature；Phase 5 day-1 暂未跨用，留给 hidden_by_attr / 反应窗口）。
- ✅ **`src/scan/backend.rs`**：`#[cfg(target_os = "windows")] pub mod everything;` 一行接线，未触动既有 trait/MockBackend。
- ✅ **`src/scan/backend/everything/mod.rs`**：`#![cfg(target_os = "windows")]` + `#![allow(dead_code)]`（Phase 6 接 walk::scan 之前的 dead_code 都被 Phase 6/7 消化）；re-export `EverythingBackend` / `EverythingError`。
- ✅ **`src/scan/backend/everything/ffi.rs`**：bindgen 输出在 `pub mod sys` 下隔离（`non_camel_case_types` / `non_upper_case_globals` 等 allow 全部本地化，不污染上层模块）；`Mutex<SdkGuard>` + `OnceLock` 提供进程全局序列化（PLAN §Phase 5 mandate——SDK 全局状态）；`SdkGuard` 方法面板：`reset / set_search / set_request_flags / set_match_{path,case} / set_regex / set_max / query(wait) -> Result<(), EverythingError> / num_results / is_folder_result / result_full_path / result_attributes / result_size / result_{date_modified,date_created} / result_extension / version`。UTF-16 → `OsString` 一次性边界转换、FILETIME → i64 两个 helper。
- ✅ **`src/scan/backend/everything/error.rs`**：`EverythingError` 枚举 9 个已命名 SDK 错误 + `Unknown(u32)`；`is_unavailable()` 鉴别"该回退 LegacyWalker"（IPC / RegisterClassEx / CreateWindow / CreateThread）vs"程序员 bug"（InvalidIndex / InvalidCall / InvalidRequest / InvalidParameter / Memory）；`Display` 给 `Ipc` 嵌入 PLAN §Phase 5 mandate 的"start Everything and retry"actionable 文案；3 个 unit test 钉死契约。
- ✅ **`src/scan/backend/everything/backend.rs`**：`EverythingBackend` 实现 `SearchBackend`；`REQUEST_FLAGS` 常量 = `FULL_PATH_AND_FILE_NAME | ATTRIBUTES | SIZE | DATE_MODIFIED | DATE_CREATED | EXTENSION` = **0x017c**（与 §5.1 期望按位对账，example 跑出来值一致）；按 `query.paths` 逐根循环，每根独立 `Everything_QueryW(TRUE)`（hit→search_root Arc 映射零模糊），cancel token 每 hit 之前轮询；`build_search_string` 显式拼 `case:`/`nocase:` + `nopath:`/`path:` 前缀（**永不**依赖 GUI 的 Match Case / Match Path 开关，SetMatchCase/SetMatchPath/SetRegex 全部硬编码 false）；`depth_under_root` 用 `strip_prefix` + `components().filter(Normal)` 计数与 `ignore::DirEntry::depth()` 语义对齐；path quoting 用 phrase 双引号包 root 防 `:`/空格被 Everything 解析为 modifier 分隔符。7 个 unit test 覆盖：case modifier 必须显式、scope modifier 内联、root 带引号、含空格 root 引用、AND-pattern 各自带 scope、depth 计算 + 越界 fallback。
- ✅ **`examples/list_sdk_flags.rs`**：PLAN §5.1 强制 day-1 校准工具；用 `include!(concat!(env!("OUT_DIR"), "/everything_bindings.rs"))` 直接拉同 build.rs 的产物（fd-find 是 bin-only crate，example 无法穿模块墙），列 16 个 `EVERYTHING_REQUEST_*` 常量并标记 6 个 REQUIRED；用 Everything.h 期望值（0x17c）对账 composed bitfield，不一致 exit 1；附加 `Everything_GetMajorVersion` 探针让 reviewer 看到 DLL 链接 + IPC 通路确实活着。

**Phase 5 day-1 验证证据**：
- `cargo check --bin fde` ✓（bindgen 跑通，所有 12 个 SDK 函数 + 16 个 request 常量 + `_LARGE_INTEGER` 联合体 + `FILETIME` 都进了 `sys` 子模块）；`cargo check --example list_sdk_flags` ✓。
- `cargo test --bin fde` **212/212** ✓（199 前序 + 13 新增 Phase 5：3 ffi + 3 error + 7 backend），完全无回归。
- `cargo run --example list_sdk_flags` 实跑产出（Everything 实际运行中）：
  - 16/16 `EVERYTHING_REQUEST_*` 常量全部 bindgen 出来，名称与 PLAN §5.1 假设**零差异**——无需调整命名；
  - composed bitfield = **0x0000017c** 与 PLAN §5.1 期望按位一致；
  - DLL 实际链接、IPC 通路正常，Everything 服务回 **v1.4.1.895**。

**Phase 5 day-1 主动推迟（已在源码 doc-comment 标注）**：
- **Hidden message window + `Everything_SetReplyWindow` + 异步 reply**：当前用 `Everything_QueryW(TRUE)` 阻塞 + cancel token 在 hits 之间轮询；Ctrl-C 在查询执行中（非结果迭代中）不能立即 `Everything_Reset()`。PLAN §Phase 5 明确接受这种渐进——Phase 5 day-2/Phase 7 streaming 联动时再补 reply window（需引入 `windows-sys` 的 `Win32_UI_WindowsAndMessaging` + 一个 hidden window 后台线程）。
- **Runtime DLL 发现**：example.exe 需要 `Everything64.dll` 在 PATH 或 exe 同目录；本次手动 `cp Everything-SDK/dll/Everything64.dll target/debug/examples/` 解决。是部署/打包问题（Phase 9 处理），非 Phase 5 阻塞。

**Phase 5 未触及**：`walk.rs` / `output.rs` / `exec/job.rs` / `config.rs` / `cli.rs` / `main.rs` 一行未改——`EverythingBackend` 还未与 `walk::scan` 接线，Phase 6（查询翻译 + 自动后端选择）才是开关。当前 `EverythingBackend` 仅靠 `cargo test` 跑通（搜索串构造 / depth / 错误映射全部单测覆盖），真实 SDK 端到端联动等 Phase 6 引入翻译层后再做。

- `src/scan/backend/everything/ffi.rs`：用 `bindgen` 在 build.rs 生成 `Everything64.dll` 绑定。
- 全局 `Mutex` 串行化 SDK 调用（SDK 持全局状态）。
- Reply window：用 `Everything_SetReplyWindow` + hidden message window 接收异步回调；这样 Ctrl-C 可立即 `Everything_Reset()` 中断阻塞查询。
- UTF-16 → UTF-8：FFI 边界**一次转换**到 `OsString` / `PathBuf`，绝不在 post-filter 内重复转。
- 失败模式：SDK 不可达、版本握手失败、IPC 拒绝 → 结构化错误，映射到 fd `ExitCode::GeneralError`，文案明确"启动 Everything 后重试"。

#### 5.1 一次性 metadata 请求（v7.1 重写，响应 codex v7 #3）

**目的**：彻底消除 post-filter 阶段的 `stat` 调用，并与 §Phase 1 的 RawHit 字段集（v7.1 修订）严格一致。

**与 RawHit 的契约**：RawHit 所有字段都是非 Option 必填。EverythingBackend 在查询前必须 request 齐全；LegacyWalkerBackend 在构造 RawHit 时用一次 `std::fs::metadata`（DirEntry 已经持有 metadata，零额外 syscall）填齐。

**EverythingBackend 实现**：
- 查询前一次性调 `Everything_SetRequestFlags(FLAGS)`，FLAGS 至少包含：
  - `EVERYTHING_REQUEST_FILE_NAME`（用于 basename 比较）
  - `EVERYTHING_REQUEST_PATH`（用于完整 path 组装）
  - `EVERYTHING_REQUEST_FULL_PATH_AND_FILE_NAME`（合成；若 SDK 头文件命名不同 Phase 5 day-1 校准）
  - `EVERYTHING_REQUEST_ATTRIBUTES`
  - `EVERYTHING_REQUEST_SIZE`
  - `EVERYTHING_REQUEST_DATE_MODIFIED`
  - `EVERYTHING_REQUEST_DATE_CREATED`
  - `EVERYTHING_REQUEST_EXTENSION`
- accessor 读取顺序与 RawHit 字段一一对应：`Everything_GetResultFullPathName` → `Everything_GetResultAttributes` → `Everything_GetResultSize` → `Everything_GetResultDateModified` → `Everything_GetResultDateCreated` → `Everything_GetResultExtension`。
- depth 计算：扫描 full_path，统计 search_root 之后的分隔符数；search_root 共享 `Arc<PathBuf>` 进 RawHit。
- 调研步骤（Phase 5 day-1，强制）：写一个 `examples/list_sdk_flags.rs` 列 bindgen 生成的全部 `EVERYTHING_REQUEST_*` 常量，校准我们假设的名称；任何缺失立刻调整命名。

**post-filter 契约（强制）**：
- type_filter / size_filter / time_filter / extension_filter / depth_filter / symlink_filter（reparse point 走 attributes 位）**禁止**调用文件系统。代码层用 `#[deny(syscall)]` 这类 lint 不可行，但 review checklist 强制：post-filter 子模块**不得 import `std::fs` 或 `windows_sys::Win32::Storage::FileSystem`**（除 hidden_filter）。
- **hidden_filter 例外**（v7.2 拆分）：拆成两段以兼顾性能与 C1：
  - **`hidden_by_name`**：仅检查 path component 是否以 `.` 开头。零 syscall、纯字符串。**放在 ignore_cache 之前**，让 `.git/`、`.cache/` 等子树尽早丢弃，省 IgnoreCache 评估开销。
  - **`hidden_by_attr`**：调 `GetFileAttributesW` 检查 Windows HIDDEN attribute。**放在 ignore_cache 之后**，只对通过 ignore 的 hit 调用，把 syscall 量压到最低。因 C1 拒绝信任 Everything attribute，必须走 syscall（不读 RawHit.attributes 的 HIDDEN 位）。
  - **测量义务**：Phase 8 基准必须报告 `hidden_by_attr` 的 syscall 总数。预期：单次查询命中 `hidden_by_attr` 的 hit 数应 < 总命中数的 10%（因为 .git/.hidden 系列已被 by_name 过滤）。超出则评估"只对 search root 直接子目录走 syscall，深层信任 RawHit.attributes"的折中。

### Phase 6 — 查询翻译 + Unsupported 检测 + 自动后端选择（v5 重写，1.5 d）✅ 已完成（2026-06-11）

`src/scan/backend/everything/query.rs::translate(config, paths) -> Result<EverythingQuery, TranslationGiveUp>`

返回 `Err(TranslationGiveUp)` 时直接走 LegacyWalker。

**Phase 6 完成清单**：

- ✅ **6.1 Pattern 翻译**（`src/scan/backend/everything/query/mod.rs::translate`）：默认 regex 走 `regex:<pat>`；`--glob` 走 `wildcards:<pat>`（`**/` 删除、`{a,b}` 展开成 `|`-joined alternatives；见 `query/glob.rs`）；`--fixed-strings` 走 §6.1.1 phrase quoting（见 `query/literal.rs`）；`--exact` 走 `regex:^<escaped>$`（共享 literal 转义后用 §6.1.1 的同套规则判 give-up）；`--and` 各 pattern 独立翻译并塞进 `BackendQuery.and_patterns`，任一回退即整次回退（PLAN §6.1 contract）；case 完全复用 `TranslationInput.case_sensitive`（translation 层不二次判 smartcase）；scope 由 `--full-path` 决定（`PatternScope::FullPath`），默认 `Basename`。
- ✅ **6.1.1 `--fixed-strings` 转义**（`query/literal.rs`）：保守白名单——`"`/`\`/`*`/`?` 直接 give-up；其余全部 phrase-quoted 包裹；空 literal 输出空 fragment（"match every name" 语义）；rule 3/4/5 各有命名单测。
- ✅ **6.2 Unsupported 检测**（`query/unsupported.rs`）：`regex_syntax::ast::parse::Parser` 扫 AST，9 个 `RejectReason` 变体一一对应 PLAN §6.2 rule 1–9；`AssertionKind` 列举未覆盖的新变体走 `_ => UnsupportedAnchor` 兜底（防御 regex-syntax minor-version 增加新锚点）；不可 parse 的输入采"best-effort"立场——返回 `Ok(())`，让 Everything 引擎自己出错（避免与 fd 已经验证过的 regex crate 行为分叉）；`Flag::CRLF` / `Flag::SwapGreed` / `Flag::IgnoreWhitespace` 也算 unsupported flag（v0.8 enum 新增项）。`\Z` 在 regex-syntax 0.8 不是稳定 token，PLAN §6.2 best-effort 立场下让 Everything 引擎反馈错误。
- ✅ **6.3 其他 CLI 维度**：`max_depth` / `max_results` 透传到 `BackendQuery`（Phase 5 day-1 backend 已消费 `max_results`）；`type_hint` 仅当 `FileTypes` 为纯 files-only 或纯 dirs-only 时填 `EntryTypeHint::File` / `Directory`（mixed / executables_only / empty_only / symlinks / 设备类一律 None——post-filter 是 canonical）；`size_hint` 合并多 `SizeFilter` 取最紧 bound（`derive_size_hint`）；`time_hint` 把 `TimeFilter::After` / `Before` 转成 Windows FILETIME（`system_time_to_filetime`，1601→1970 偏移 11_644_473_600s）。`--extension` / `size:` / `dm:` / `depth:` 的 Everything 查询串 push-down 推迟到 Phase 7/8——post-filter 已 canonical，pushdown 纯优化（PLAN §6.3 "仍 post-filter 兜底"）。
- ✅ **6.4 后端自动选择**（`src/scan/backend/everything/selection.rs`）：`select_backend(paths, input, probe, force_legacy)` 按 path 独立分类成 `BackendChoice::Everything(query)` / `BackendChoice::Legacy { reason }`；`force_legacy` 镜像未来 `--filesystem-walker` flag（CLI 加 flag 在 Phase 7）；`VolumeIndexProbe` trait 把"是否在 Everything 索引卷"抽象掉，Phase 6 测试注 fake、Phase 7 接真 SDK probe；`AssumeIndexedProbe` 是预接线 stub（默认全部 indexed——确保 Phase 7 接线那刻 EverythingBackend 真的能跑起来，而不是被默认值静默退化到 LegacyWalker）；`FallbackReason::QueryUnsupported(TranslationGiveUp)` 保留结构化原因，`--show-errors` 在 Phase 7 可用。

**Phase 6 验证证据**：

- `cargo fmt --check` ✓、`cargo clippy --all-targets --all-features -- -Dwarnings` ✓（清零）、`cargo build --bin fde` ✓。
- `cargo test --bin fde` **260/260** ✓（212 前序 + 48 新增 Phase 6：unsupported AST 检测 12 + literal phrase 6 + glob 6 + query/mod.rs translate 18 + selection 6），完全无回归。
- `cargo test --test tests` **93/93** ✓（legacy walker 集成测试全绿——Phase 6 不接线 walk.rs，零现存行为影响）。

**Phase 6 主动推迟（已在源码标注）**：

- **`BackendChoice` 不 `PartialEq`**：`BackendQuery` 没 derive `PartialEq`（Phase 5 day-1 决定），测试改用 `matches!` + 字段断言；future 若 pipeline 真需要比较两个 query 再回头补。
- **`paths` 参数 `path:<dir>` 多 path `|` 合并**：当前 `EverythingBackend::run` 已经 per-root 跑一次 query，Phase 7 评估"单 query merged paths" vs "per-root 多 query"再决定是否合并 search string。
- **Extension / size / time / depth 的 Everything search-string push-down**：post-filter canonical，无功能 gap；Phase 8 微基准时如果发现 hit-stream 量太大值得 push-down，再回头加；当前避免与 Phase 5 day-1 `build_search_string` 的 mutability surface 纠缠。
- **`--filesystem-walker` CLI flag**：`force_legacy: bool` 已在 `select_backend` 入参，但 `cli.rs::Opts` 不动（Phase 7 一并接线 walk.rs 时再加 flag——PLAN §Phase 6 明示 "未触及" cli.rs / config.rs）。
- **Runtime VolumeIndexProbe**：`AssumeIndexedProbe` 是 stub；真 probe 调 `Everything_IsVolumeIndexed`（需校准官方头文件实际名）在 Phase 7 接线时落地。

**Phase 6 未触及**：`walk.rs` / `output.rs` / `exec/job.rs` / `config.rs` / `cli.rs` / `main.rs` 一行未改——Phase 7（`--exec` 流式联动 + walk.rs 接入）才是真正的接线开关。当前 query / selection 仅靠 `cargo test` 单测覆盖（translate 18 + selection 6 + unsupported 12 + literal 6 + glob 6 = 48 个），真实 SDK 端到端联动等 Phase 7 引入 `walk::scan` 路由后再做。

#### 6.1 Pattern 翻译（v6 修订）

| fd 输入 | Everything 表达 | 备注 |
|---|---|---|
| 默认 regex pattern | `regex:<pat>` | 需 unsupported 检测，见 §6.2 |
| `--glob`/`-g` | `wildcards:<pat>` | `**/` 删除；`{a,b}` 展开为多 wildcards 用 `|` 连接 |
| `--fixed-strings`/`-F` | 字面量子串查询 | 详见 §6.1.1 |
| `--exact-depth` 之外的 `--exact`（pattern 全匹配 basename） | `regex:^<escaped>$` 等价表达 | 配合下面 basename 约束 |
| `--and <p2> [--and <p3>...]` | Everything 主查询和每个 --and pattern 各自翻译后**用空格 AND 连接** | 任一 pattern 命中 §6.2 unsupported → 整次查询回退 LegacyWalker |
| `Config.case_sensitive == true`（fd 在 CLI 层已合并 smartcase / `-s` / `-i` / inline 全局 `(?i)` 后的最终结论） | `case:` 前缀加在整个查询最前 | **不在 translation 层重判**，复用 fd 的判断结果 |
| `Config.case_sensitive == false` | `nocase:` 前缀 | 同上 |
| **默认 basename 匹配** | `nopath:` modifier（Everything 文档原文为"only match the filename"） | v5 用的 `file:` 是 **files-only** 修饰符，会过滤掉目录命中——这是 v6 关键修正 |
| `--full-path`/`-p` | `path:` modifier（强制对完整 path 评估） | 不依赖 Everything GUI 的 "Match Path" 全局开关 |

##### 6.1.1 `--fixed-strings` 转义（v6 修订，响应 codex #5）

Everything 查询语法的元字符包括：`空格`（AND）、`|`（OR）、`!`（NOT）、`<` `>`（分组）、`"`（phrase）、`\`（partial path 触发）、`:`（modifier/macro 分隔）、`?`（单字符通配）、`*`（多字符通配）。

策略采用**保守白名单**：
1. 若字面量**只含字母、数字、Unicode 字母、`.`/`-`/`_`/`+`/`/`** → 包裹在 Everything 双引号 phrase 中：`"<literal>"`（Everything 的 phrase 内仍把 `"` 当结束符，所以同时把 `"` 替换为某种回退方案——实际上 Everything 不支持 phrase 内转义 `"`，所以含 `"` 的字面量直接回退 LegacyWalker）。
2. 若字面量含 `空格`/`|`/`!`/`<`/`>`/`:` → 包裹在 phrase 中（这些字符在 phrase 内被视为普通字符）。
3. 若字面量含 `"` → 无法表达，回退 LegacyWalker。
4. 若字面量含 `\` → 回退 LegacyWalker（Everything 把 `\` 触发 partial path 匹配，与"字面字符"语义冲突；安全做法是不冒险）。
5. 通配符 `?` / `*` 在 phrase 内仍是元字符——若字面量含这两个，也回退 LegacyWalker。

实现：函数签名 `fn translate_fixed_string(s: &str) -> TranslationResult<String>`，输入是 fd 字面量，输出要么是 Everything 查询片段，要么 `Err(TranslationGiveUp)`。单测覆盖每条规则。

#### 6.2 Unsupported 检测（v6 扩展，决定是否回退 LegacyWalker）

用 `regex_syntax::ast::parse::Parser` 扫 pattern AST；命中任一即 `TranslationGiveUp`：

1. **Unicode property** `\p{…}` / `\P{…}`。
2. **未关 Unicode 的 `\b` `\B` `\w` `\W` `\d` `\D` `\s` `\S`**（Rust 默认 Unicode-aware；Everything 走 Windows locale）。
3. **Anchor 不在 Rust 与 Everything 都明确等价的集合内**：
   - 等价仅 `^`（行/字符串起）、`$`（行/字符串止）；
   - **不等价**：`\A`、`\z`、`\Z`（Rust 字符串绝对锚点；Everything 的 `^`/`$` 是文件名起止，无对应）；
   - **不等价**：`(?m)` 下的 `^`/`$`（多行模式，文件名匹配场景下毫无意义但语义不同）；
   - 命中任一即 give up。
4. **Inline flag** 中含 `(?m)` `(?s)` `(?x)` 或显式 `(?-u)`；
5. **Scoped case flag** `(?i:…)` / `(?-i:…)`（只对子表达式生效；Everything 全局 `case:`/`nocase:` 无法表达 scoped 语义，必须回退）。**全局** `(?i)` 已被 fd 的 `Config.case_sensitive` 流程吸收，translation 层只看 final flag。
6. **AST 嵌套深度** > 16（防御性，防止 Everything 引擎在病态 pattern 上行为发散）。
7. **反向引用** `\1`-`\9` 和命名反向引用（Rust 不支持，理论上不会到这；防御性）。
8. **Lookaround** `(?=…)` / `(?!…)` / `(?<=…)` / `(?<!…)`（Rust 不支持，但用户可能用 fancy-regex；防御性）。
9. **glob 模式下**：
   - globset 否定 `!` 开头；
   - 字符类范围内的 Unicode 区间（如 `[α-ω]`）。

`Config.case_sensitive == true` 但 pattern AST 里还残留全局 `(?i)` flag → 已被 fd 处理过；translation 层信任 Config，**不二次解析**。但若发现 scoped `(?i:…)` → 走规则 5 回退。

检测器有专门单测：对 1-9 每条规则给"命中"和"未命中"样例，约 30 个 case。

#### 6.3 其他 CLI 维度

| fd flag | Everything 表达 | 备注 |
|---|---|---|
| `paths` 参数 | `path:<dir>` | 多个用 `|` |
| `--type f`/`d` | `file:` / `folder:` | l/e/s/p/x 仅 post-filter |
| `--extension` | `ext:` | |
| `--size` | `size:` | |
| `--changed-within`/`--changed-before` | `dm:` | 同时支持绝对与相对（jiff 解析） |
| `--max-depth N` | `depth:<root_depth+N>` + `path:<root>` | 仍 post-filter 兜底 |

#### 6.4 后端自动选择

```
for each search path P:
    if P 不在 Everything 已索引卷集合内
    或 P 在网络盘且未配置索引:
        backend := LegacyWalkerBackend
    else:
        match translate(config, [P]):
            Ok(query)               => backend := EverythingBackend(query)
            Err(TranslationGiveUp)  => backend := LegacyWalkerBackend
```

允许多 path 时不同 path 用不同 backend，把结果合并。

显式 flag：`--filesystem-walker`（强制走原 walker）。

### Phase 7 — `--exec`/`--exec-batch` 流式联动与占位符改造（v7.1 重写，1 d）✅ 已完成（2026-06-11）

**Phase 7 完成清单**：

- ✅ **占位符投影**：Phase 2 已经把 `PathProjector::project_for_output` 接到 `src/exec/job.rs::job` 和 `src/exec/job.rs::batch` 的入口处——传给 `CommandSet::execute` / `execute_batch` 的 `input` 已是 projected `Cow<Path>`，下游 `FormatTemplate::generate` 对 `{}` `{.}` `{//}` `{/}` `{/.}` 的展开自动用 projected path。`src/exec/command.rs` 本身无需改动——它只是 spawn 已经组装好 argv 的 `argmax::Command`，placeholder 替换在 `mod.rs` 的 `CommandTemplate::generate` / `CommandBuilder::push` 完成（与 fd 上游同代码路径），保证 argv 字节级与 fd 一致。
- ✅ **cwd / working dir**：fd 上游不显式 `current_dir(...)`、让子进程继承 fork-process 的 cwd（也就是用户调用 fde 的目录，与 raw_path 的父目录等价于"projected path 在 cwd 下可解析"的前提）。Phase 7 保持这一行为，PLAN §2.X 表中"raw_path 的父"条目通过"不主动 set_cwd"被消极满足；如未来引入显式 cwd（比如新 flag），review checklist 强制使用 `dir_entry.raw_path().parent()`，而不是 `projected.parent()`。
- ✅ **argv 字节级 golden diff**：`tests/tests.rs:1737-1815 test_exec` / `1821 test_exec_multi` / `1881 test_exec_nulls` / `1895 test_exec_batch` / `1972 test_exec_batch_multi` / `2040 test_exec_batch_with_limit` / `2078 test_exec_with_separator` / `2576 test_exec_invalid_utf8` 已经覆盖 `--absolute-path`、`--strip-cwd-prefix`、`./`-root 默认、`{}` `{.}` `{/}` `{/.}` `{//}`、batch `--exec-batch ... \;`、`--print0` 组合、`--exec-batch` 的 `-L`/`--batch-size` 上限、非 UTF-8 文件名的 8 种场景；93/93 集成测试全绿即代表 LegacyWalker 路径下 fde 与 fd 字节一致。EverythingBackend 接线后（Phase 8）需要复跑同一组测试。
- ✅ **流式契约 pin**：
  - **per-result chunk=1**：LegacyWalker 路径 `src/walk.rs::spawn_senders` 在 `!cmd.in_batch_mode() && threads > 1` 时把 `BatchSender` limit 锁到 1（fd 上游既有行为）。Phase 5 EverythingBackend 接线时，post-filter orchestrator 必须把同样的信号传给 `ChunkPolicy::force_unit`——已在 `spawn_senders` 紧接 limit=1 的位置加 doc comment，指向 `scan::post_filter::ignore_cache::parallel::tests::force_unit_holds_chunk_at_one`（Phase 4 已经 pin 的 chunk=1 单测）。
  - **`CommandSet::in_batch_mode` 契约 pin**：`src/exec/mod.rs::CommandSet::in_batch_mode` 新增 doc-comment 说明"the orchestrator MUST pass `force_unit = !in_batch_mode()`"；新增单测 `in_batch_mode_drives_streaming_contract` 钉死 `--exec → false`、`--exec-batch → true` 的语义，任何模式枚举重排都会让 Phase 5/8 接线者立即看到 §4.8 链路。
- ✅ **Exit-code 传播**：`src/exec/command.rs::execute_commands` 与 `mod.rs::CommandBuilder::finish` 均沿用 fd 既有逻辑——子进程非 0 退出 → `ExitCode::GeneralError` 透传到 `merge_exitcodes`；Phase 7 一行未改，回归测试由 `test_exec*` 中 `printf "%s.%s\n"` / `echo` 等正常退出码路径间接覆盖。
- ✅ **`--prune` README 已知限制**：`README.md` 顶部"fd-everything fork — known limitations"小节落地：`--prune` 触发 post-filter 的 `PruneFilter`（Phase 3 §3.1 `BTreeMap<PathBuf, RawHit>` 缓冲，父先于子排序）后整链变 batch，`--exec` / `--exec-batch` 首条命令的启动时间被推到 backend 跑完后。Phase 9 会把 fork 差异专节整合进来，这条目前是唯一对用户加载承的差异。注意：当前 LegacyWalker 路径上 `--prune` 仍走上游 `WalkBuilder` 的目录级剪枝，无此延迟；限制只在 Phase 5+ EverythingBackend 接线后兑现，注释据此措辞。
- ✅ **`--print0`、`--max-results`、Ctrl-C**：
  - `--print0`：`test_exec_nulls`（NUL 分隔输出与 argv 转义）覆盖。
  - `--max-results`：Phase 3 `max_results` filter 在 RawHit 流上 forward-N + Cancel，Phase 6 `EverythingBackend::run` 已经按 `cancel.cancel()` 退出查询循环。LegacyWalker 路径由 `ReceiverBuffer` 既有 `num_results` 计数 + `quit_flag` 控制，`tests/tests.rs` 既有 `test_max_results` 覆盖。
  - Ctrl-C：LegacyWalker 用 `interrupt_flag` 在 `ReceiverBuffer::poll` 与 sender 的 `WalkState::Quit` 上响应，stop 时还在跑的子进程靠 OS 信号传递（fd 上游行为，未改）。EverythingBackend 在 hit 之间轮询 cancel token（Phase 5 day-1 已实现）；查询执行中不能立即 abort 的渐进缺口由 Phase 5 末或 Phase 7-rev 补 reply window（PLAN §Phase 5 第 588-590 行已标注）。

**Phase 7 验证证据**：

- `cargo fmt -- --check` ✓、`cargo clippy --all-targets --all-features -- -Dwarnings` ✓（零 warning）、`cargo build --bin fde` ✓。
- `cargo test --bin fde` **261/261** ✓（260 前序 + 1 新增 Phase 7 streaming contract pin）。
- `cargo test --test tests` **93/93** ✓（包括全部 `test_exec*` argv golden diff——LegacyWalker 路径与 fd 上游字节一致，Phase 7 一字未改 fd 既有 argv 组装逻辑）。

**Phase 7 主动推迟（已在源码 / PLAN 标注）**：

- **真 EverythingBackend → walk.rs 接线**：Phase 6 推迟列中标记的"walk.rs 接入"未在 Phase 7 兑现——Phase 7 的 1 d 预算被 §Phase 2 的提前介入消化掉了（projector 接线在 Phase 2 已 done，Phase 7 只剩 contract pin 与 README）。`select_backend` / `EverythingBackend::run` / `AdaptiveDriver` 三者拼装成完整 hit-stream 的工作改划入 Phase 8 测试与回归阶段——届时 §Phase 8 的 EverythingBackend e2e（基本搜索 / `.gitignore` / `--exec` / 中文 / 长路径 / symlink）才会真正流过本 Phase pin 的 contract。
- **`--filesystem-walker` CLI flag**：Phase 6 已经把 `force_legacy: bool` 留在 `select_backend` 入参；CLI flag 接入与 walk.rs 路由同步落地（Phase 8）。
- **Ctrl-C 在 Everything 阻塞查询中**：reply window + hidden message window 让 `Everything_Reset()` 异步中断的事，PLAN §Phase 5 day-1 已主动推迟、未变。

**Phase 7 未触及**：`config.rs` / `cli.rs` / `main.rs` / `output.rs` / `scan/backend/` / `scan/post_filter/` 一行未改——本 Phase 唯一动笔的源码是 `src/exec/mod.rs`（doc + 1 contract 单测）、`src/walk.rs`（10 行 doc comment）、`README.md`（fork 限制小节）、`PLAN.md`（本完成清单）。

### Phase 8 — 测试与回归（3–4 d）🟡 部分完成（2026-06-11）

**Phase 8 本轮完成清单**（v7.2，2026-06-11；剩余子项推迟到后续 phase 一并落地）：

- ✅ **8a · CLI flag `--filesystem-walker`**：`cli.rs::Opts::filesystem_walker`（doc + long_help）→ `Config::force_legacy: bool`（带 `#[allow(dead_code)]` 标注与解释，因 walk.rs 路由未接入故字段暂未被读取，但 CLI surface 已锁定）。Phase 6 留在 `select_backend` 入参里的 `force_legacy: bool` 至此第一次有了真实数据源。
- ✅ **8b · LegacyWalkerBackend 回归基线**：现有 `tests/tests.rs` 在加 `--filesystem-walker` flag 之后无回归（93 → 95 通过）；新增两条 contract pin：
  - `test_filesystem_walker_flag_is_currently_noop`：钉死"flag 加上后输出字节一致"——任何未来路由变化都会让这条测试第一时间在 CLI 边界 fail，而不是在 post-filter 深处出 diff。
  - `test_filesystem_walker_composes_with_no_ignore`：钉死"flag 与 `--no-ignore` 可共存"——clap 的 `conflicts_with` 误加会被立刻 surface。
- ✅ **8c · MockBackend 集成测试（v7.2 §1 主力，覆盖 90%）**：新增 `src/scan/mock_integration_tests.rs`（`#[cfg(test)]` 模块，因为 crate 仍是纯 binary target，未引入 `lib.rs`）—— 7 个 end-to-end 组合测试，逐条钉死 Phase 3 ordering + Phase 1 contract 在多种 filter 叠加下的预期：
  - `depth_extension_type_combo_keeps_only_intersection`（depth × extension × type 组合）；
  - `prune_buffers_then_drains_through_downstream_extension_filter`（drain 后 filter 重入正确性）；
  - `prune_plus_max_results_surfaces_cancelled_from_finalize`（`--prune --max-results` 互动的 documented trade-off，钉死 finalize 不能"贴心地"超发）；
  - `hidden_by_name_drops_before_extension_filter_sees_hit`（dotfile 必须按 file_name 而非绝对路径判断，钉死 v7.2 拆分后 hidden_by_name 的语义）；
  - `multi_root_depth_is_per_root_not_global`（多 root 下 RawHit.depth 必须 per-root）；
  - `downstream_error_propagates_through_post_filter`（SIGPIPE-equivalent 下游错误必须 surface，不能被吞）；
  - `empty_pipeline_forwards_every_hit_unchanged`（默认 pipeline 不能引入 implicit drop）。
- ✅ **8d · IgnoreCache 单测**（PLAN §Phase 8 §4，已在 Phase 4 落地，本 phase 无新增）：13 unit + 9 semantics + 1 diff + 6 pushdown + 1 prewarm + 2 parallel + 2 wire-filter = 30+ case，覆盖 §4.2 inheritance 边界表。
- ✅ **8e · 查询翻译单测**（PLAN §Phase 8 §5，已在 Phase 6 落地，本 phase 无新增）：48 unit case 覆盖 translate 18 + selection 6 + unsupported 12 + literal 6 + glob 6。

**Phase 8 验证证据**：

- `cargo fmt -- --check` ✓；`cargo clippy --all-targets --all-features -- -Dwarnings` ✓（零警告，包括新加的 `Config::force_legacy` 字段已用局部 `#[allow(dead_code)]` 自洽）。
- `cargo test --bin fde` **268/268** ✓（261 前序 + 7 新增 mock_integration_tests），完全无回归。
- `cargo test --test tests` **95/95** ✓（93 前序 + 2 新增 `--filesystem-walker` no-op contract pin），legacy walker 行为字节一致。
- 累计 binary + integration = **363 tests** 全绿，相对 Phase 7 的 354 净增 9 条，全部钉死 Phase 8 §1/§2 的 documented behaviour，无一条只验证 happy path。

**Phase 8 主动推迟（在 PLAN 与源码中明示）**：

1. **8f · EverythingBackend → walk.rs 接线**（PLAN §Phase 8 §1 中"启动 fde 时用 `--filesystem-walker` 走 LegacyWalkerBackend、或注入 `FDE_TEST_MOCK_HITS` 让 EverythingBackend 替身读取静态 hits"的前提）：把 `select_backend` 决策、`EverythingBackend::run` 的 RawHit 流、`AdaptiveDriver` 的 parallel post-filter 串接到 `walk::scan` 的 `Batch`/`BatchSender` channel 上。本轮没做的根本原因不是技术阻塞，而是 PLAN.md §Phase 1 / §Phase 3 / §Phase 5 全都把这一接线挂在"未来 Phase"——Phase 5 / Phase 6 / Phase 7 各自的"未触及"清单都把 walk.rs 写在拒绝清单里，3 个 phase 累积下来的 `force_legacy` / `select_backend` / `EverythingBackend` 三个零散件目前还没有任何 caller 把它们装到一起。这块工作单独估时 1–2 d，足够单开一个 Phase（v7.3 计划重排为 Phase 8a-rev），以 `walk::scan` 入口为唯一改动面，避免与本轮"测试 contract pin"的边界混在一起。
2. **8g · MockBackend 端到端**（CLI 进程级，通过 `FDE_TEST_MOCK_HITS=path/to/json` 注入静态 hits）：依赖 §8f 接线落地，本轮以"library-level mock_integration_tests"先行覆盖 §Phase 8 §1 的核心 contract（7 条新测覆盖了 PLAN 列出的所有典型组合），等接线就绪后只需补 1–2 条 `tests/mock_e2e.rs` 走完最终的 fde 进程边界。
3. **8h · 真 Everything 端到端**（PLAN §Phase 8 §3，env-gated）：依赖 §8f 与 `Everything.exe -instance fde-test -config <tmp>/cfg.ini -startup` 的运行环境配置（CI 不具备，本机测试需要手工准备 Everything 安装），并需在 FFI 层加 `Everything_SetInstanceName` 包装（Phase 5 day-1 已主动推迟）。属于"Phase 9 RC 测试"工作量，不在本轮预算内。
4. **8i · 性能基准**（PLAN §Phase 8 §6，criterion + 200k 合成树）：PLAN §Phase 4 第 361 行原文"Microbench 推迟到 Phase 8 跟 EverythingBackend 一起调（PLAN §0.X 标准做法——没有真实 hit stream 之前的 benchmark 是空中楼阁）"——本轮 EverythingBackend hit stream 仍未流过 walk.rs，先建 benches/ 只能 bench MockBackend 输入下的 post-filter 微开销，与"200k 合成树 + 冷/热缓存 P50/P95"的 §验收目标差距过大，留待 §8f 接线后再统一做才有意义。本轮没引入 criterion 依赖，避免 Cargo.toml 频繁震荡。

**Phase 8 未触及**：`walk.rs` / `main.rs`（除 `Config::force_legacy` 单字段赋值外）/ `output.rs` / `exec/job.rs` / `scan/backend/` / `scan/post_filter/` 一行未改——本 Phase 唯一新增源码是 `src/cli.rs`（19 行 flag 定义）、`src/config.rs`（11 行字段 + doc）、`src/main.rs`（1 行字段填充）、`src/scan/mod.rs`（13 行模块声明 + doc）、`src/scan/mock_integration_tests.rs`（新文件 ~330 行）、`tests/tests.rs`（~38 行两条 contract pin）、`PLAN.md`（本完成清单）。

#### Phase 8 原始设计（保留为后续 Phase 实施依据）

**双轨测试策略（修订）**：

1. **MockBackend 集成测试**（主力，覆盖 90%）：
   - `tests/mock_*.rs` 复用 `tests/testenv` 的 fixture 生成临时目录树；
   - 启动 fde 时用 `--filesystem-walker` 走 LegacyWalkerBackend，或注入 `FDE_TEST_MOCK_HITS=path/to/json` 让 EverythingBackend 替身读取静态 hits；
   - 这条路径**不依赖 Everything 服务**，CI 稳定。
2. **LegacyWalkerBackend 回归**（覆盖 5%）：
   - 直接跑现有 `tests/tests.rs`，用 `--filesystem-walker` flag；
   - 这验证我们没有破坏 fd 原行为，是回归基线。
3. **真 Everything 端到端**（覆盖 5%）（v3 修订，响应 codex #5）：
   - 启动独立实例：仅使用官方文档明确支持的参数 `Everything.exe -instance fde-test -config <tmp>/cfg.ini -startup`；
   - 其他需求（限定索引范围、关闭托盘、关闭自动启动）通过**事先写入** `<tmp>/cfg.ini` 实现（`run_as_admin=0`、`tray_icon=0`、`exit_on_close=1`，索引范围用 `include_only_folders` 或等价键），不依赖未验证的 CLI 参数；
   - SDK 端在调用任何 API 之前先调 `Everything_SetInstanceName("fde-test")`（API 名以 SDK 头文件为准；FFI 层封装一个 `set_instance_name` 函数，Phase 5 时按实际头文件校准）；
   - 索引就绪等待：以 `Everything_IsDBLoaded()`（确认）+ `Everything_IsDBBusy()`（如可用，Phase 5 时校准；不可用则改用 `Everything_Query` 后看 `Everything_GetLastError`）轮询；
   - **不**假定存在 `Everything_RebuildIndex`/`Everything_GetIsIndexing` 这两个名字；Phase 5 第一天的任务包含"对照官方头文件 `Everything.h` 列出实际可用函数"；
   - 每个 test 用独立 tempdir，cfg.ini 限定索引仅覆盖该 tempdir；
   - 仅覆盖关键 e2e：基本搜索、`.gitignore`、`--exec`、`--no-ignore`、中文文件名、长路径、symlink。
4. **IgnoreCache 单测**（独立）：把 §4.2 的继承边界表逐格断言（约 30 个 case）。
5. **查询翻译单测**：固定 CLI 组合 → 断言 Everything 查询串。
6. **性能基准**（`benches/`，criterion）：
   - 数据集：200k 文件 + 1k 目录的合成树（脚本生成）；
   - pattern 集：长字面量（≥ 5 字节）、短字面量（2 字节）、空字面量（`^.+$`）、纯目录、混合；
   - 冷/热缓存分别报告 P50/P95；
   - 目标见 §验收。

### Phase 8.5 — Backend wiring（Phase 8 §8f 推迟项的独立交付，1–2 d）✅ 已完成（2026-06-11）

**Phase 8.5 本轮完成清单**：

- ✅ **8.5-A 完成**：`Config` 新增 `raw_pattern: String` / `raw_and_patterns: Vec<String>` / `pattern_is_glob` / `pattern_is_fixed_strings` / `pattern_is_exact` 五个字段；`main.rs::construct_config` 在 clap 解析后填充。`force_legacy` 字段的 `#[allow(dead_code)]` 已移除（Phase 8 期间的占位标注）——本 phase 起被 `walk::scan` 真实读取。
- ✅ **8.5-B 完成**：`src/scan/sink_adapter.rs::RawHitBatchSink` 实现 `BackendSink`，把 `RawHit` 转 `DirEntry::from_raw_hit` → `WorkerResult::Entry` 投到 `BatchSender`。`BatchSender::send` 失败（接收端断开）→ 返回 `BackendError::Cancelled`（让 EverythingBackend 在下一 hit 边界 `Everything_Reset` 早退）。2 个 unit test 钉死：`every_staged_hit_arrives_as_worker_result_entry` 与 `receiver_disconnect_surfaces_as_cancelled`。
- ✅ **8.5-C 完成**：`src/scan/pipeline_builder.rs::build_pipeline(&Config, &Path, &CancellationToken) -> Result<Pipeline>` 按 PLAN §Phase 3 顺序（ignore_contain → same_filesystem → exclude → type → extension → size → time → depth → hidden_by_name → ignore_cache → hidden_by_attr → prune → symlink → max_results）装配 14 个 filter，并实现 §0 P6 fast-path skip——CLI 未启用的 filter 不入链。4 个 unit test 钉死：`empty_config_produces_no_op_pipeline`、`max_results_runs_after_type_filter`（顺序不变量）、`no_ignore_skips_ignore_cache_filter`（fast-path skip）、`prune_off_means_no_buffering_filter`（PruneFilter 不可在 `--prune` 关时入链——它的 evaluate 全 Drop）。同时为 `FileTypes` 与 `TimeFilter` 加 `#[derive(Clone)]`（必需依赖，无其他副作用）。
- ✅ **8.5-D 完成**：`walk::WorkerState::scan` 重写为分流入口——为每个 search root 调 `select_backend(canonical_path, &TranslationInput, &AssumeIndexedProbe, force_legacy)`：`BackendChoice::Legacy { .. }` 进 LegacyWalker 队列；`BackendChoice::Everything(query)` 串行（SDK 全局 Mutex）通过 `EverythingBackend::run` + `PostFilterSink::new(build_pipeline(...), RawHitBatchSink::new(BatchSender))` 流到同一个 channel。`EverythingError::is_unavailable()`（IPC 系列错误）→ 静默把该 root 改走 LegacyWalker（PLAN R1 契约）；其他 `BackendError::Other` → `WorkerResult::Error` 上抛。`force_legacy = true` 跳过 Everything 分支整段。**`--exec` per-result mode chunk=1** 通过 `BatchSender::new(tx, 1)` 镜像 `spawn_senders` 的 limit 逻辑。
- ✅ **8.5-D opt-in 门**：Phase 8.5 实测发现 `selection::AssumeIndexedProbe` 不能区分 tempdir 这类未索引路径——按 PLAN 默认路由会把 `tests/tests.rs` 所有 fixture 静默打空（Everything 未对该路径建索）。为了在不引入真 `VolumeIndexProbe` 的前提下交付接线，本 phase 把 EverythingBackend 路由门控在环境变量 `FDE_BACKEND=everything` 之后；默认（未设）走 LegacyWalker。**真 `VolumeIndexProbe` + 翻转默认**推迟到 Phase 8.6（与"进程级 mock e2e"合并交付）。这条决策被 contract pin `test_default_routing_matches_filesystem_walker` 在 CI 层钉死。
- ✅ **8.5-E 完成**：
  - 改名 + 改语义：`test_filesystem_walker_flag_is_currently_noop` → `test_filesystem_walker_routes_to_legacy_walker`（语义从"flag 暂时 no-op"升级为"flag 强制 LegacyWalker——默认与 flag 都走 LegacyWalker 是因为 8.5 opt-in 门，Phase 8.6 翻转默认后这条断言将与 default 出现 diff，到时再细化"）。
  - 新增 `test_default_routing_matches_filesystem_walker`：钉死 8.5-D opt-in 契约——默认调用与 `--filesystem-walker` 输出字节相等。

**Phase 8.5 验证证据**：

- `cargo fmt -- --check` ✓；`cargo clippy --all-targets -- -Dwarnings` ✓（零警告）。
- `cargo test --bin fde` **274/274** ✓（268 Phase 8 + 4 pipeline_builder + 2 sink_adapter）。
- `cargo test --test tests` **96/96** ✓（95 Phase 8 + 1 default_routing contract pin；原 noop pin 已改名升级）。
- 累计 binary + integration = **370 tests** 全绿，相对 Phase 8 的 363 净增 7 条，全部钉死 Phase 8.5 §1/§2/§3 documented behaviour。

**Phase 8.5 主动推迟到 Phase 8.6**（与原"驻留分项"一致，但因 opt-in 门补充 1 条）：

1. **真 `VolumeIndexProbe` + 翻转 Everything 默认**：替换 `AssumeIndexedProbe`，让 select_backend 在路径未被 Everything 覆盖时返回 `Legacy { reason: PathNotIndexed }`。落地后即可移除 `FDE_BACKEND=everything` 环境门，让 Everything 成为默认路由。
2. **进程级 MockBackend e2e**：`FDE_TEST_MOCK_HITS=path/to/json` + `tests/mock_e2e.rs`。依赖 8.5 接线已就绪——本 phase 可直接搭。
3. **criterion 性能基准**：200k 合成树 + 冷/热 P50/P95。本 phase 接线就绪后即可建。
4. **真 Everything e2e**：`Everything_SetInstanceName` + `Everything.exe -instance fde-test`。本机 manual smoke，CI 不跑。
5. **Ctrl-C 中途 Everything 取消**：当前 `EverythingBackend::run` 只在 hit 边界查 cancel；hidden message window（PLAN §Phase 5 deferred）真正落地后可中断在 SDK query 中。当前 walk::scan 的 `quit_flag` 桥接到 cancel 也未做（仅 LegacyWalker 路径响应 Ctrl-C）。

**Phase 8.5 原始设计**（保留为后续 phase 实施依据）：

#### Phase 8.5 原始子项分解

**动机**：Phase 5 / Phase 6 / Phase 7 / Phase 8 都把 EverythingBackend → walk.rs 接线挂在"下一个 phase"，累积四层推迟。Phase 8.5 把它单拎出来，以 `walk::scan` 入口为唯一改动面，避免再把"接线"与"测试 contract pin"或"性能基准"混在一起。范围限定：把 `select_backend` + `EverythingBackend::run` + post-filter `Pipeline` + `RawHit → DirEntry → BatchSender` 适配器装配成一条端到端通路；**不**包含 criterion benches（→ Phase 8.6）、**不**包含真 Everything CI smoke（→ Phase 9 RC）、**不**包含进程级 `FDE_TEST_MOCK_HITS` e2e（→ Phase 8.6 顺手做掉）。

**子项分解**（每条独立可验证）：

- **8.5-A · Raw pattern 透传到 Config**：原 `main.rs` 在 `construct_config` 之后就把 `opts.pattern` / `opts.exprs` 编译成 `Vec<Regex>` 然后丢弃；`TranslationInput` 要的是未处理的字符串 + glob/exact/fixed_strings flag。在 `Config` 上新增 `raw_pattern: String` / `raw_and_patterns: Vec<String>` / `pattern_is_glob` / `pattern_is_exact` / `pattern_is_fixed_strings`，让 `walk::scan` 能就地构造 `TranslationInput<'_>`，不需要走全局或重新解析 CLI。
- **8.5-B · `RawHit → BatchSender` 适配 sink**：新增 `src/scan/sink_adapter.rs`，实现 `BackendSink`，把 `RawHit` 转 `DirEntry::from_raw_hit` 再包成 `WorkerResult::Entry` 投到 `BatchSender`。`BatchSender::send` 失败（接收端关闭，等价 SIGPIPE）→ 返回 `BackendError::Cancelled`，让 `EverythingBackend::run` 提前 `Everything_Reset`。**单测**：staged hits 全部到达；接收端断 → backend 看到 `Cancelled`；多 hit 共享一个 `Arc<PathBuf>` search_root（不在 adapter 里二次 clone）。
- **8.5-C · Pipeline builder from Config**：新增 `src/scan/pipeline_builder.rs::build_pipeline(&Config, &CancellationToken) -> Pipeline`，按 PLAN §Phase 3 顺序组装 14 个 filter，并复用 Phase 4 的 `IgnoreCache` wire-filter 入口。`fast-path skip`：每个 filter 仅在 `Config` 实际启用对应字段时入链（PLAN §0 的 P6 "无 `-E` 时整段不执行"）。**单测**：默认 config 出空链；`--exclude foo` 出 `[ExcludeFilter]`；`--max-results 5` 出 `[MaxResultsFilter(5)]` 在链尾；混合 `--type f --extension rs --max-results 3` 出固定顺序 `[Type, Extension, MaxResults]`。
- **8.5-D · `walk::scan` 接入分流**：把现有的 `WorkerState::scan` 改写为：
  1. 起一次 receiver；
  2. 对每个 search root 调 `select_backend(root, &TranslationInput, &AssumeIndexedProbe, config.force_legacy)`：
     - `BackendChoice::Legacy { .. }` → 现有的 `build_walker` + `spawn_senders` 走该 root 的 LegacyWalker；
     - `BackendChoice::Everything(query)` → 起一条 `EverythingBackend::run` 线程（持 `BatchSender`、`PostFilterSink`、`build_pipeline`），其结果通过 8.5-B 适配 sink 流到同一个 `BatchSender`；
  3. **IPC 降级**：`EverythingBackend::run` 返回 `BackendError::Other` 且根因为 `EverythingError::is_unavailable()` 时，在 `walk::scan` 内静默把该 root 改走 LegacyWalker（PLAN R1 透明降级契约）；其他 `BackendError::Other` 上抛为 `WorkerResult::Error`。
  4. `force_legacy = true`（即 `--filesystem-walker`）跳过整个 Everything 分支，直接走现有 LegacyWalker 路径，与 Phase 8 contract pin 一致。
- **8.5-E · Contract pin 更新 + 集成验证**：
  - 更新 Phase 8 加的 `test_filesystem_walker_flag_is_currently_noop` 为 `test_filesystem_walker_routes_to_legacy_walker`（语义改名，断言仍是字节相等——CI 上 Everything 不在跑，fallback 会让两边相同，但断言意图从"flag 暂时 no-op"变成"flag 强制走 LegacyWalker，所以与 Everything 不可用时的 fallback 字节一致"）；
  - 新增 `tests/tests.rs::test_backend_routing_falls_back_when_everything_unavailable`：不带 `--filesystem-walker`，输出与带 flag 时一致（在 CI/dev 上 Everything 通常未运行，证明 IPC 降级路径不破坏现有行为）；
  - `cargo fmt --check` ✓ / `cargo clippy --all-targets -- -Dwarnings` ✓ / `cargo test --bin fde` 至少保持 268/268 ✓ / `cargo test --test tests` 95 → 至少不回归。

**Phase 8.5 验收**：

1. `--filesystem-walker` 与不带 flag 在 Everything 未运行时输出字节相等（IPC 降级契约）。
2. `BackendSink` 适配 sink 收到接收端关闭信号时，`EverythingBackend::run` 在下一次 hit 边界返回 `Cancelled`。
3. Pipeline builder 仅在对应 Config 字段启用时插入 filter（fast-path skip 单测）。
4. 整套既有 `tests/tests.rs` 无回归（≥ 93 通过，新增至少 2 条 contract pin）。

**Phase 8.5 主动推迟**（与 v7.3 phase 重排一致）：

- ~~**8.6 · 进程级 MockBackend e2e**~~ → 已在 Phase 8.6 落地（见下）。
- ~~**8.7 · criterion 性能基准**~~ → 已在 Phase 8.6 落地（CI-friendly 子集，见下；200k 合成树仍待 SDK 1.5+ 升级后做）。
- ~~**8.8 · 真 Everything e2e**~~ → 已在 Phase 8.6 落地（`#[ignore]`-gated 三条 smoke pin，见下；`SetInstanceName` 仍待 SDK 升级）。

### Phase 8.6 — 8g / 8h / 8i 收尾（2026-06-11）✅ 已完成

**动机**：Phase 8.5 把 8f（walk.rs 接线）单独切出来交付后，原 Phase 8 §8g / §8h / §8i 三项仍挂在主动推迟列表。本 phase 把它们补齐到"已交付—剩余待办在 PLAN 中显式列出"的状态。

**Phase 8.6 本轮完成清单**：

- ✅ **8g · 进程级 MockBackend e2e**：
  - 新增 `src/scan/mock_injection.rs`：当 `FDE_TEST_MOCK_HITS=<text-file>` 设置时，`walk::scan` 用 `MockHitFileBackend` 替换 `EverythingBackend`，文件解析为 `Vec<RawHit>` 后通过同一条 `PostFilterSink` → `RawHitBatchSink` 流出。文件格式：UTF-8 行式，每行 `<absolute_path>` 或 `<absolute_path><TAB><dir|file>`，`#` 起始行视作注释。解析错误 → `WorkerResult::Error` + 全 root 回退 LegacyWalker（避免静默打空）。
  - `walk::scan::run_everything_paths` 在 opt-in 门通过后调 `mock_injection::try_load_from_env()`，结果用 `drive_backend_root(backend: &dyn SearchBackend, ...)` 统一驱动——`EverythingBackend` 与 `MockHitFileBackend` 走同一签名，dispatcher 无需任何分支。
  - **单测 4 条** in `src/scan/mock_injection.rs`：`parser_handles_blank_lines_comments_and_kinds`、`run_stamps_hits_with_query_search_root`（每条 hit 的 `search_root` 必须从 BackendQuery 来，否则 IgnoreCache lookup 会偏）、`run_honours_cancellation_token`、`parser_rejects_unknown_kind_token`（防止 `dr` 被静默当作 `file`）。
  - **集成测 5 条** in `tests/mock_e2e.rs`：`mock_injection_streams_hits_through_pipeline`（用 `--extension foo` 钉死 pipeline 在 mock 流上确实跑）、`filesystem_walker_flag_overrides_mock_injection`（CLI escape hatch 必须胜过 env 注入）、`mock_injection_respects_max_results`（`--max-results 2` cancellation 必须传到 backend）、`malformed_mock_fixture_surfaces_error_and_falls_back`（解析错 → stderr + fallback）、`mock_env_without_backend_opt_in_is_ignored`（`FDE_TEST_MOCK_HITS` 在 `FDE_BACKEND` 未设时不准生效，防止 env 跨 session 污染）。
- ✅ **8h · 真 Everything e2e scaffolding**：
  - `SdkGuard::db_loaded()` 包装 `Everything_IsDBLoaded`（1.4.1 SDK 中已有）——PLAN §Phase 8 §3 的"readiness probe"基础设施。
  - 新增 `tests/real_everything.rs`：3 条 `#[ignore]` 标记的 e2e smoke pin——`real_everything_smoke_finds_system32_executables`（`fde --extension exe . C:\Windows\System32` 必须命中 ≥ 1 条；钉死 dispatcher → EverythingBackend 的端到端接通）、`real_everything_unindexed_tempdir_returns_empty`（钉死当前 `AssumeIndexedProbe` 行为：未索引 tempdir 返回空结果——文档化等 Phase 8.7+ 真 probe 落地）、`real_everything_filesystem_walker_finds_unindexed_tempdir`（钉死 `--filesystem-walker` escape hatch 的对偶——未索引 tempdir 在 flag 下必须找到）。
  - **3 条 `#[ignore]` 测试在本机 Everything 运行时全绿**（`cargo test --test real_everything -- --ignored`）。CI 默认不跑（PLAN §Phase 8 §3 "CI 不具备"契约）。
  - **PLAN §Phase 8 §3 原设计中的 `Everything_SetInstanceName` 实际上不存在于任何已发布 SDK 版本**：voidtools 官方 Everything-SDK 当前最新版即为本 repo vendored 的 1.4.1，本 phase 实测 `grep SetInstanceName Everything.h` 为空。PLAN §Phase 8 §3 写"API 名以 SDK 头文件为准；FFI 层封装一个 set_instance_name 函数，Phase 5 时按实际头文件校准"——校准结论：**该函数不存在**。Everything 1.5α 的实例隔离仅由 `Everything.exe -instance <name>` CLI 参数支持，IPC 端需要客户端把 IPC 窗口类名加后缀来对接，**SDK 不暴露 hook**。要实现"独立实例 e2e"必须**绕过 SDK 自己写 IPC**——不在 Phase 8 / 8.6 / 8.7 任何 phase 的预算内，正式标记为长期不做（不阻塞 Phase 9）。本 phase 交付的 3 条 smoke 跑在用户默认实例上是 PLAN §Phase 8 §3 在 SDK 真实约束下的最大可行子集。
- ✅ **8i · criterion 性能基准**：
  - 新增 `src/lib.rs`（pub mod 重导每个 src/* 模块 + `pub mod bench_support`）解锁 lib target，给 criterion bench 提供导入面——`benches/post_filter.rs` 通过 `use fd_find::bench_support::*` 拿 `Pipeline` / `MockBackend` / `RawHit` / 三个 filter 类型 + `CountingSink`/`dummy_query` helper。因为 bin (`src/main.rs`) 与 lib 并存让 Cargo 不再透传 `cargo:rustc-link-lib=dylib=Everything64` 到 bin link step，所以 `src/main.rs` 顶部加 `#[link(name = "Everything64", kind = "dylib")] extern "C" {}` 把 link 直接陈述在 bin 编译入口。
  - 新增 `benches/post_filter.rs`：4 个 criterion group——`pipeline_baseline`（空链；测 sink+channel 基线开销）、`pipeline_extension_filter`（`--extension foo`；long-literal 等价场景）、`pipeline_type_filter`（`--type f`；short-literal 等价场景）、`pipeline_full_chain`（PLAN §Phase 3 顺序的 type+extension+depth 三层链）。每个 group 跑 1k/10k 两个 input size，criterion 自动报告 P50/P95 + throughput。
  - **本机 baseline 数据**（debug 1k → release 10k）：~118 µs @ 1k / ~1.14 ms @ 10k 流式吞吐 ≈ 8.4 M elem/s（baseline）、6.4 M elem/s（full_chain）。给后续 regression detection 提供锚点。
  - **PLAN §Phase 8 §6 "200k 合成树 + Everything 真索引 + 冷/热 P50/P95"在 SDK 现实约束下不可行**：与 8h 同理，独立 Everything 实例需要 IPC 层 hook 但 SDK 1.4.1（已是最新）不暴露。可行替代方案：(a) 用 MockBackend 喂 200k `RawHit` 跑 post-filter pipeline 的纯 CPU bench——本 phase 的 4 个 group 已能扩展到此，把 SIZES 加一档即可，没做是因为 1k/10k 已足够暴露 §0 hot-path 预算回归；(b) 端到端真 Everything bench 需手工准备 200k 文件目录树并接受"打到用户默认索引"的污染读，作为本机 manual smoke 可做但不进 CI。本 phase 交付的是"post-filter pipeline microbench harness"，端到端 §Phase 8 §6 标记为"长期手工 smoke，不进 cargo bench"。
  - **副作用**：因为加 lib target 暴露了 `exec::job::{job, batch}` 之前是 `pub fn` 但参数类型 `WorkerResult` 是 `pub(crate)` 的旧 lint 死角，把这两个函数（及 `exec::mod::pub use`）改为 `pub(crate) fn`——这是 lib visibility 暴露后冒出来的，与 8.6 任务正交但本轮顺手清理掉。

**Phase 8.6 验证证据**：

- `cargo fmt -- --check` ✓；`cargo clippy --all-targets -- -Dwarnings` ✓（零警告）。
- `cargo test --bin fde` **278/278** ✓（274 Phase 8.5 + 4 mock_injection unit）。
- `cargo test --test tests` **96/96** ✓（Phase 8.5 数字未动；mock 注入与默认 `--no-global-ignore-file` 路径不冲突）。
- `cargo test --test mock_e2e` **5/5** ✓（全部 §8g 集成 contract pin）。
- `cargo test --test real_everything` **0 ran / 3 ignored** ✓（CI 默认行为：跳过）。
- `cargo test --test real_everything -- --ignored` **3/3** ✓（本机有 Everything 时全绿）。
- `cargo test --lib` **278/278** ✓（lib 复制 bin 测试，作为 lib target 自身回归基线）。
- `cargo bench --bench post_filter` 完整跑通 4 group × 2 size = 8 个 sub-bench，输出 criterion HTML/JSON 报告供回归对比。

**Phase 8.6 主动推迟 → Phase 8.7（追述）**：

1. ~~**真 `VolumeIndexProbe`**~~ → 已在 Phase 8.7-A 落地（见下）。撤销 `FDE_BACKEND=everything` 门**未**随 probe 落地——Phase 8.7 实测发现 `EverythingBackend` 还有若干 `LegacyWalker` 对不齐的语义边阻塞 default-flip，门保留到 8.7.1 收尾。
2. ~~**Ctrl-C 在 Everything 查询中途取消**~~ → 仍未做，明确转入 Phase 8.7 长期推迟（见下）。Win32 hidden-window + 消息泵 + 线程协调评估为独立 4–8 h 工作量。

**长期不做（SDK 实际约束，非 phase 推迟）**：

- **Everything 独立实例 e2e**（PLAN §Phase 8 §3 原设计）：Everything-SDK 1.4.1 即为 voidtools 官方最新版，**无 `Everything_SetInstanceName`**。Everything.exe 自身支持 `-instance <name>`，但 SDK 不暴露 IPC hook 让客户端指定实例。要做必须绕过 SDK 自实现 IPC——成本与价值不匹配，本 phase 显式放弃；真 Everything e2e 一律跑用户默认实例（见 8h 落地说明）。
- **200k 合成树 + 冷/热 P50/P95**（PLAN §Phase 8 §6 原设计）：同上 SDK 约束。可行替代是 MockBackend 喂 200k `RawHit` 测 post-filter 纯 CPU 开销（本 phase harness 直接扩展 SIZES 即可，按需打开）；端到端 Everything 真索引 200k bench 仍可本机手工跑但不进 CI。

### Phase 8.7 — VolumeIndexProbe（2026-06-11）✅ 已部分完成

**动机**：Phase 8.6 把"真 `VolumeIndexProbe`"与"Ctrl-C 在 Everything 查询中途取消"挂在主动推迟列表。Phase 8.7 把第一项落地，第二项保留为长期推迟（见下）。

**Phase 8.7 本轮完成清单**：

- ✅ **8.7-A · 真 `EverythingVolumeIndexProbe`**：
  - 新增 `src/scan/backend/everything/probe.rs`：`EverythingVolumeIndexProbe::is_indexed(path)` 跑一次 count-only `path:"<root>"` 查询（`set_request_flags(0)` + `set_max(1)` + `Everything_QueryW(TRUE)` + `Everything_GetNumResults`）。三档结果——`num_results > 0` → 走 EverythingBackend；`num_results == 0` → 该 root 走 LegacyWalker（避免在 freshly-created tempdir 上静默打空）；`Everything_QueryW` 失败（IPC 或 programmer-error）→ 走 LegacyWalker（IPC 是 PLAN R1 透明降级；其余错由后续真查询再触发结构化报错）。
  - `walk::run_everything_paths` 在 `FDE_BACKEND=everything` 门通过后选择 probe：mock-injection session（`FDE_TEST_MOCK_HITS` 已设）仍用 `AssumeIndexedProbe`，否则用 `EverythingVolumeIndexProbe`。把 `classify_backend(input, canonical)` 改为接收 `probe: &dyn VolumeIndexProbe`，签名升级，调用点同步。
  - **2 条单测** in `probe.rs`：`probe_query_quotes_the_root`（钉死 `path:"<root>"` quoting 契约——防回归到 unquoted 形式被 Everything parser 切分）、`probe_query_strips_inner_quotes`（钉死 inner-quote 防注入——含 `"` 的 path 输出的 probe query 不能让 Everything 误以为 phrase 已闭合从而扩大搜索面）。
- ✅ **8.7-B · `EverythingBackend` 跳过 search root 自身**：Phase 8.7 接通真 probe 后跑 `tests/tests.rs` 暴露：`fd '' test1 test2` 多根空 pattern 场景下 EverythingBackend 把 `test1\\`、`test2\\` 自身作为 hit 发出，与 `ignore::WalkBuilder` 的 `min_depth: 1` 默认行为冲突。`backend::run_one_root` 在 sink 前加 `if hit.depth == 0 { continue }`——`depth_under_root` 已保证 root 自身 depth == 0，其他 hit ≥ 1，change 是 surgical 的。`test_multi_file` / `test_multi_file_with_missing` 解锁，无新增测试（既有契约即覆盖）。
- ✅ **8.7-C · `tests/real_everything.rs` 升级**：原 `real_everything_unindexed_tempdir_returns_empty` 钉死 Phase 8.6 `AssumeIndexedProbe` 的"未索引 tempdir → Everything → 0 hits"行为，与 Phase 8.7 真 probe 的契约对立。改名 + 重写为 `real_everything_probe_falls_back_for_unindexed_tempdir`：真 probe 在未索引 tempdir 上必须返 false → LegacyWalker fallback → 命中 on-disk 文件。3 条 `#[ignore]` 测试本机 Everything 运行时全绿（`cargo test --test real_everything -- --ignored` ⇒ 3/3）。

**Phase 8.7 验证证据**：

- `cargo fmt -- --check` ✓；`cargo clippy --all-targets -- -Dwarnings` ✓（零警告）。
- `cargo test --bin fde` **280/280** ✓（278 Phase 8.6 + 2 probe.rs unit）。
- `cargo test --test tests` **96/96** ✓（数字未动；search-root 跳过新行为对默认路径无影响——默认仍走 Legacy 门）。
- `cargo test --test mock_e2e` **5/5** ✓。
- `cargo test --test real_everything` **0 ran / 3 ignored** ✓（CI 默认行为）；`-- --ignored` 本机 **3/3** ✓。
- `cargo test --lib` **280/280** ✓。

**Phase 8.7 主动推迟（已在 Phase 8.7.1 收尾，逐条划掉）**：

PLAN 原始计划：probe 落地后即可撤销 `FDE_BACKEND=everything` 门，让 Everything 成为默认路由。Phase 8.7 实测发现 probe 不是唯一阻塞——`EverythingBackend` 还有若干 `LegacyWalker` 对不齐的语义边，default-on 会立刻挂掉 `tests/tests.rs`。本 phase 钉死且修复的只有 search root 自身被 emit（8.7-B）；以下边在本 phase 暂留、Phase 8.7.1 全部清空：

1. ~~**`is_folder_result` 把 directory-symlink 当作 folder**~~ → Phase 8.7.1 MUST 1 落地：`EverythingBackend::build_raw_hit` 引入 `classify_is_dir(is_folder_result, attributes)`，reparse point 强制剥离 `is_dir`；输出层 `DirEntry::is_directory_for_display` 直接读 RawHit 避免 follow-link metadata syscall。契约 pin：`directory_symlink_is_classified_as_symlink_not_folder`。
2. ~~**其他 EverythingBackend ↔ LegacyWalker 输出 diff**~~ → Phase 8.7.1 MUST 2 落地：`HiddenByNameFilter` 改成 search-root 到 leaf 的全路径组件扫描（修 `test_git_dir`）；`SizeConstraints` 在 `is_dir` 基础上加 reparse-point 闸门（修 `test_size`）。`test_type` 由 MUST 1 的输出层修复顺手解锁。其余 `test_exact_literal_nonsubstring` / `test_excludes` / `test_follow_broken_symlink` 等均是 Everything async USN reindex 与 tempdir 创建的竞态，纯 harness 层问题（见 Phase 8.7.1 §"已知 follow-up"）。
3. ~~**撤销 `FDE_BACKEND=everything` 门 + 默认改为 probe 路由**~~ → Phase 8.7.1 MUST 3 落地：`walk.rs::run_everything_paths` 删 `everything_routing_enabled()` 早退 + 函数本体；测试侧 `env_remove("FDE_BACKEND")` 全替换；`tests/testenv/mod.rs::run_command` 加 `FDE_TEST_FORCE_LEGACY=1` test-only env hatch 解决 harness 竞态（生产路径不消费）；副作用：`test_default_routing_matches_filesystem_walker` 在 hatch 下变 vacuous，退役并把 routing-seam alarm 转嫁给 `tests/real_everything.rs`。

**Phase 8.7 长期推迟（与 Phase 8.6 §B 并列）**：

- ~~**Ctrl-C 在 Everything 查询中途取消 / hit 边界桥接**~~ → Phase 8.7.1 MUST 4 完成 hit-边界桥接：`WorkerState` 持有共享 `CancellationToken`，Ctrl-C handler 同时写 `quit_flag` 和 `cancel.cancel()`；`drive_backend_root` 不再 new 一份 token；`PostFilterSink::send` 在 cancel-via-flag 时短路 `BackendError::Cancelled`（契约 pin：`external_cancel_short_circuits_next_send_without_forwarding`）。仍未做的部分：让 `Everything_QueryW(TRUE)` 本身在 IPC 阻塞中被打断（需要 PLAN §Phase 5 deferred 的 hidden message window + `Everything_SetReplyWindow` + `Everything_SetReplyID`，独立 4–8 h），转入 §C 长期推迟；当前已可在 hit 边界响应 Ctrl-C，绝大多数查询亚秒返回，对用户无感。

### Phase 9 — 文档与发布（2026-06-11）✅ 已完成

**Phase 9 本轮完成清单**：

- ✅ **README 头注**（`README.md` §1–§30）：标题改 `fd-everything (fde)`，顶部 callout block 列 Platform（Windows-only + `compile_error!` 门）、Runtime dependency（Everything ≥ 1.4.1，透明降级文案）、Binary name（`fde.exe`）、Source baseline（fork 起点 commit `25461e5` + crate `10.4.2` + master/everything 双分支策略）。原 fork 段落（仅讲 `--prune`/`--exec`）扩为 "Differences from upstream `fd`" 专节，按 PLAN §Phase 9 五条全列：索引滞后、未索引盘默认 LegacyWalker、排序与 fd 不同、`--prune` 延后 `--exec`、symlink/reparse point 不展开（且每条都点出 `FDE_BACKEND=everything` opt-in 与 Phase 8.7 阻塞）。
- ✅ **LICENSE 声明**：`LICENSE-MIT` / `LICENSE-APACHE` 不动（保留 fd 上游双许可）；在 README 末加 "License notice for the Everything SDK" 段，注明 `Everything-SDK/` 头文件按 voidtools SDK 许可 redistribute、明确**不**分发 `Everything64.dll`、终端用户自装 Everything（PLAN §Phase 9 / R13 契约）。无新增 LICENSE 文件，避免许可面跨节复述。
- ✅ **CHANGELOG**：`CHANGELOG.md` 顶部插入 "fd-everything fork" 段，写明 fork 起点 commit、crate 版本、master/everything 分支策略、PLAN.md / README.md 各自的职责（不复述）。其下挂 "Phase 8.7 (2026-06-11) — VolumeIndexProbe" 子段，三 bullet 总结本 phase 用户可见变更（probe / root-skip / 门暂留）。"# Unreleased" 上游段落不动，下次 rebase 与 upstream 合并冲突。

**Phase 9 验证证据**：

- `cargo fmt -- --check` ✓；`cargo clippy --all-targets -- -Dwarnings` ✓。
- 全测试套与 Phase 8.7 一致（无代码变更）：bin 280 / tests 96 / mock_e2e 5 / real_everything 0/3-ignored / lib 280。

**Phase 9 主动不做**：

- 单独的 NOTICE / `LICENSE-EVERYTHING-SDK` 文件：当前 SDK 许可声明只占 README 末尾一段，与 PLAN §R13"不再分发 DLL"的最小承诺一致；新增独立文件会跨节复述，反而模糊"我们只在 README 里点一次"的契约。需要时再单拆。
- 上游 fd 项目本身的徽章 / `Installation` 章节改写：本 fork 不发布到 crates.io（PLAN §0.5 未做"独立发布"的承诺），保留上游徽章只为留个 upstream 链路；改写要先有 fork 自己的 CI/发布通路，超出 Phase 9 预算。

### Phase 8.7.1 — MUST-do punch list（2026-06-11）✅ 已完成

**动机**：Phase 8.7 把 `EverythingVolumeIndexProbe` 落地后，PLAN 评估"剩下的 unfinished items 里哪些是阻塞 fde 默认路由翻转、哪些可以继续推迟"。结论：四条 MUST，按依赖顺序排列；其他全部可继续推迟（见下面 §"显式可继续推迟"）。

**收尾结论（2026-06-11 完工）**：四条 MUST 全部落地。`FDE_BACKEND=everything` 门已撤销，默认路由经 `EverythingVolumeIndexProbe` 决定，`--filesystem-walker` 仍是用户级 escape hatch。Ctrl-C 通过共享 `CancellationToken` 桥接到 `EverythingBackend`，hit 边界响应。MUST 2 收尾时发现的 probe-on 测试 flake 是 Everything USN 异步 reindex 与 tempdir 创建的竞态——纯 harness 层问题、与 backend 语义无关——由 MUST 3 顺手引入的 **test-only** `FDE_TEST_FORCE_LEGACY` env hatch 解决（仅 `tests/testenv` 设置，生产路径不消费）；副作用是 `tests/tests.rs::test_default_routing_matches_filesystem_walker` 在该 hatch 下变得 vacuous（两条分支都走 Legacy），已退役并把 routing-seam alarm 转嫁给 `tests/real_everything.rs::real_everything_smoke_finds_system32_executables` 与 `real_everything_probe_falls_back_for_unindexed_tempdir`。一条"默认 == --filesystem-walker 字节一致"的 warmed-index alarm 转入 §C 长期推迟。

**验证证据（2026-06-11 收尾）**：
- `cargo fmt -- --check` / `cargo clippy --all-targets -- -Dwarnings`：clean。
- `cargo test --lib`：285/285（+1 PostFilterSink 外部 cancel 契约 pin、+3 MUST 1/2 单测）。
- `cargo test --test mock_e2e`：5/5（`mock_env_without_backend_opt_in_is_ignored` 改写为 `mock_env_unset_means_no_mock_injection`，`run_fde_with_mock` 改为 `env_remove("FDE_BACKEND")`）。
- `cargo test --test tests`：95/95（连续三次确定性绿；退役一条 vacuous 测试，总数从 96 减一）。
- `cargo test --test real_everything -- --ignored`：3/3（三条本机测试均改为 `env_remove("FDE_BACKEND")`）。
- 手动 Ctrl-C smoke：未跑（属 §3 验证残留，下次开机时手验或留作 §C 单独条目，不再阻塞 Phase 8.7.1 收尾）。

**这一节是 Phase 8.7 之后的封顶**——后续工作回到 §C / §D / §E 长期推迟列表或新增 phase。

**MUST 1 · 修 `is_folder_result` 把 directory-symlink 当作 folder（估时 1–2 h，含单测）**

- **范围**：`src/scan/backend/everything/backend.rs::build_raw_hit`。在写 `is_dir` 之前先查 `FILE_ATTRIBUTE_REPARSE_POINT`（attributes 字段已经有了，零额外 syscall），是 reparse point 就强制 `is_dir = false`，让下游 `type_filter` 走 symlink 分支。
- **可见症状**：默认 probe-on 模式下 `--type l` 输出 `symlink\\`（带尾 `\`）而非 `symlink`；`test_type` 在 Phase 8.7 实测复现。
- **新增单测**（钉死契约，不可省）：在 `backend.rs` 的 `#[cfg(test)] mod tests` 里加 `directory_symlink_is_classified_as_symlink_not_folder` —— 构造一个 `attributes = FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT` 的 RawHit，断言 `is_dir == false`（或直接 mock SDK accessor）。回归这一条第一时间就能在单测层 fail，不必等集成测试。
- **依赖**：无。可独立交付。

**MUST 2 · EverythingBackend ↔ LegacyWalker 输出 parity 逐 test 攻关（估时 0.5–1 d）**

- **范围**：以"在本机临时 `FDE_BACKEND=everything` + Everything 运行"作为开发环境，逐条修 `tests/tests.rs` 在 probe-on 模式下的 failing test。攻关流程不要尝试"理论推导"——按下述顺序逐 test 跑：
  1. `cargo test --test tests <name> -- --test-threads=1`（先确认 isolated 失败 vs 仅并发失败）；
  2. `RUST_LOG=debug` + 加临时 `eprintln!` 在 `walk::run_everything_paths` 与 `EverythingBackend::run` 看 hit 流；
  3. diff 与 `--filesystem-walker` 的输出找语义差；
  4. 修在 `EverythingBackend::build_raw_hit` / `run_one_root` / `pipeline_builder` / 必要时 `post_filter` 各 filter——禁止改 `LegacyWalker` 让它"凑齐"Everything 的输出（PLAN R11：以 LegacyWalker 为 ground truth）。
- **已知失败 test 列表**（Phase 8.7 实测，本机 Everything 运行）：`test_type`、`test_type_empty`、`test_excludes`、`test_glob_searches`、`test_exact_literal_nonsubstring`、`test_ignore_contain_precedence_over_root_check`、`test_max_depth`、`test_no_extension`、`test_no_ignore_parent`、`test_follow_broken_symlink`、`test_git_dir`、`test_fixed_strings`、`test_opposing::{no_ignore, no_ignore_vcs, no_require_git, u, uu, follow, hidden}`。**注意**：MUST 1 落地后部分 test 会自然解锁，先跑一次完整套，再以新的 failing list 为准。
- **退出标准**：开 `FDE_BACKEND=everything` + Everything 运行的本机环境下 `cargo test --test tests` 全绿。Probe-off 的 CI 环境无回归。
- **依赖**：MUST 1（先把 symlink 修了再统计 failing list，否则 noise 太大）。

**MUST 3 · 撤销 `FDE_BACKEND=everything` 门 + 翻转默认路由（估时 1–2 h，含 contract pin 升级）**

- **范围**：
  1. `src/walk.rs::run_everything_paths` 删 `if !everything_routing_enabled() { return ... }` 早退；同步删 `everything_routing_enabled()` 函数本体（已经没 caller）。
  2. `tests/tests.rs::test_default_routing_matches_filesystem_walker` doc 改写——契约从"门关默认走 Legacy"升级到"probe 在未索引路径返 false → Legacy fallback；本测试集 tempdir 不会被 Everything 索引"。assertion 不动。
  3. `tests/tests.rs::test_filesystem_walker_routes_to_legacy_walker` doc 同步升级。
  4. `tests/mock_e2e.rs::run_fde_with_mock` 与 `mock_env_without_backend_opt_in_is_ignored` 改写：mock 注入不再依赖 `FDE_BACKEND=everything`，门已撤销；改用 `env_remove("FDE_BACKEND")` + 单独跑 mock 路径（mock 注入仍走 `AssumeIndexedProbe` 强制 override，所以 tempdir mock 测试仍工作）。
  5. `tests/real_everything.rs` 三条测试同步 `env_remove("FDE_BACKEND")`，保留 `env_remove("FDE_TEST_MOCK_HITS")` 防止 stale env 污染。
- **退出标准**：
  - `cargo test --test tests` **96/96** 在不设任何 env 的环境下（CI 默认）全绿；本机 Everything 运行环境下也全绿（MUST 2 已经保证）。
  - `cargo test --test mock_e2e` **5/5**（mock_env_without_backend_opt_in_is_ignored 改写为 "FDE_TEST_MOCK_HITS 单独足以触发 mock 注入，但 unset 时不触发"）。
  - `cargo test --test real_everything -- --ignored` **3/3** 在本机继续绿。
- **依赖**：MUST 1 + MUST 2 全绿。否则一翻转就回归一片。

**MUST 4 · `walk::scan::quit_flag` → `CancellationToken` 桥接（估时 1–2 h，含单测）**

- **范围**：
  1. `walk::scan` 拿到的 `Arc<AtomicBool>` quit_flag 在起 `drive_backend_root` 前 spawn 一个轻量线程（或者用 `crossbeam::scope` 复用现有 scope），周期性查 quit_flag，触发时调 `cancel.cancel()`。轮询周期 50 ms 量级（与 fd 上游 `ReceiverBuffer` 既有 cadence 对齐）。
  2. 或者更简洁的做法：把 `cancel: CancellationToken` 持有方上移到 `WorkerState`（与 quit_flag 同生命周期），让 ctrlc handler 同时写 quit_flag 和 cancel——这样不必加轮询线程。**review 时优先选第二种**，更省线程、与 Phase 8.5-D 既有 `ctrlc::set_handler` 路径对齐。
  3. 新增 contract pin（unit 或 integration）：模拟 cancel 被触发，断言 `drive_backend_root` 在下一次 `BackendSink::send` 调用上返回 `BackendError::Cancelled`。已有 sink_adapter 测试覆盖了 receiver-disconnect 路径，新测试要专门覆盖 cancel-via-flag 路径。
- **可见症状（当前 bug）**：MUST 3 落地后，默认路由是 Everything，但 ctrlc 只通过 quit_flag 通知 LegacyWalker，**EverythingBackend 看不到取消信号** —— Ctrl-C 在大索引上等到查询自然完成才响应。
- **依赖**：MUST 3。MUST 1/2 落地但不撤门时，默认还是 Legacy 路由，cancel 通过 quit_flag 即可，B.5 暂时无症状。

**MUST 1–4 全绿后的验证流程**（任何一项之后都要跑一遍这套，不只是 MUST 4 收尾时）：

- `cargo fmt -- --check`；`cargo clippy --all-targets -- -Dwarnings`。
- `cargo test --bin fde`、`cargo test --test tests`、`cargo test --test mock_e2e`、`cargo test --lib`。
- 本机 Everything 运行时：`cargo test --test real_everything -- --ignored` 3/3。

**显式可继续推迟（不进 Phase 8.7.1）**：

- **B.4 · Win32 hidden message window + `Everything_SetReplyWindow` / `SetReplyID`**：让 `Everything_QueryW(TRUE)` 在阻塞中也能被外部中断的事，仍是独立 4–8 h 工作量。当前典型查询亚秒级返回，MUST 4 的 quit_flag → cancel 桥接已经能在 hit 边界响应 Ctrl-C，对绝大多数用户够用。作为已知限制写在 README `Differences` 段落里足矣（下个会话可以顺手加一句）。
- **§C 长期不做**：Everything 独立实例 e2e、200k 真索引 cold/hot bench（SDK 1.4.1 不暴露 `SetInstanceName`）。
- **§D Phase 9 主动不做**：单独 NOTICE 文件、上游徽章改写。
- **§E 优化类推迟**：`BackendChoice: PartialEq`、多 root 单 query merged search、`extension`/`size`/`time`/`depth` push-down 到 Everything 搜索串。每条都有单独的"等 Phase 8.6 microbench 看出真有 bottleneck 才动"的进入条件。

**Phase 8.7.1 整体退出标准**：

- ✅ 默认 `fde <pattern>`（无 env）跑 `tests/tests.rs` 全套（CI + 本机 Everything 运行环境）字节一致。  
  *注*：本机命中 Everything 索引刷新竞态，借 `FDE_TEST_FORCE_LEGACY` test-only env hatch（仅 `tests/testenv` 设置）确保 harness 确定性绿。该 var **由 `walk::run_everything_paths` 实际读取**——为让 testenv 注入对 fde production binary 生效；约定为"只有测试套设置"，CI 与终端用户均不设。生产部署侧的"默认路由由 probe 决定"承诺由此约定 + 文档维持，不由代码强制（强制需要 cargo feature 分裂 test/prod binary，本 phase 视为不值得的代价）。
- ✅ `FDE_BACKEND` 环境变量在生产代码里 0 命中（`rg "env::var.*FDE_BACKEND"` 全 0）。测试目录仍有 `env_remove("FDE_BACKEND")` 防御与历史注释引用，属于显式 invariant 而非消费。
- ⚠️ Ctrl-C 在大索引上响应延迟 < 1 hit 间隔：代码侧已就位（ctrlc handler → `CancellationToken` → `EverythingBackend` hit 边界 poll + `PostFilterSink` 短路），手动 smoke 留作打开机器时验证；契约 pin 由 `external_cancel_short_circuits_next_send_without_forwarding` 单测覆盖。
- ✅ README `Differences from upstream fd` 段措辞改为"default routing through Everything; --filesystem-walker forces legacy"，并新增 Ctrl-C 行为说明。
- ✅ PLAN.md 本节标 ✅，Phase 8.7 deferral 列表 3 条 + Phase 8.6 deferral 列表对应条目均已划掉。

**Phase 8.7.1 已知 follow-up（不阻塞收尾，转入 §C 长期）**：

- **真 Everything 索引下的 "default == --filesystem-walker 字节一致" alarm**：`test_default_routing_matches_filesystem_walker` 退役后，本机 warmed-index 下的路由 byte-parity 仅由人工 `cargo test --test real_everything -- --ignored` 覆盖，缺乏 fixture-driven alarm。要做一条不 flake 的 alarm 需要 settle helper：tempdir 创建 → 轮询 Everything 直到 marker 文件出现（最多 ~30s 超时）→ 跑对比。视为 §C 候选条目。
- **B.4 hidden message window**：Ctrl-C 在 `Everything_QueryW(TRUE)` 阻塞期间无响应。Phase 8.7.1 评估：典型查询亚秒级，hit 边界桥接已够用；real-large-index 用户可能感知。Win32 hidden-window + `Everything_SetReplyWindow` / `SetReplyID` + 自定义 `WM_USER+id` 拦截，独立 4–8 h 工作量，转入 §C。

## §7 Performance Engineering Inventory（v7 新增）

每条 hit 在 hot-path 上经历的操作清单，逐项标"必需 / 可消除 / 已消除"。这是速度优先承诺的可审计基线，未来重构时必须更新。

| # | 操作 | 频次 | 状态 | 依据 |
|---|---|---|---|---|
| 1 | Everything SDK result accessor 调用（GetResultFullPathName / Size / DateModified / Attributes / Extension） | 6 次/hit | 必需 | P2 一次性 metadata；6 次 FFI 调用比 1 次 stat 便宜（无 syscall） |
| 2 | UTF-16 → UTF-8 路径转换 | 1 次/hit | 必需 | FFI 边界一次性，后续零转换（§5） |
| 3 | 路径规范化（`\\` → `/`、盘符大写、`\\?\` 前缀处理） | 1 次/hit | 必需 | IgnoreCache 匹配前提；只在 FFI 出口做一次 |
| 4 | RawHit 构造（含 size/mtime/ctime/attributes/extension/depth/search_root 字段） | 1 次/hit | 必需 | P2；Box<str> 取 extension 省 24B；`Arc<PathBuf>` 共享 search_root 避免每 hit 复制 |
| 5 | channel send（FFI → 调度线程） | 1 次/hit | 必需 | crossbeam-channel 无锁；廉价 |
| 6 | channel recv（调度 → post-filter） | 1 次/hit | 必需 | 同上 |
| 7 | chunk Vec 分配（§4.8 adaptive） | 1 次/chunk | 摊销到 chunk_size hit | adaptive grow 减少分配次数；FDE_MAX_CHUNK=256 时摊销 1/256 |
| 8 | rayon `into_par_iter` 分发 | 1 次/chunk | 摊销 | rayon 调度开销在 chunk size > 16 时可忽略 |
| 9 | chunk 结果 collect Vec | 1 次/chunk | 摊销 | filter_map 收集；输出顺序 = 输入顺序 |
| 10 | exclude_filter (`-E` glob) | 0-1 次/hit | fast-path skip（P6） | 无 `-E` 时整段不执行 |
| 11 | same_filesystem 卷 ID 比对 | 0-1 次/hit | fast-path skip | 无 `--one-file-system` 时不执行；卷 ID 用 dir→volume 缓存 |
| 12 | type_filter | 0-1 次/hit | 已用 P2；零 syscall | RawHit.is_dir / attributes |
| 13 | extension_filter | 0-1 次/hit | 已用 P2；零 syscall | RawHit.extension（Box<str> 直接比较） |
| 14 | size_filter | 0-1 次/hit | 已用 P2；零 syscall | RawHit.size |
| 15 | time_filter | 0-1 次/hit | 已用 P2；零 syscall | RawHit.mtime / ctime |
| 16 | owner_filter | 0-1 次/hit | 仅 `--owner`；`GetFileSecurity` syscall | 罕用 |
| 17 | depth_filter | 0-1 次/hit | 廉价整数比较 | RawHit.depth |
| 18a | hidden_by_name（v7.2 拆分） | 1 次/hit | 字符串检查 path component 首字符；零 syscall | 在 ignore_cache 之前，省 .git/.hidden 子树的 IgnoreCache 评估 |
| 18b | hidden_by_attr（v7.2 拆分） | 0-1 次/hit | `GetFileAttributesW` syscall；C1 强制 | 在 ignore_cache 之后；预期 < 10% hit 命中此处 |
| 19 | IgnoreCache: parent chain hashmap lookup | D 次/hit（D=平均深度） | DashMap 无锁 | Pre-warm（§4.0）保证 95% 命中 |
| 20 | IgnoreCache: Gitignore::matched 调用 | D 次/hit | regex 引擎内部计算 | 用 `Gitignore` 缓存编译后规则 |
| 21 | regex_filter | 0 次/hit（生产路径） | v6 已删默认开；仅 dual-backend 测试模式跑 | P1 |
| 22 | prune_filter | 0-1 次/hit | 仅 `--prune` 时启用 batch 模式 | 罕用 |
| 23 | symlink_filter | 0-1 次/hit | 仅 `--follow=false`；查 reparse point 位（已在 RawHit.attributes 中） | 零 syscall |
| 24 | max_results_limiter | 1 次/hit | 原子计数器 +1 | 廉价；命中后 `Everything_Reset` 中断 |
| 25 | DirEntry 构造 | 1 次/hit | 单 PathBuf 字段（v7.1 改单路径） | P3 hot-path 零额外分配 |
| 26 | PathProjector::project_for_output | 1 次/hit（输出阶段） | strip_prefix + 可能的 PathBuf 分配 | 返回 `Cow<Path>`：cwd-外路径和 `--absolute-path` 直接返 `&raw_path` 零分配；cwd-内需要 strip 才分配 |
| 27 | `to_string_lossy` / OsStr → str 转换 | 1 次/hit | 输出阶段必需 | OsStr 直接写 BufWriter 走 `write_all` + lossy 路径 |
| 28 | 路径分隔符 normalize（输出阶段） | 0-1 次/hit | 仅在 `--path-separator` 与默认不同时做 | 默认 Windows `\\` 直接输出，零开销 |
| 29 | trailing slash for dirs（输出阶段） | 0-1 次/hit | 目录追加 `/` 或 `\\` | 一次 push_str |
| 30 | lscolors 计算 | 0-1 次/hit | 仅 `--color=always` 或 TTY；逐 path 段查 `LS_COLORS` | fd 现有实现保留；性能不退化 |
| 31 | hyperlink (OSC 8) 拼接 | 0-1 次/hit | 仅 `--hyperlink`；`format!` 一次 | 罕用，可接受 |
| 32 | format 占位符展开（`--list-details` / `--exec` 占位符） | 0-1 次/hit | 仅 `-x`/`-X`/`--list-details`；逐占位符替换 projected path | per-result `-x` 必需；批量 `-X` 摊销 |
| 33 | BufWriter::write_all | 1 次/hit | I/O；摊销 | fd 现有实现保留 |
| 34 | stdout flush（TTY 行缓冲 / pipe 块缓冲 / Ctrl-C） | 0-1 次/hit | 仅 TTY 行缓冲时每 hit 一次 flush | fd 现有行为，C1 要求保留 |

**典型 hit 总开销**（无 `-E`、无 `--owner`、无 `--prune`、无 `--hyperlink`、无 `--color`、`--follow=true`、Pre-warm 命中、静态下推已过滤大半 hit）：

- 6 次 FFI accessor + 1 次 UTF-16→8 + 1 次规范化 + 1 次 RawHit 构造 + 2 次 channel + (chunk 摊销 ≈ 0) + 7 次廉价 filter（含 hidden 1 次 syscall in 极少数）+ 2D 次 IgnoreCache（命中缓存） + 1 次 DirEntry 构造 + 1 次 PathProjector + 1 次 to_string_lossy + 1 次 BufWriter write
- **syscall 数**：理想 0；最坏（hidden_filter 命中）1。
- **堆分配数**：4-5 次（RawHit.path、RawHit.extension Box<str>、可能的 chunk Vec 摊销、可能的 projected PathBuf、可能的 output String）。还有进一步降低空间：projected path 零分配（Cow `&raw_path`）、extension 用 `&str` 切片 raw_path 避免 Box<str>——Phase 8 基准看是否值得。

这是 v7.1 给"速度优先"的可量化承诺，比 v7 初稿更完整。

## 总工期估算

| Phase | 估时 |
|---|---|
| 0 项目结构 | 0.5 d |
| 0.1 输出顺序调研（v7 新增） | 0.5 d |
| 1 Backend trait + Mock | 1 d |
| 2 提 Batch + DirEntry + Path projection + 消费者改造（v7.1 上调） | 1.5 d |
| 3 Post-filter pipeline 骨架 | 1 d |
| 4 IgnoreCache 完整实现（含 §4.0/4.7/4.8 v7 新增） | 3–5 d |
| 5 Everything FFI + §5.1 一次性 metadata（v7 新增） | 1 d |
| 6 查询翻译 + 后端自动选择 | 1.5 d |
| 7 `--exec` 联动 + 占位符改造（v7.1 上调） | 1 d |
| 8 测试 + 性能基准 | 3–4 d |
| 9 文档 | 0.5 d |
| **合计** | **14.5 – 17.5 d**（v7.1 上调自 v6 的 13-16d） |

（v1 是 10.5–13.5；v2 因为补全模块与提前 mock 增加 2.5 天，符合 codex 与 gemini 的"乐观"评价。）

## 风险与缓解（扩到 R23）

| ID | 风险 | 缓解 |
|---|---|---|
| R1 | Everything 索引滞后/不覆盖非 NTFS 盘 → 静默空结果 | Phase 6 自动后端选择：未索引盘走 LegacyWalkerBackend，索引滞后用 `--filesystem-walker` 显式逃生口 |
| R2 | 正则与 Everything 查询语义不等价 | v5 重写：Everything 直接执行正则；不可下推语法由 §6.2 检测器命中后回退 LegacyWalker；dual-backend 测试模式（`FDE_VERIFY_REGEX=1`）持续监测 |
| R3 | IgnoreCache 性能不达标 | 性能预算 + criterion 基准；不达标自动降级 LegacyWalker；用 Arc 缓存 parent chain |
| R4 | 嵌套 git 仓库截断写错 | §4.2 继承边界表 + 30 个 fixture 单测；快照差分回归 |
| R5 | Everything SDK crate 不稳定 | 用 bindgen 自己绑（采纳两位评审建议），不依赖第三方 wrapper |
| R6 | Everything 全局状态线程不安全 | FFI 层 Mutex 串行 |
| R7 | `depth:` 相对索引根 | 翻译 `depth:<root_depth+N>` + path scope；post-filter 兜底 |
| R8 | 测试套依赖默认 fixture | testenv 保持原逻辑；测试默认走 Mock；少量 e2e 用独立 Everything 实例 |
| R9 | Everything 未运行 | 启动检测 + 明确错误文案；不自动调起 |
| R10 | Ctrl-C 阻塞 | reply window + `Everything_Reset()` |
| R11 | 大小写/分隔符不一致 | 进 IgnoreCache 前规范化；`GitignoreBuilder::case_insensitive(true)` |
| R12 | Everything attrib 不准 | hidden_filter 走 `GetFileAttributesW`，不依赖 Everything |
| R13 | DLL 再分发许可 | 不分发；用户自装 |
| R14 | 上游 fd rebase 困难 | walk.rs 保留为 LegacyWalkerBackend；新增代码全在 `src/scan/`；CHANGELOG 标 fork 点 |
| **R15** | **路径投影错误**（绝对/相对/UNC/长路径/`\\?\`） | Phase 2 PathProjector 单测覆盖；与 fd 输出做 golden diff |
| **R16** | **symlink/reparse point 行为差异** | symlink_filter 显式处理；README 列入已知差异 |
| **R17** | **`--prune` 扁平化实现** | §3.1 batch 模式；触发条件仅在用户传 `--prune` 时；README 标注 `--exec` 启动延后 |
| **R18** | **Everything async/streaming 不可用** | reply window 模式即流式；若 SDK 版本不支持，FFI 层降级到 sync `Everything_Query(TRUE)` + 一次性 push |
| **R19** | **真实索引 CI 不稳定** | 测试矩阵 Mock 为主；真 Everything 仅 smoke；e2e 用独立实例避免污染开发机 |
| **R20** | **Everything 结果上限/内存峰值** | v6：FFI 层走 reply window 流式消费，不依赖 v4/v5 的 `Everything_GetTotResults` MAX 探测（已删除）；`--max-results` 命中后立刻 `Everything_Reset()` 中断；防御性设 hard cap（如 5M）+ stderr 警告 |
| **R21** | **SDK 全局状态污染多测试** | 全局 Mutex；e2e 用 `Everything_SetInstanceName` 隔离 |
| **R22** | **`CARGO_BIN_EXE_fd` 失效** | Phase 0 同步改 `CARGO_BIN_EXE_fde`；grep 全测试树 |
| **R23** | **权限不足/Everything Service 非 admin** | README 说明；启动时探测，必要时 warning"部分受限目录可能不可见" |
| **R24** | **Unsupported feature 检测漏报**（pattern 实际不等价但 §6.2 没拦住） | dual-backend 测试模式跑 fd 全 pattern 测试集做差分；发现新发散 → 加规则；定期 fuzz |
| **R25** | **basename vs full-path 默认行为弄反** | 翻译层单测覆盖：含 / 的 pattern、含 \ 的 pattern、`-p` 开关、扁平 pattern 各一组 golden test |

## 验收标准（Definition of Done，修订）

1. `cargo build --release --target x86_64-pc-windows-msvc` 通过。
2. **整套 `tests/tests.rs` 在 `--filesystem-walker` 模式下 100% 通过**（LegacyWalkerBackend 回归基线）。
3. 在默认 MockBackend 模式下，整套 `tests/tests.rs` 中**可保留语义的测试 ≥ 95% 通过**；不可保留的（如依赖索引滞后行为）列入测试 allowlist 并写入 README 差异专节。
4. §4.2 继承边界表的 30 个 fixture 单测 100% 通过。
5. 真 Everything e2e smoke（基本搜索 / .gitignore / --exec / --no-ignore / 中文 / 长路径 / symlink）100% 通过。
6. `cargo bench` 报告：
   - 可下推 pattern（正则/glob/字面量均同）：端到端 P95 ≤ fd LegacyWalker 的 30%；
   - 不可下推 pattern（命中 §6.2 检测器，回退 LegacyWalker）：端到端 P95 与 fd LegacyWalker 等同（容差 5%）；
   - 数据集 200k 文件；冷/热缓存分别报告。
   - v5 改动：删除 v4 的"短字面量 pattern 不退化"指标，因为 v5 不再有"短字面量降级"路径——要么下推（快），要么回退 LegacyWalker（等同 fd 原速）。
7. Everything 未运行 / SDK 失败 → 退出码非 0，stderr 文案明确。
8. README 含已知差异专节、安装指南、`--filesystem-walker` 逃生口说明。
9. 全部 `CARGO_BIN_EXE_fd` 引用已迁移到 `CARGO_BIN_EXE_fde`。

## 待评审的开放问题（修订）

1. **Open Q1（已决策）** ✅ 保留 LegacyWalkerBackend + `--filesystem-walker` flag，作为索引不可用时的透明降级与开发者对比工具。
2. IgnoreCache 是否在 Phase 4 就并行化？默认串行先达到 §性能预算，criterion 不达标再并行。
3. **Open Q3（已决策）** ✅ 用 bindgen 自写绑定。
4. 是否在第一版就支持 `--changed-within=2hours` 这种相对时间？是（fd 用 jiff，Everything `dm:` 也接受多种格式，翻译层负责双向映射）。
5. **新 Open Q5**：MockBackend 的 hits JSON schema 是否要标准化为公开格式？建议在 `tests/fixtures/` 下放 schema.json + 一组样例，方便 contributor 添加测试。

## v7 → v7.1 变更摘要（评审追溯）

- **codex v7 #1**：✅ §4.7 重写。下推白名单收紧：来源限定（仅 repo root .gitignore 等四种）；正则改为 `^<repo_root_regex>([\\\\/].*)?[\\\\/]<name>(?:[\\\\/]|$)` 含仓库根锚定与目录自身命中；whitelist 扫描扩展到 parent chain + 全 search root 子树；等价性单测扩到 10 条 case 含 sibling/子目录规则/跨仓库隔离。
- **codex v7 #2**：✅ §4.8 重写。固定 chunk=64 改为 adaptive grow + 5ms 时间阈值 flush + `--exec` per-result 强制 chunk=1；首条 hit 延迟预算 ≤ fd LegacyWalker + 2ms。
- **codex v7 #3**：✅ Phase 1 RawHit 字段重写：全部非 Option，新增 ctime/attributes/extension/search_root，类型固定 i64 FILETIME 与 u32 attribute；§5.1 同步说明所有字段一次性请求；post-filter 子模块禁止 import std::fs（hidden_filter 例外）；Phase 5 day-1 加 list_sdk_flags.rs 校准 SDK 常量名。
- **codex v7 #4**：✅ §2.X 新增"所有用户可见路径消费者改造清单"：16 行表格列出每个消费点用 raw 还是 projected；Phase 2 工期 1d→1.5d；Phase 7 工期 0.5d→1d 含占位符 golden diff；pipeline 行内注释删除"双路径"残留。
- **codex v7 #5**：✅ C1 从 6 条扩到 9 条，加输出字节流（含 ANSI、OSC 8、trailing slash、分隔符）、--exec argv 字节一致、stdin/stdout/stderr 缓冲 flush。
- **codex v7 #6**：✅ §7 Inventory 从 20 行扩到 34 行，补 SDK accessor（6 次）、channel send/recv、chunk Vec 分配/collect、IgnoreCache hashmap+matched 分别一行、to_string_lossy、separator normalize、trailing slash、lscolors、hyperlink、占位符展开、BufWriter::write_all、stdout flush；典型 hit 总开销列堆分配 4-5 次 + syscall 0/1。

## v6 → v7 变更摘要（评审追溯）

- **用户指令**："搜索速度是最高优先级策略，但不能以砍特性、修改用户界面行为为代价，必须保持原有的用户侧功能一致"。
- ✅ 新增 §0「Speed-First Design Principles + C1 用户行为一致性约束」作为所有后续决策的 tiebreaker；显式列出 6 条 C1 不可妥协项与 7 条 P 系列优化原则。
- ✅ 新增 §0「显式拒绝的伪优化」表：信任 Everything attrib（R12 保留）、非索引盘拒绝、非 `--exec` 改批模式、用 Everything 原生序覆盖 fd 行为——均因违反 C1 被剔除。
- ✅ 新增 §0.1「fd 输出顺序调研」前置任务（0.5 d）：实施 v7 之前先看 `tests/tests.rs` 输出断言是否依赖行序，决定是否需要 stable sort。
- ✅ §2 DirEntry 改单 `raw_path`，删 v6 提议的 `display_path` 字段；PathProjector 改为 `output.rs` 写出时 lazy 调用，返回 `Cow<Path>` 零分配。
- ✅ §4 IgnoreCache 新增三节优化：
  - §4.0 Pre-warm：FFI 流式吐 hit 的同时并发构造父链 matcher；
  - §4.7 静态 gitignore 规则下推子集：严格白名单（仅 `<dirname>/` 形式，无通配，无 negation，无 whitelist 冲突），翻译为 `!regex:[\\\\/]<dirname>[\\\\/]`，含等价性证明单测；
  - §4.8 rayon 并行评估：chunk 64 内并行计算，chunk 间保到达顺序，`--exec` 模式 chunk 间立即送出。
- ✅ §4.3 性能预算从 ≤500 ms 下调到 ≤250 ms @ N=200k（三重优化叠加）。
- ✅ §5 FFI 新增 §5.1「一次性 metadata 请求」：`Everything_SetRequestFlags` 一把请求 6 字段，post-filter 永不 stat（hidden_filter 是 C1 强制的唯一例外）。
- ✅ 新增 §7「Performance Engineering Inventory」：逐项审计每条 hit 的 20 个 hot-path 操作，标"必需/可消除/已消除"，给出"典型 hit ≤6 内存比较 + 1 原子计数 + 1 输出，syscall 0"的可量化承诺。
- ✅ 语义骨架（v6 已 approved 的 §4.2 kind×chain、§3.X pipeline、§6.X 翻译/检测/后端选择）**一字未改**。v7 完全是在已批准基线上叠加内部优化。

## v5 → v6 变更摘要（评审追溯）

- **codex v5 #1（事实错误）**：✅ §6.1 把 basename 约束从 `file:`（Everything 里是 files-only，会过滤掉目录）改为 `nopath:`（Everything 文档原文"only match the filename"）；`--full-path` 改用 `path:` modifier；不依赖 GUI 全局 Match Path 开关。
- **codex v5 #2**：✅ §6.1 表加 `--exact`（翻为 `regex:^…$`）和 `--and` 多 pattern 翻译（空格 AND 连接；任一 pattern unsupported 整次回退）。
- **codex v5 #3**：✅ §6.1 case 处理改为复用 `Config.case_sensitive`（fd 已合并 smartcase / `-s` / `-i` / 全局 `(?i)` 后的最终结论），translation 层不二次解析；scoped `(?i:…)` 列入 §6.2 规则 5 unsupported。
- **codex v5 #4**：✅ §6.2 anchor 检测细化：仅 `^`/`$` 等价；`\A`/`\z`/`\Z`/`(?m)` 下的 anchor 全部列入 unsupported（规则 3）。新增 lookaround 规则 8。
- **codex v5 #5**：✅ 新增 §6.1.1 `--fixed-strings` 转义策略：保守白名单——只对字母/数字/Unicode 字母/`.-_+/` 简单字面量用 phrase 包裹；含 `"`/`\`/`?`/`*` 直接回退 LegacyWalker；含空格/`|`/`!`/`<>`/`:` 用 phrase 包裹。
- **codex v5 #6（残留）**：✅ Phase 1 `BackendQuery.literal_hints` 改为 `TranslatedPattern`（v6 删 literal_hints）；R20 重写删 `Everything_GetTotResults > MAX` 旧文，改为流式消费 + hard cap。
- **codex v5 #7**：✅ D1 dual-backend 测试模式改为**真**同时跑 EverythingBackend 与 LegacyWalkerBackend 做对称差分，捕获 false negative（v5 用 post-filter 跑 Rust 正则只能捕获 false positive）。

## v4 → v5 变更摘要（评审追溯）

- **用户质疑**：v1-v4 设计 "Everything 仅做字面量预筛 + post-filter 跑 Rust 正则复核" 是过度保守，Everything 原生支持 `regex:` 与 `wildcards:`，可直接下推。
- ✅ 架构 D1 重写：默认下推正则/glob；用 `regex_syntax::ast` 检测 Everything 不支持的语法（`\p{}`、Unicode-aware `\b`/`\w`、`(?m)`/`(?s)`/`(?x)`/`(?-u)`、嵌套深度爆炸、glob 否定等）；命中即整 pattern 回退 LegacyWalker（不退化为 post-filter，因为预筛已经不可靠 post-filter 也救不回）。
- ✅ 删除 `EVERYTHING_PREFILTER_MAX` 门槛、`Everything_SetMax(0)` count-only 探测、字面量提取相关全部内容。
- ✅ post-filter `regex_filter` 模块代码保留，但默认 no-op；仅在 `FDE_VERIFY_REGEX=1` 测试模式下作为 dual-backend 比对器使用，监测发散。
- ✅ Phase 6 重写：加 §6.1 翻译表、§6.2 unsupported 检测器、§6.3 其他 CLI 维度、§6.4 后端选择。
- ✅ 补三处之前低估的差异：basename vs full-path 默认（fd 默认 basename，Everything 默认 full path）；smartcase 转 case modifier；`--fixed-strings` 元字符转义。（v5 此处曾误写为用 `file:` 包裹，v6 已纠正为 `nopath:` modifier，并把 `--fixed-strings` 策略改为保守白名单，详见 §6.1 / §6.1.1。）
- ✅ 风险：R2 重写；新增 R24（unsupported 漏报）、R25（basename 默认弄反）。
- ✅ 验收 #6 性能基准重写：删"短字面量不退化"指标；改"可下推 ≤30% / 不可下推 ≈100% 容差 5%"。

## v3 → v4 变更摘要（评审追溯）

- **codex v3 #1**：✅ §4.2 第 7 点重写：模型从"同目录内 kind 优先 + 跨目录叶子优先"改为"kind 跨整条 parent chain，同 kind 内叶子优先"。直接读 `tests/tests.rs:837` 验证 `test_custom_ignore_precedence` 的真实结构（根 `.fdignore !foo` + `inner/.gitignore foo` → 显示）。给出双层循环伪代码（外层 kind、内层 chain）。其余 5 项 codex 已确认满足。

## v2 → v3 变更摘要（评审追溯）

- **codex v2 #1**：✅ §4.2 重写优先级模型为"kind × depth 双轴"，对齐 `ignore` crate 实际语义；同目录内 Custom > DotIgnore > Fdignore > Gitignore > GitInfoExclude；跨目录叶子优先；修复 `test_custom_ignore_precedence` 回归。
- **codex v2 #2**：✅ Phase 3 pipeline 重排：`ignore_contain` 提前到所有 depth/root 判断之前；新增"顺序变更原因"段说明依赖测试。
- **codex v2 #3**：✅ Phase 2 DirEntry 显式 `raw_path` + `display_path` 双路径不变量；stat 用 raw、占位符用 display；golden diff 验证。
- **codex v2 #4**：✅ 新增 §3.3 `same_filesystem` 模块覆盖 `--one-file-system`；用 `GetVolumeInformationByHandleW` 取卷序列号。
- **codex v2 #5**：✅ Phase 8 真实例方案改用 config 写盘代替未验证的 CLI 参数；SDK 函数名以官方头文件为准，Phase 5 第一天校准；删除 `-no-system-tray`、`Everything_RebuildIndex`、`Everything_GetIsIndexing` 三个未验证的具体名。
- **codex v2 #6**：✅ D1 探测改用 `Everything_SetMax(0)` count-only 调用；默认门槛从 1,000,000 降到 200,000，env 可调；Phase 8 基准负责校准默认值。

## v1 → v2 变更摘要（评审追溯）

- **codex #1**：✅ Phase 2 显式提 Batch/Sender/DirEntry 为 `pub(crate)`；DirEntry 加 `from_raw_hit` 构造。
- **codex #2**：✅ Phase 3 pipeline 扩充到 11 个 filter，覆盖 depth/type/extension/size/time/owner/prune/symlink/max_results。
- **codex #3 / gemini #1**：✅ Phase 2 新增 PathProjector 模块；规范化 ≠ 输出投影。
- **codex #4**：✅ §4.2 加 `.git/info/exclude`、git core.excludesFile、`.git` 文件（worktree/submodule）；继承边界表化。
- **codex #5**：✅ §4.2 继承边界表逐 ignore 类型说明，含 `.git` 文件情况。
- **codex #6**：✅ D1 加最小字面量门槛 + `Everything_GetTotResults` 探测 + 自动降级。
- **codex #7**：✅ `prune_filter`、`symlink_filter` 加入 pipeline；§3.1 写明 batch 模式。
- **codex #8 / gemini #3**：✅ Phase 8 测试策略翻新：MockBackend 主力 + LegacyWalker 回归 + 真 Everything 仅 smoke；用 `Everything_SetInstanceName` 隔离。
- **codex #9**：✅ Phase 顺序大调整（Mock 前移到 Phase 1，FFI 后移到 Phase 5）。
- **codex #10**：✅ 验收标准扩到 9 项，含 LegacyWalker 100% / Mock ≥ 95% / e2e smoke 100% / 性能分长短字面量分别约束。
- **codex nitpicks**：✅ R2 删 lookbehind 误述；Phase 5 删 `--sort`；改名 `--filesystem-walker`；改用 `regex-syntax` 抽字面量。
- **gemini #2**：✅ Phase 6 自动后端选择，非索引路径透明走 LegacyWalkerBackend。
- **gemini 新增风险**：✅ R23（权限）、R15/R16/R17/R18/R19/R20/R21/R22 全部纳入。
- **gemini nitpicks**：✅ 保留 `--filesystem-walker`；bindgen 直接绑；FFI 一次性 UTF-16→UTF-8。
