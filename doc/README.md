# PageClaw (爪页) 开发与设计文档中心

本目录归档 PageClaw 项目的架构设计、开发规范、工具扩展方案与核心使用指南。

---

## 📚 目录结构

| 文档名称 | 对应规划方向 | 说明 |
| :--- | :--- | :--- |
| **[01-ci-cd-and-release.md](./01-ci-cd-and-release.md)** | **方向一：发布流程与 CI/CD** | 涵盖 GitHub Actions 自动化工作流、多平台交叉编译（Windows / Linux / macOS）、Release 打包与发布规范。 |
| **[02-mcp-tools-extension.md](./02-mcp-tools-extension.md)** | **方向二：MCP 工具协议扩展** | 涵盖 Model Context Protocol (MCP) 服务端设计、新增工具注册（SQL 表格导出、CDP 截图、文档分块、文本摘要等）与协议交互规范。 |
| **[03-adapters-and-features.md](./03-adapters-and-features.md)** | **方向三：平台适配器与功能矩阵** | 涵盖 15+ 主流平台适配器（B站、知乎、微信公众号、小红书、GitHub、HF 等）全量参数对照、核心功能用法与实战案例。 |

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

# 4. 本地启动 MCP stdio 服务测试
cargo run -p pageclaw-cli -- mcp
```
