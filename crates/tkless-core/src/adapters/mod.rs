pub mod arxiv;
pub mod baike;
pub mod bilibili;
pub mod csdn;
pub mod discourse;
pub mod github;
pub mod hackernews;
pub mod huggingface;
pub mod juejin;
pub mod medium;
pub mod paperswithcode;
pub mod reddit;
pub mod skills;
pub mod stackoverflow;
pub mod substack;
pub mod twitter;
pub mod v2ex;
pub mod wechat;
pub mod wikipedia;
pub mod xiaohongshu;
pub mod youtube;
pub mod zhihu;

pub use arxiv::ArxivAdapter;
pub use baike::BaikeAdapter;
pub use bilibili::BilibiliAdapter;
pub use csdn::CsdnAdapter;
pub use discourse::DiscourseAdapter;
pub use github::GithubAdapter;
pub use hackernews::HackerNewsAdapter;
pub use huggingface::HuggingfaceAdapter;
pub use juejin::JuejinAdapter;
pub use medium::MediumAdapter;
pub use paperswithcode::PapersWithCodeAdapter;
pub use reddit::RedditAdapter;
pub use skills::SkillsAdapter;
pub use stackoverflow::StackoverflowAdapter;
pub use substack::SubstackAdapter;
pub use twitter::TwitterAdapter;
pub use v2ex::V2exAdapter;
pub use wechat::WechatAdapter;
pub use wikipedia::WikipediaAdapter;
pub use xiaohongshu::XiaohongshuAdapter;
pub use youtube::YoutubeAdapter;
pub use zhihu::ZhihuAdapter;

use crate::error::TklessError;
use crate::model::{ExtractionOptions, PageDocument};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use url::Url;

/// Type alias for dyn-compatible asynchronous return futures in PlatformAdapter.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Trait implemented by platform-specific content extractors.
pub trait PlatformAdapter: Send + Sync {
    /// Identifier of the platform (e.g. "bilibili")
    fn name(&self) -> &'static str;

    /// Checks if this adapter should intercept the given URL.
    fn matches(&self, url: &Url) -> bool;

    /// Whether generic HTML extraction can represent this resource after an adapter failure.
    /// API-only catalog resources should return false instead of exposing a client-side shell.
    fn allow_html_fallback(&self, _url: &Url) -> bool {
        true
    }

    /// Fetches and formats the platform content into a PageDocument.
    fn extract<'a>(
        &'a self,
        fetcher: &'a crate::fetcher::Fetcher,
        url: &'a Url,
        options: &'a ExtractionOptions,
    ) -> BoxFuture<'a, Result<PageDocument, TklessError>>;
}

/// Registry managing active platform adapters.
#[derive(Clone)]
pub struct AdapterRegistry {
    adapters: Vec<Arc<dyn PlatformAdapter>>,
}

impl Default for AdapterRegistry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

impl AdapterRegistry {
    /// Creates an empty registry without adapters.
    pub fn new() -> Self {
        Self {
            adapters: Vec::new(),
        }
    }

    /// Creates a registry initialized with all built-in platform adapters.
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(BilibiliAdapter::new()));
        registry.register(Arc::new(YoutubeAdapter::new()));
        registry.register(Arc::new(WechatAdapter::new()));
        registry.register(Arc::new(ZhihuAdapter::new()));
        registry.register(Arc::new(XiaohongshuAdapter::new()));
        registry.register(Arc::new(JuejinAdapter::new()));
        registry.register(Arc::new(GithubAdapter::new()));
        registry.register(Arc::new(HackerNewsAdapter::new()));
        registry.register(Arc::new(DiscourseAdapter::new()));
        registry.register(Arc::new(TwitterAdapter::new()));
        registry.register(Arc::new(PapersWithCodeAdapter::new()));
        registry.register(Arc::new(CsdnAdapter::new()));
        registry.register(Arc::new(MediumAdapter::new()));
        registry.register(Arc::new(SubstackAdapter::new()));
        registry.register(Arc::new(ArxivAdapter::new()));
        registry.register(Arc::new(HuggingfaceAdapter::new()));
        registry.register(Arc::new(WikipediaAdapter::new()));
        registry.register(Arc::new(RedditAdapter::new()));
        registry.register(Arc::new(StackoverflowAdapter::new()));
        registry.register(Arc::new(V2exAdapter::new()));
        registry.register(Arc::new(BaikeAdapter::new()));
        registry.register(Arc::new(SkillsAdapter::new()));
        registry
    }

    /// Registers a custom platform adapter.
    pub fn register(&mut self, adapter: Arc<dyn PlatformAdapter>) {
        self.adapters.push(adapter);
    }

    /// Finds the first matching adapter for a URL.
    pub fn find_adapter(&self, url: &Url) -> Option<&dyn PlatformAdapter> {
        self.adapters
            .iter()
            .find(|a| a.matches(url))
            .map(|b| b.as_ref())
    }

    /// Returns structured metadata for all 22 built-in platform adapters.
    pub fn list_adapter_metadata() -> Vec<AdapterInfo> {
        vec![
            AdapterInfo {
                name: "skills",
                title: "Skills.sh (Agent Skills)",
                domain_patterns: &["skills.sh", "www.skills.sh"],
                features: &[
                    "技能详情与折叠正文",
                    "仓库技能列表",
                    "站内搜索",
                    "榜单自动翻页",
                    "结构化技能元数据",
                ],
            },
            AdapterInfo {
                name: "bilibili",
                title: "哔哩哔哩 (Bilibili)",
                domain_patterns: &["bilibili.com", "b23.tv"],
                features: &[
                    "字幕轨提取",
                    "AI大纲总结",
                    "时间轴弹幕",
                    "高赞评论",
                    "封面/视频流下载",
                    "推荐流与排行榜",
                ],
            },
            AdapterInfo {
                name: "youtube",
                title: "YouTube",
                domain_patterns: &["youtube.com", "youtu.be"],
                features: &[
                    "多语言字幕轨(timedtext)",
                    "视频章节时间戳",
                    "频道作者与播放量",
                    "高清简介提纯",
                ],
            },
            AdapterInfo {
                name: "hackernews",
                title: "Hacker News",
                domain_patterns: &["news.ycombinator.com"],
                features: &[
                    "Top极客榜单提取",
                    "官方Firebase API解析",
                    "层级讨论评论树",
                    "直达外链提取",
                ],
            },
            AdapterInfo {
                name: "discourse",
                title: "Discourse / linux.do",
                domain_patterns: &[
                    "linux.do",
                    "users.rust-lang.org",
                    "community.openai.com",
                    "meta.discourse.org",
                ],
                features: &[
                    "linux.do深度适配",
                    "原生.json API免渲染解析",
                    "楼层回复与点赞结构化",
                    "代码块与外链解包",
                ],
            },
            AdapterInfo {
                name: "twitter",
                title: "X (Twitter)",
                domain_patterns: &["x.com", "twitter.com"],
                features: &[
                    "免鉴权Syndication API",
                    "长推文与讨论串(Threads)",
                    "高清媒体大图/视频",
                    "引用推文嵌套解析",
                    "点赞/转推互动指标",
                ],
            },
            AdapterInfo {
                name: "paperswithcode",
                title: "Papers With Code",
                domain_patterns: &["paperswithcode.com"],
                features: &[
                    "论文摘要与作者列表",
                    "arXiv与PDF直达下载",
                    "GitHub开源实现与Stars",
                    "Benchmark评测SOTA表现",
                ],
            },
            AdapterInfo {
                name: "zhihu",
                title: "知乎 (Zhihu)",
                domain_patterns: &["zhihu.com", "zhuanlan.zhihu.com"],
                features: &[
                    "防外链跳转解包",
                    "高清原图替换",
                    "LaTeX公式规范化",
                    "反爬挑战探测",
                ],
            },
            AdapterInfo {
                name: "wechat",
                title: "微信公众号 (WeChat)",
                domain_patterns: &["mp.weixin.qq.com"],
                features: &["懒加载图片还原", "作者与发布时间解析", "反爬/删除风控探测"],
            },
            AdapterInfo {
                name: "xiaohongshu",
                title: "小红书 (Xiaohongshu)",
                domain_patterns: &["xiaohongshu.com", "xhslink.com"],
                features: &[
                    "笔记正文与话题标签",
                    "无水印多图画廊提取",
                    "服务端状态树解析",
                ],
            },
            AdapterInfo {
                name: "github",
                title: "GitHub",
                domain_patterns: &["github.com"],
                features: &[
                    "README提纯",
                    "Issue/PR讨论链提取",
                    "Release发版说明",
                    "Trending每日/周热榜",
                ],
            },
            AdapterInfo {
                name: "huggingface",
                title: "Hugging Face",
                domain_patterns: &["huggingface.co"],
                features: &[
                    "Model Card模型卡",
                    "数据集正文",
                    "论文Abstract",
                    "YAML元数据",
                ],
            },
            AdapterInfo {
                name: "stackoverflow",
                title: "Stack Overflow",
                domain_patterns: &["stackoverflow.com"],
                features: &["采纳答案标记", "代码与公式规范化", "得票数与作者统计"],
            },
            AdapterInfo {
                name: "v2ex",
                title: "V2EX",
                domain_patterns: &["v2ex.com"],
                features: &["主楼与附言合并", "楼层回复结构化表格"],
            },
            AdapterInfo {
                name: "reddit",
                title: "Reddit",
                domain_patterns: &["reddit.com"],
                features: &["帖子正文还原", "评论树递归扁平化", "JSON接口自适应"],
            },
            AdapterInfo {
                name: "wikipedia",
                title: "维基百科 (Wikipedia)",
                domain_patterns: &["*.wikipedia.org"],
                features: &[
                    "信息盒Infobox",
                    "正文层级保留",
                    "参考引用清理",
                    "维基表格保留",
                ],
            },
            AdapterInfo {
                name: "baike",
                title: "百度百科 (Baidu Baike)",
                domain_patterns: &["baike.baidu.com"],
                features: &["词条摘要Lemma", "基础信息表结构化", "多义词消歧"],
            },
            AdapterInfo {
                name: "arxiv",
                title: "ArXiv",
                domain_patterns: &["arxiv.org"],
                features: &["论文Abstract", "作者列表", "PDF下载链接", "Atom订阅源"],
            },
            AdapterInfo {
                name: "medium",
                title: "Medium",
                domain_patterns: &["medium.com"],
                features: &["技术博文提纯", "付费弹窗遮罩剥离", "作者与标签"],
            },
            AdapterInfo {
                name: "substack",
                title: "Substack",
                domain_patterns: &["*.substack.com"],
                features: &["专栏博文提纯", "创作者与订阅信息"],
            },
            AdapterInfo {
                name: "csdn",
                title: "CSDN",
                domain_patterns: &["blog.csdn.net"],
                features: &["外链中间页解包", "代码行号剥离", "广告容器清除"],
            },
            AdapterInfo {
                name: "juejin",
                title: "掘金 (Juejin)",
                domain_patterns: &["juejin.cn"],
                features: &["外链跳转解包", "技术排版保真", "公式与代码高亮"],
            },
        ]
    }
}

/// Metadata describing a supported platform adapter and its specialized extraction capabilities.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AdapterInfo {
    pub name: &'static str,
    pub title: &'static str,
    pub domain_patterns: &'static [&'static str],
    pub features: &'static [&'static str],
}

/// Formats a list of AdapterInfo into a clean GitHub Flavored Markdown table.
pub fn format_adapters_markdown(adapters: &[AdapterInfo]) -> String {
    let mut out = String::new();
    out.push_str("# TkLess 原生平台适配器矩阵 (Supported Platforms)\n\n");
    out.push_str(
        "| # | 平台名称 (Platform) | 匹配域名 (Domains) | 核心专属优化能力 (Specialized Features) |\n",
    );
    out.push_str("|---|---|---|---|\n");
    for (idx, a) in adapters.iter().enumerate() {
        out.push_str(&format!(
            "| {} | **{}** | `{}` | {} |\n",
            idx + 1,
            a.title,
            a.domain_patterns.join("`, `"),
            a.features.join("、")
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyAdapter;
    impl PlatformAdapter for DummyAdapter {
        fn name(&self) -> &'static str {
            "dummy"
        }
        fn matches(&self, url: &Url) -> bool {
            url.host_str() == Some("dummy.test")
        }
        fn extract<'a>(
            &'a self,
            _fetcher: &'a crate::fetcher::Fetcher,
            _url: &'a Url,
            _options: &'a ExtractionOptions,
        ) -> BoxFuture<'a, Result<PageDocument, TklessError>> {
            Box::pin(async { Err(TklessError::InvalidArgument("test".into())) })
        }
    }

    #[test]
    fn test_registry_matching() {
        let mut reg = AdapterRegistry::new();
        reg.register(Arc::new(DummyAdapter));

        let matching_url = Url::parse("https://dummy.test/page").unwrap();
        let other_url = Url::parse("https://example.com/").unwrap();

        assert!(reg.find_adapter(&matching_url).is_some());
        assert_eq!(reg.find_adapter(&matching_url).unwrap().name(), "dummy");
        assert!(reg.find_adapter(&other_url).is_none());
    }
}
