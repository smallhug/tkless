# TkLess 开发与设计文档中心

本目录说明当前工作区的使用方式、协议、开发标准与验证记录。文档对齐日期：2026-10-28；版本与迁移另见 [工作区更新说明](../RELEASE_NOTES.md) 和 [变更日志](../CHANGELOG.md)。
> **Spend less tokens, perceive more web.**

---

## 📚 目录结构

| 文档名称 | 对应规划方向 | 说明 |
| :--- | :--- | :--- |
| **[01-ci-cd-and-release.md](./01-ci-cd-and-release.md)** | **方向一：发布流程与 CI/CD** | 涵盖 GitHub Actions 自动化工作流、多平台交叉编译（Windows / Linux / macOS）、Release 打包与发布规范。 |
| **[02-mcp-tools-extension.md](./02-mcp-tools-extension.md)** | **方向二：MCP 工具协议扩展** | 当前 MCP 接入、Bing 搜索参数、协议响应、进度与动态工具发现。 |
| **[03-adapters-and-features.md](./03-adapters-and-features.md)** | **方向三：平台适配器与功能矩阵** | 平台适配器参数、核心用法与实战案例；当前注册 22 个内置适配器。 |
| **[04-development-standards.md](./04-development-standards.md)** | **工程标准：开发规范指南** | 涵盖架构分层、两阶段动态发现标准规范、SSRF与凭据安全硬约束、适配器扩展规程及 Git 提交规范。 |
| **[05-search-engine-validation.md](./05-search-engine-validation.md)** | **搜索与功能验证** | Bing 搜索验证说明、源码修复、批量输出迁移和自动化验证记录。 |
| **[06-bing-search.md](./06-bing-search.md)** | **Bing 搜索** | HTTPS 连接预检、平台定向、精简参数与验证记录。 |
| **[07-batch-jobs.md](./07-batch-jobs.md)** | **批量任务** | JSONL 流式输出、检查点、失败重试、单主机调度与 MCP 流式扩展。 |
| **[08-skills-sh.md](./08-skills-sh.md)** | **Skills.sh** | 技能详情、折叠正文恢复、仓库列表、站内搜索、榜单自动翻页及公开接口边界。 |

---

## 🛠️ 快速开发常用命令

```bash
# 1. 运行工作区全部测试
cargo test --workspace

# 2. 运行代码格式与 Lint 检查
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings

# 3. 生产优化构建 (启用 LTO 与二进制裁切)
cargo build --release

# 4. 本地启动 MCP stdio 服务测试 (标准全量模式)
cargo run -p tkless-cli -- mcp

# 5. 本地启动 MCP 两阶段动态工具发现模式
cargo run -p tkless-cli -- mcp --dynamic
```
