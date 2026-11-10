# Nix Home Studio — 产品总纲

Status: **生效中**（负责人 2026-11-08 确认）
Applies to: 所有后续里程碑、任务卡、代码评审与发布说明
Baseline checked: `HEAD = fec5785` = `origin/master`，工作区干净

这是本项目的长期依据。当代码、任务卡或口头描述与本文件冲突时，先停下来说明冲突，
由负责人决定改代码还是改本文件；不得由 Agent 自行扩大范围或另立一套路线。

---

## 1. 产品定位

**Your Nix environment, managed anywhere.**

帮助普通 Linux 用户跨发行版地管理、理解、迁移、锁定并复现自己的 Nix 环境。

目标用户不是 NixOS 专家，而是：

- Arch / CachyOS 用户
- Fedora 用户
- Ubuntu / Debian 用户
- 其他已经安装 Nix 的发行版用户
- 用 Home Manager 管理个人环境的开发者

典型场景（所有功能都要能回到这句话）：

> 我用 CachyOS / Fedora / Ubuntu，不想成为 Nix 专家，也不想手写维护一堆配置。
> 我只想管理我的软件、备份我的环境、换电脑时能迁过去。

产品不得要求用户先理解完整 Nix 生态才能完成日常操作。

---

## 2. 核心原则

1. 简单易用优先于功能数量。
2. 跨发行版优先于绑定 NixOS。
3. 安全、可恢复优先于自动化程度。
4. 真实环境验证优先于 fixture 通过。
5. 复用现有架构，不为每个功能重建一套系统。
6. 每个功能必须有清晰用户价值和明确安全边界。
7. 只读优先：先能看懂，再谈修改；先有 Diff，再谈写入。

---

## 3. 事实基线（2026-11-08 核对）

| 项目 | 事实 |
| --- | --- |
| 分支 / 提交 | `master`，`fec5785`，已推送，远端一致 |
| 标签 | `m6.4` → `2616f2a`（未移动） |
| 测试门禁 | **355 tests**，`check-all.sh` 通过，CI run `42493808449` 成功 |
| M1–M6.3 | 已验收（发现、编辑引擎、模块生成、校验、GUI 骨架） |
| M6.4 | 已验收，标签 `m6.4` |
| M6.5 | 已验收（真实环境验收；修复“校验依赖 PATH”缺陷，提交 `c852646`） |
| M6.6 | 审计与修复**已完成**（2026-11-04）：`docs/m66-accessibility-audit.md`、`docs/m66-accessibility-report.md`，6 个 GUI 缺陷已修 |
| M7（历史，`M7-gens`） | **已关闭 2026-11-08**：generation 只读列表、打包、隔离 E2E、Gate 8 test-only 证据 |
| 产品激活 / 回滚 | **产品从不执行** `home-manager switch` 或 rollback；E2E 只是测试沙箱 |
| 真实主机 | profile-only 布局，3 generations，Home Manager 从 store 解析 |

**M6.6 状态必须分五类，不得合并成一句“环境限制”**：

| 分类 | 事项 | 结论 |
| --- | --- | --- |
| **已修复** | F1–F6（Build-log 路由、错误未播报、校验完成未播报、切屏焦点丢失、缺 `aria-busy`、表格无名/表头无 scope）＋ `el()` 把 `aria-label` 写成 JS 属性导致搜索框无标签 | 2026-11-04 完成，带 `check-a11y.js` 回归测试 |
| **已真实桌面验证** | 键盘走完全流程、切屏后标题获得焦点、apply 后消息与标题存在、表格命名为 `table 'Packages'`、错误被 AT-SPI 暴露为 notification、`GDK_SCALE=2` 渲染正确 | 仅在 1× 面板可成立，见下一行 |
| **环境验证缺口**（不得报通过） | 屏幕阅读器实际语音（无 Orca / speech-dispatcher）、125% / 150% 缩放、真实 200% HiDPI 面板 | 有环境后复测；无环境就写“未验证” |
| **辅助技术验证缺口**（不得报通过） | 屏幕阅读器下表头导航与 verbosity（表头只暴露 value） | 依赖屏幕阅读器，同上 |
| **已修复（2026-11-09）** | ① skip link：`<nav>` 前加 skip-to-content，`<main id="main-content" tabindex="-1">`，键盘一次 Tab 即到内容；② 默认窗口改为 1100×700（`minWidth 900 / minHeight 600` 不变） | 两项均**不是**环境缺口。`check-a11y.js` 增加结构、本地化与焦点回归；真实桌面见 `docs/m66-accessibility-report.md` 的 close-out 记录 |
| **明确不做** | 完整 WCAG 2.2 AA 认证声明、重做高对比主题、自研屏幕阅读器 | 不在本项目范围 |

这些条目已记录，不得把环境/辅助技术验证缺口报成“通过”，也不得把已验证项归入“环境限制”。

---

## 4. 编号约定（含历史冲突说明）

仓库历史里 `M7` 已经被 **generations / rollback / 隔离 E2E** 阶段使用并关闭。
本总纲的阶段表中 `M7` 指 **Home Manager 完整体验**。两者不是同一件事，为避免引用混淆：

| 写法 | 含义 |
| --- | --- |
| `M7-gens` | 2026-11-08 关闭的历史阶段（generation 列表、打包、E2E） |
| `M7` / `M7-hm` | 本总纲的 Home Manager 完整体验（下一阶段） |
| `M8` … `M12` | 本总纲的 Flake Foundation 及以后，编号唯一且不得复用 |

新代码、提交信息、任务卡中引用历史阶段必须写 `M7-gens`，避免与 `M7-hm` 混淆。

---

## 5. 五个管理领域

领域之间相互独立。**每个新领域进入实现前，必须先有独立设计与边界审查**，
不得因为“已经在写 GUI”就顺手把新领域接进去。

| 域 | 内容 | 当前状态 | 进入条件 |
| --- | --- | --- | --- |
| **A — Nix Home** | Home Manager 发现、用户级软件包、模块生成、配置编辑 / Diff / 备份 / 冲突检测、build-only 校验、apply、history | **已实现，当前核心** | — |
| **B — Nix Development** | `nix develop`、`nix shell`、`nix-direnv`、`direnv`、devenv；项目级开发环境 | 未实现 | 独立设计 + 进程白名单扩展评审 |
| **C — Nix System** | NixOS 配置发现、系统 flake、系统 generation、nix-darwin；系统激活需独立安全模型 | 仅只读概念，未实现 | 独立安全设计（高权限），短期不做 |
| **D — Nix Store** | store 占用分析、`nix store verify`、`optimise`、GC、Cachix、binary cache 诊断 | 未实现 | GC 等破坏性操作需独立确认与保护设计 |
| **E — Nix Doctor** | nixfmt、statix、deadnix、nixd / nil、配置诊断、build 错误解释 | 未实现（M9） | 独立诊断服务，不混入 GUI 或 Home Manager Core |

Domain A 优先做成熟；B–E 不得提前动工。

---

## 6. 三项长期差异化能力

### Manage
让用户容易管理 Nix 环境。当前 Domain A 就是 Manage 的第一层。

### Migrate
跨设备、跨发行版迁移（M11）。必须考虑 CPU 架构、发行版差异、Home Manager 版本、
系统依赖、flake inputs、硬件相关配置、机器专属信息。
**不能复制 `home.nix` 就宣称迁移成功。**

### Lock
环境锁定与复现（M12）。必须区分：

- `flake.lock`：锁定 flake inputs；
- NHS Profile：记录用户配置、工具链、软件包清单、平台信息与迁移关系。

密钥、Token、SSH 私钥等敏感信息一律不得打包导出。

---

## 7. 编辑器集成原则

- 不内置完整 Neovim，不开发自己的通用代码编辑器。
- 使用用户默认外部编辑器；可选 Neovim / VS Code / Zed 集成。
- 诊断由 NHS 独立提供，结果可定位到文件与行（M9）。
- 编辑后仍然走现有 Diff → Build Validation → Apply 流程。
- `nixfmt` / `statix` / `deadnix` / `nixd` / `nil` **不是** GUI 的硬性依赖。

---

## 8. 版本路线图

| 阶段 | 核心任务 | 主要交付 | 状态 |
| --- | --- | --- | --- |
| M6.5 | 基础可靠性 | 真实环境验收、store 工具链修复 | 已完成 |
| M6.6 | 交互与可访问性 | 键盘、焦点、AT-SPI、HiDPI | 审计 + 修复已完成；剩余验证项见 §3 |
| M7 (hm) | Home Manager 完整体验 | 日常配置管理 | **进行中：WP1 布局识别与 WP2 配置状态已实现（2026-11-09）；WP3 下一步** |
| M8 | Flake Foundation | Flake 发现、解析、关系展示（只读） | 已定义，未开始 |
| M9 | Nix Doctor | 代码诊断与质量检查 | 已定义，未开始 |
| M10 | Machine Profile | 机器环境档案（数据模型优先） | 已定义，未开始 |
| M11 | Migration | 跨设备、跨发行版迁移分析与计划 | 已定义，未开始 |
| M12 | Lock & Export | 配置锁定、导出与复现 | 已定义，未开始 |

依赖关系：M8 → M10 → M11 → M12 是硬依赖（先理解环境，才能记录档案，才能比较迁移，才能锁定复现）。
M9 与 M10 可并行，但都不得早于 M8 的解析结果落地。

---

## 9. M6.6 — 交互、键盘与可访问性可靠性

**已完成的证据**：`docs/m66-accessibility-audit.md`（审计）、
`docs/m66-accessibility-report.md`（修复报告）。已修 6 个 GUI 缺陷（Build-log 路由、
错误未播报、校验完成未播报、切屏焦点丢失、缺少 `aria-busy`、表格无名/表头无 scope），
并修掉 `el()` 把 `aria-label` 写成 JS 属性导致搜索框没有标签的问题。

**剩余事项与处置结论**：

| 事项 | 分类 | 处置 |
| --- | --- | --- |
| 屏幕阅读器语音 | 环境验证缺口 | 安装 GNOME Orca / speech-dispatcher 后复测；在此之前只报 AT-SPI 语义证据 |
| 125% / 150% 缩放 | 环境验证缺口 | 需要可安全改变 scale 的会话；不得改用户桌面设置来凑测试 |
| 真实 200% HiDPI 面板 | 环境验证缺口 | 2× 渲染已正确，缺的是真实面板；借到设备再测 |
| 表头语音导航 | 辅助技术验证缺口 | 依赖屏幕阅读器；先确认表头 value/name 再决定是否补 ARIA |
| **skip link** | **产品缺陷（WCAG 2.4.1）已修复 2026-11-09** | 在 `<nav>` 前加 skip-to-content，`<main>` 可编程聚焦；`check-a11y.js` 覆盖结构、目标、本地化和焦点落点；AT-SPI 暴露为 `link 'Skip to content'` |
| **默认窗口尺寸** | **产品体验问题已修复 2026-11-09** | 默认窗口改为 1100×700，`minWidth/minHeight` 保持 900/600；1366×768 屏幕含标题栏不再溢出 |

两项产品问题均已于 2026-11-09 修复，带 `check-a11y.js` 回归测试；真实桌面启动与 AT-SPI 语义已验证，但本机平铺合成器未按默认几何显示窗口，键盘焦点由自动化回归覆盖。环境与辅助技术验证缺口仍按上表处理。
M6.6 不增加任何 Nix 管理功能，也不因这两项改动放宽任何安全边界。

---

## 10. M7 (hm) — Home Manager 完整体验

目标：让 Domain A 真正成为日常可用工具，而不是演示流程。

范围（按价值排序）：

1. 配置布局识别完善：classic / flake-home / profile-only / 歧义场景的说明与引导。
2. Home Manager 配置状态展示：当前 generation、配置来源、可编辑性、只读原因。
3. 软件包管理流程完善：curated catalog 的搜索、选择、移除、重复项与冲突提示。
4. 错误诊断：把 build / validation 失败翻译成可执行的下一步，保留原始输出。
5. Diff 与备份体验：可读性、折叠、备份可见与可恢复说明。
6. Apply 可靠性：失败回滚、快照一致性、重复 apply 拒绝（已具备，需补真实场景）。
7. Generation 与 History 信息完善：包括 `home-manager generations` 交叉校验（M7-gens 的 WP3 遗留）。
8. Flake Home Manager 布局兼容性评估：只评估，不实现 Flake 编辑（留给 M8）。

### 10.1 与 `M7-gens` 的边界（必须写进任务卡）

`M7-gens` 已交付并关闭 generation 列表、打包、隔离 E2E 与 Gate 8 证据。
`M7` (hm) **不重新打开**这一领域：

| 不来一遍 | 原因 |
| --- | --- |
| 不重新实现 generations 发现/列表 | `M7-gens` 已交付且被真实主机验证；本阶段只补 `home-manager generations` 交叉校验（WP3 遗留），不改模型 |
| 不扩大 rollback 的权限或安全边界 | rollback 可行性已在 `M7-gens` 定案为“只描述命令、不执行”；本阶段不改该决定 |
| 不引入系统级激活 | 属 Domain C，需独立安全模型 |
| 不引入 `sudo` / `nixos-rebuild` | 长期禁令，见 §18 |
| 不把 Flake Foundation 提前塞进 M7 | M8 的发现/解析是独立阶段；M7 最多做“flake-home 布局的兼容性评估”，不解析 inputs/outputs |
| 不只因为想重构就重写 GUI | 只在复现的真实体验缺陷上做小改动 |

判据：M7 的每个工作包都必须能回答“它改善了哪一步日常体验”。答不上来的，不做。

### 10.2 明确不做

真实用户配置初始化写入、产品内 `switch` / rollback、GC、`nix-env`、Flake 编辑/解析、
系统级配置、自研编辑器。

**真实用户配置的初始化与写入需要独立安全设计，并且不自动获得授权。**

---

## 11. M8 — Flake Foundation（只读）

### 11.1 目标

让 NHS 能识别、理解并展示用户的 Flake 环境；**不修改 Flake**，不自动执行
`nix flake update`。“只读”的准确含义见 §11.4：只读指不写用户文件，**不等于**没有
store / 缓存 / 网络副作用。

起点（已核对代码）：当前只有布局判定——`discovery.rs` 文本探测 `flake.nix` 是否声明
`homeConfigurations.<user>`、`home.nix` 是否被 flake 引用；**没有** inputs / outputs /
`flake.lock` 的结构化解析，`proc.rs` 的 Nix 子命令白名单只有 `build`。

### 11.2 交付范围

1. **Flake 自动发现**：`flake.nix`、`flake.lock`、`home.nix`、Home Manager flake 配置、
   NixOS flake 配置；支持不同目录布局，不假定单一结构。
2. **Flake 解析**：inputs、input 来源与版本、outputs、Home Manager 入口、NixOS 入口、
   用户配置与系统配置的关系。
3. **`flake.lock` 分析**：锁定 inputs、每个 input 的 revision、原始引用与锁定版本、
   间接依赖、锁定文件缺失或异常。
4. **平台与兼容性分析**：CPU 架构、Linux 平台、flake 声明的系统、Home Manager 与
   nixpkgs 的版本关系、可能的兼容性问题（为 M10/M11 打基础）。
5. **GUI 展示**：独立 Flake 页面——Overview / Inputs / Outputs / Home Manager / NixOS /
   Compatibility。只展示，不编辑。

展示模型是**理解结构**，不是要求用户采用某种目录：

```
Nix Environment
├── Flake            inputs, outputs
├── Home Manager     home.nix, modules/
└── NixOS            configuration.nix, hosts/
```

### 11.3 M8 明确不做

| 功能 | M8 |
| --- | --- |
| Flake 自动发现 | 是 |
| Flake 只读解析 | 是 |
| `flake.lock` 分析 | 是 |
| Outputs 关系展示 | 是 |
| 平台信息识别 | 是 |
| Flake 编辑器 | 否 |
| 自动修改 `flake.nix` | 否 |
| 自动更新 flake inputs | 否 |
| 自动迁移配置 | 否 |
| Machine Profile | 否（M10） |
| Migration | 否（M11） |
| Lock & Export | 否（M12） |

### 11.4 M8 必须解决的设计决策

先把“只读”定义清楚：**只读 ≠ 无副作用**。M8 涉及三类互不相同的副作用，
必须分别评估，不得用“只读分析”一句带过：

| 副作用类别 | 例子 | M8 规则 |
| --- | --- | --- |
| **用户配置文件写入** | 改 `flake.nix`、`flake.lock`、`home.nix` | **永久禁止**（M8 与后续阶段都不自动做） |
| **Store / 缓存 / 网络** | `nix flake metadata` 可能求值、拉取 remote input、写 store 与缓存 | 默认不使用；若使用必须单独设计与审查，见 D8.1 |
| **进程启动** | 任何 `nix` 子命令 | 必须经 `proc.rs` 白名单；当前白名单只有 `build` |

- **D8.1 数据获取方式**：优先纯静态解析（读 `flake.nix`、`flake.lock`，无求值、无网络、
  无 store 写入）。当静态解析无法得到可靠结果时的处理顺序：
  1. 该项标记为“未求值 / 不可确定”，不猜；
  2. 若确需 Nix 自身求值（`nix flake metadata` 等），必须单独立项评审：白名单扩展、
     超时、离线行为、网络与 remote input 策略、store/缓存写入的可接受性、失败降级。
  3. 评审通过前，产品不得因为“反正只是 metadata”就调用它。它**不是**只读操作。
- **D8.2 职责分层**：解析结果的数据模型由 `nhs-core` 定义与管理（无 GUI 依赖）；
  GUI 只负责展示；具体数据获取由受控 Adapter 提供，Adapter 不得自行定义模型或绕过
  Core 的策略。M9–M12 复用同一模型，不各自重写发现/解析。
- **D8.3 用户文件只读**：M8 不新增任何写用户文件的路径——不创建、不修改、不删除
  `flake.nix` / `flake.lock` / 用户配置；不自动执行 `nix flake update`。
- **D8.4 结果诚实性**：纯静态解析得到的 outputs/inputs 必须与“已求值结果”在 UI 和
  JSON 中明确区分（例如 `source: static` / `evaluated: false`）；不得把静态解析的
  近似结论描述成求值成功的 outputs，也不得把“未发现”写成“不存在”。

---

## 12. M9 — Nix Doctor

提供 `nixfmt`、`statix`、`deadnix`、`nixd` / `nil` 集成评估、诊断报告、定位与修复建议。
格式化与自动修复必须走 Diff + 用户确认；诊断服务独立于 GUI 与 Home Manager Core。
这些工具缺失时产品必须正常降级，不得成为硬依赖。

---

## 13. M10 — Machine Profile

定义跨设备环境档案：平台、架构、发行版、Nix 版本、Home Manager 版本、配置来源、
flake inputs、用户软件包清单。**先做数据模型与导出格式**，不急着做复杂 GUI。
复用 M8 的 Flake 解析结果。

---

## 14. M11 — Migration

支持跨机器、跨发行版迁移分析与计划生成：先比较（M10 档案），再生成计划，
最后只执行经过确认的操作。系统级 / 高权限步骤不在自动执行范围内。

---

## 15. M12 — Lock & Export

配置快照、flake 锁定状态、环境导出和复现检查。导出前必须排除密钥、Token、SSH 私钥等
敏感信息，并提供“导出了什么 / 排除了什么”的清单。

---

## 16. 横向轨道：跨发行版使用基线

不是单独消耗里程碑编号的横切要求，贯穿所有阶段：

- **运行基线**：Arch / CachyOS、Fedora、Ubuntu / Debian 上的构建与启动；明确记录
  哪些是真实桌面验证、哪些只是容器或 Xvfb。
- **安装形态**：以 Nix flake 为主；AppImage / Flatpak / deb 需评估 WebKitGTK 依赖成本，
  未评估前不得宣称“一键安装”。
- **未安装 Nix 的引导**：给出官方安装方式与检查步骤，**不自动 sudo**。
- **安装损坏 / 权限不足 / daemon 异常**：给出诊断与修复命令，不自动执行提权操作。
- **Home Manager 缺失**：沿用现有创建提示。

当前证据：CachyOS 原生桌面、Ubuntu 24.04 容器（headless / Xvfb，非原生桌面）、
学校 WSL2 记录。**未验证**：Fedora / Arch 原生桌面、Wayland HiDPI 缩放、AppImage / Flatpak。

---

## 17. 架构与工程纪律

- Rust Core + Adapter + Tauri GUI；Core 不依赖 GUI。
- GUI 不实现业务安全逻辑；Adapter 不绕过 Core 的权限与安全策略。
- 所有 Nix 进程调用经过统一受控执行层（`proc.rs` 白名单 + 超时 + 输出上限）。
- 统一安全写入路径：Diff / Backup / Conflict Guard / 快照。
- 明确状态机；异步结果必须经 `Event` 回流，不绕过 revision / attempt 校验。
- 不为了扩展产品而提前建立庞大抽象框架；但**同一能力只实现一次**（M8 解析结果供 M9–M12 复用）。
- 每个新增 Domain 先设计、再评审、再实现。
- 完整门禁：`sh crates/nhs-gui/scripts/check-all.sh`，外加 `nix flake check`。

---

## 18. 安全边界与绝对禁止

以下为长期约束，任何里程碑都不得自行放宽：

- 不移动 `m6.4` 标签，不 force push，不未经授权推送或发布。
- 不擅自升级 Rust / 工具链版本。
- 不创建或修改用户的 `~/.config/home-manager`；不对真实用户配置执行写入，除非有独立授权与设计。
- 不执行 `nixos-rebuild switch`、系统级激活、`sudo` 或任何提权命令。
- 不执行 `home-manager switch` / rollback（产品路径）；测试沙箱 E2E 需单独授权。
- 不执行 Nix GC 或任何破坏性 Nix 操作。
- 不为测试执行可能消耗数 GB 磁盘的构建。
- 不把 fixture / 容器 / Xvfb 结果描述成真实机器验收。
- 不修改 `upstream/` 只读参考仓库。
- 不读取、打印、复制或修改密钥、Token、浏览器 profile 或无关个人文件。
- 不因为“两个项目都用 Rust/Tauri”就合并代码；先核对模块边界、数据模型、依赖与许可证。

Gate 8（隔离 E2E）现有证据为 **PASS (test-only, 2026-11-08)**，
沙箱 `/var/tmp/nhs-e2e.d45fGg` 保留；这不代表产品支持激活或回滚。

---

## 19. 每个阶段的交付格式

每个阶段结束必须给出：

1. 做了什么；
2. 为什么做；
3. 修改了哪些文件；
4. 测试结果（命令与输出摘要）；
5. 真实环境证据（没有就写没有）；
6. 未完成事项；
7. 风险与下一步。

不得把未经验证的结论写成已完成；不得自行扩大任务范围。

---

## 20. 当前决定

- 路线与阶段职责按本文执行；M8 是**只读 Flake Foundation**，不是 Flake 编辑器，
  也不是迁移功能。“只读”按 §11.4 定义：用户文件只读；store / 缓存 / 网络是另一类
  副作用，单独评估；静态解析结果不得说成求值结果。
- M6.6 的审计与修复已完成。skip link 与默认窗口尺寸已于 **2026-11-09** 修复：
  skip link 带 `check-a11y.js` 结构与焦点回归，窗口默认 1100×700、最小尺寸不变。
  真实桌面启动与 AT-SPI 语义已验证；本机平铺合成器未按默认几何显示窗口，键盘
  焦点由自动化回归覆盖。屏幕阅读器语音、125% / 150% 缩放、真实 200% HiDPI、
  表头语音导航仍是环境/辅助技术验证缺口，不得报成通过。
- 下一阶段为 `M7` (hm)：Home Manager 完整体验；与 `M7-gens` 的边界见 §10.1，
  不重做 generations、不扩大 rollback 权限、不引入系统级激活。
- M8 之后依次 M9 → M10 → M11 → M12；M8 的解析结果必须被后续阶段复用。
- 不内置 Neovim；不急于开发 NixOS 管理功能。
