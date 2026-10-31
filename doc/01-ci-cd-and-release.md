# 01 - 发布流程与 CI/CD 自动化构建指南

本文档规定 TkLess 的版本管理规范、多平台自动化交叉编译方案与 GitHub Actions 流水线配置。

---

## 1. 版本命名规范 (Semantic Versioning)

TkLess 遵循 [SemVer 2.0.0](https://semver.org/) 规范：
- **主版本号 (Major)**：架构级变更、底层协议不兼容调整（例如 MCP 协议核心定义重构）。
- **次版本号 (Minor)**：向后兼容的新特性引入（例如新增平台适配器、新增 CLI 子命令或导出格式）。
- **修订号 (Patch)**：向后兼容的缺陷修复、性能调优或依赖补丁。

---

## 2. GitHub Actions CI/CD 流水线设计

在仓库根目录 `.github/workflows/` 下设立两组自动化工作流：
1. `ci.yml`：针对所有 PR 与 `main` 分支 push 触发自动化测试、Clippy 与格式校验。
2. `release.yml`：基于 Git Tag (`v*.*.*`) 触发多平台构建、Minisign 签名并生成 GitHub Release。

### 2.1 多平台构建矩阵

`release.yml` 拆分为两个 job：

- `build`：矩阵构建各平台产物，执行二进制版本检查，生成兼容用的同名 `.sha256` 文件，并上传为 workflow artifact。该 job **不直接发布 Release**。
- `publish`：依赖全部 `build` 成功，在受保护 Environment `release-signing` 中下载全部 artifact，生成 `SHA256SUMS`，用 Minisign 私钥签名，验证签名后创建或更新 GitHub Release。

构建矩阵覆盖：

| 目标 | 产物 |
| :--- | :--- |
| `x86_64-pc-windows-msvc` | `tkless-windows-amd64.exe` |
| `x86_64-unknown-linux-gnu` | `tkless-linux-amd64` |
| `aarch64-unknown-linux-gnu` | `tkless-linux-arm64` |
| `x86_64-apple-darwin` | `tkless-macos-amd64.dmg` |
| `aarch64-apple-darwin` | `tkless-macos-arm64.dmg` |

实际工作流见 [release.yml](../.github/workflows/release.yml)。

### 2.2 发布签名（Minisign）

自升级的信任锚是编译进二进制的 Minisign 公钥，而不是 GitHub Release 元数据。

**一次性准备：**

1. 在离线或受控机器上生成密钥对：

   ```bash
   minisign -G -p tkless-upgrade.pub -s tkless-upgrade.key
   ```

2. 将 `tkless-upgrade.pub` 的内容提交到：

   ```text
   crates/tkless-cli/assets/tkless-upgrade.pub
   ```

   文件可以包含多个公钥块（`untrusted comment:` + base64 行），用于轮换。构建时会把所有 base64 行嵌入二进制。

3. 在 GitHub 仓库设置中创建 Environment：

   - 名称：`release-signing`
   - 配置 Required reviewers（至少一名维护者）
   - 仅允许受保护分支/标签触发（按 GitHub 当前可用策略配置）

4. 在该 Environment 中保存 Secrets：

   | Secret | 内容 |
   | :--- | :--- |
   | `MINISIGN_SECRET_KEY` | Minisign 私钥文件的 base64 编码（`base64 -w0 tkless-upgrade.key`） |
   | `MINISIGN_PASSWORD` | 私钥密码 |

   私钥不要提交到仓库，也不要放进普通仓库级 Secret；只放在受保护 Environment 中。

> 本地源码构建默认没有内置公钥（仓库中的 `tkless-upgrade.pub` 是占位文件），因此 `tkless upgrade` 会 fail closed。需要先按上述步骤写入真实公钥并重新构建，或使用官方签名版本。

**发布时：**

- `publish` job 需要人工批准后才会运行。
- 聚合所有构建产物，生成带版本头的清单：

  ```text
  # version: 0.6.0
  # generated: 2026-10-31T00:00:00Z
  <sha256>  tkless-windows-amd64.exe
  ...
  ```

- 使用 `minisign -Sm SHA256SUMS` 生成 `SHA256SUMS.minisig`。
- 用仓库内公钥执行 `minisign -Vm SHA256SUMS -p ...` 自检；失败则整个发布失败，不会创建 Release。
- 上传全部产物、`SHA256SUMS` 与 `SHA256SUMS.minisig`。

**密钥轮换：**

1. 保留当前公钥，并把下一把公钥的 base64 行追加到 `tkless-upgrade.pub`。
2. 用当前私钥发布一个包含两把公钥的版本（过渡版本）。
3. 之后的 Release 改用新私钥签名；旧版本二进制仍能验证过渡版本，新版本二进制可验证后续版本。
4. 稳定后从公钥文件中移除旧公钥。

### 2.3 自升级验证顺序

`tkless upgrade` 在替换可执行文件前执行：

1. 从 GitHub Release 读取版本与资产列表（**不可信传输信息**）。
2. 下载 `SHA256SUMS` 与 `SHA256SUMS.minisig`。
3. 用内置公钥验证 Minisign 签名（任一内置公钥通过即可）。
4. 校验清单 `# version:` 与 Release tag 一致。
5. 在清单中查找当前平台资产，取得 SHA-256。
6. 下载资产并校验 SHA-256。
7. 写入候选文件，再次校验，执行 `--version` 探测，替换并回滚保护。

以下情况一律拒绝升级：

- 构建未内置公钥；
- Release 缺少 `SHA256SUMS` 或 `SHA256SUMS.minisig`；
- 签名无效或没有匹配的内置公钥；
- 清单版本与 Release 版本不一致；
- 清单缺少当前平台资产或包含重复条目；
- 资产 SHA-256 不匹配。

`--force` 只影响版本比较，不会绕过签名校验。`--repo` 指向的仓库必须由内置公钥签名，否则升级会被拒绝。

---

## 3. 产物二进制优化与瘦身策略

TkLess 在根目录 `Cargo.toml` 中配置了极速体积优化 profile：

```toml
[profile.release]
opt-level = "z"         # 体积优先优化
lto = true              # 全链接时优化 (跨 Crate 死代码消除)
codegen-units = 1       # 单编译单元获得最大跨模块优化
panic = "abort"         # 移除 unwind 表，大幅缩减体积
strip = true            # 自动剥离全部调试符号
```

### 产物体积与校验

体积随目标平台、依赖、feature 和工具链变化，应记录对应构建产物的实际大小，不使用固定体积作为已达成的跨平台保证。

发布工作流为每个二进制或 DMG 生成同名 `.sha256` 文件（兼容旧客户端），并在受保护环境中生成、签名 `SHA256SUMS`。新版本自升级只信任已签名清单中的哈希；缺少签名、签名无效或版本不匹配时拒绝替换。

工作区更新说明不代表已经发布；版本标签、版本号与 Release 正文应在发版时共同核对。

---

## 4. 本地发版检查清单 (Release Checklist)

在打 Tag 发版之前，必须执行以下核验步骤：

1. **版本号同步**：
   - 检查 `Cargo.toml` 中的 `workspace.package.version` 是否已更新。
2. **公钥就绪**：
   - 确认 `crates/tkless-cli/assets/tkless-upgrade.pub` 包含当前签名公钥（不是占位内容）。
   - 确认 `release-signing` Environment 的 `MINISIGN_SECRET_KEY` 与 `MINISIGN_PASSWORD` 可用。
3. **全量测试通过**：

   ```bash
   cargo test --workspace --all-targets
   cargo clippy --workspace --all-targets -- -D warnings
   cargo fmt --all -- --check
   ```

4. **工作区状态与提交**：
   - 确保 `git status` 干净，无未追踪文件。
5. **生成版本 Tag 并推送**：

   ```bash
   git tag -a vX.Y.Z -m "Release vX.Y.Z"
   git push origin vX.Y.Z
   ```

6. **批准签名**：
   - 在 GitHub Actions 中批准 `release-signing` Environment 的 `publish` job。
   - 确认 Release 同时包含 `SHA256SUMS` 与 `SHA256SUMS.minisig`。

`X.Y.Z` 是模板，须替换为实际待发布版本，并先确认标签不存在。发布前同时运行格式、Clippy 与 release 构建检查；当前命令见 [开发规范](04-development-standards.md)。
