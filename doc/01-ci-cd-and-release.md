# 01 - 发布流程与 CI/CD 自动化构建指南

本文档规定 PageClaw 的版本管理规范、多平台自动化交叉编译方案与 GitHub Actions 流水线配置。

---

## 1. 版本命名规范 (Semantic Versioning)

PageClaw 遵循 [SemVer 2.0.0](https://semver.org/) 规范：
- **主版本号 (Major)**：架构级变更、底层协议不兼容调整（例如 MCP 协议核心定义重构）。
- **次版本号 (Minor)**：向后兼容的新特性引入（例如新增平台适配器、新增 CLI 子命令或导出格式）。
- **修订号 (Patch)**：向后兼容的缺陷修复、性能调优或依赖补丁。

---

## 2. GitHub Actions CI/CD 流水线设计

在仓库根目录 `.github/workflows/` 下设立两组自动化工作流：
1. `ci.yml`：针对所有 PR 与 `main` 分支 push 触发自动化测试、Clippy 与格式校验。
2. `release.yml`：基于 Git Tag (`v*.*.*`) 自动触发三端交叉编译并生成 GitHub Release 产物。

### 2.1 多平台发布矩阵 (`release.yml`)

```yaml
name: Release PageClaw Binary

on:
  push:
    tags:
      - 'v*.*.*'

permissions:
  contents: write

jobs:
  build-release:
    name: Build - ${{ matrix.target }}
    runs-on: ${{ matrix.os }}
    strategy:
      matrix:
        include:
          - target: x86_64-pc-windows-msvc
            os: windows-latest
            artifact_name: pageclaw-windows-amd64.zip
            binary_name: pageclaw.exe
          - target: x86_64-unknown-linux-gnu
            os: ubuntu-latest
            artifact_name: pageclaw-linux-amd64.tar.gz
            binary_name: pageclaw
          - target: aarch64-unknown-linux-gnu
            os: ubuntu-latest
            artifact_name: pageclaw-linux-arm64.tar.gz
            binary_name: pageclaw
            use_cross: true
          - target: x86_64-apple-darwin
            os: macos-13
            artifact_name: pageclaw-darwin-amd64.tar.gz
            binary_name: pageclaw
          - target: aarch64-apple-darwin
            os: macos-latest
            artifact_name: pageclaw-darwin-arm64.tar.gz
            binary_name: pageclaw

    steps:
      - name: Checkout repository
        uses: actions/checkout@v4

      - name: Install Rust Toolchain
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}

      - name: Install cross (if needed)
        if: matrix.use_cross
        run: cargo install cross --git https://github.com/cross-rs/cross

      - name: Build Release Binary
        run: |
          if [ "${{ matrix.use_cross }}" = "true" ]; then
            cross build --release --target ${{ matrix.target }} -p pageclaw-cli
          else
            cargo build --release --target ${{ matrix.target }} -p pageclaw-cli
          fi
        shell: bash

      - name: Package Artifact (Windows)
        if: runner.os == 'Windows'
        run: |
          7z a ${{ matrix.artifact_name }} ./target/${{ matrix.target }}/release/${{ matrix.binary_name }} README.md
        shell: bash

      - name: Package Artifact (Unix)
        if: runner.os != 'Windows'
        run: |
          tar -czvf ${{ matrix.artifact_name }} -C ./target/${{ matrix.target }}/release ${{ matrix.binary_name }}
        shell: bash

      - name: Upload Release Asset
        uses: softprops/action-gh-release@v2
        with:
          files: ${{ matrix.artifact_name }}
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

---

## 3. 产物二进制优化与瘦身策略

PageClaw 在根目录 `Cargo.toml` 中配置了极速体积优化 profile：

```toml
[profile.release]
opt-level = "z"         # 体积优先优化
lto = true              # 全链接时优化 (跨 Crate 死代码消除)
codegen-units = 1       # 单编译单元获得最大跨模块优化
panic = "abort"         # 移除 unwind 表，大幅缩减体积
strip = true            # 自动剥离全部调试符号
```

### 推荐产物体积基线：
- Linux (x86_64): `< 12 MB`
- macOS (Apple Silicon): `< 10 MB`
- Windows (x86_64): `< 11 MB`

---

## 4. 本地发版检查清单 (Release Checklist)

在打 Tag 发版之前，必须执行以下核验步骤：

1. **版本号同步**：
   - 检查 `Cargo.toml` 中的 `workspace.package.version` 是否已更新。
2. **全量测试通过**：
   ```bash
   cargo test --workspace --all-targets
   ```
3. **工作区状态与提交**：
   - 确保 `git status` 干净，无未追踪文件。
4. **生成版本 Tag 并推送**：
   ```bash
   git tag -a v0.1.0 -m "Release v0.1.0: Support 15+ adapters, MCP streaming, and SQL table export"
   git push origin main --tags
   ```
