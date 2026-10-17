pub mod arxiv;
pub mod baike;
pub mod bilibili;
pub mod csdn;
pub mod github;
pub mod huggingface;
pub mod juejin;
pub mod medium;
pub mod reddit;
pub mod stackoverflow;
pub mod substack;
pub mod v2ex;
pub mod wechat;
pub mod wikipedia;
pub mod xiaohongshu;
pub mod zhihu;

pub use arxiv::ArxivAdapter;
pub use baike::BaikeAdapter;
pub use bilibili::BilibiliAdapter;
pub use csdn::CsdnAdapter;
pub use github::GithubAdapter;
pub use huggingface::HuggingfaceAdapter;
pub use juejin::JuejinAdapter;
pub use medium::MediumAdapter;
pub use reddit::RedditAdapter;
pub use stackoverflow::StackoverflowAdapter;
pub use substack::SubstackAdapter;
pub use v2ex::V2exAdapter;
pub use wechat::WechatAdapter;
pub use wikipedia::WikipediaAdapter;
pub use xiaohongshu::XiaohongshuAdapter;
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
        registry.register(Arc::new(WechatAdapter::new()));
        registry.register(Arc::new(ZhihuAdapter::new()));
        registry.register(Arc::new(XiaohongshuAdapter::new()));
        registry.register(Arc::new(JuejinAdapter::new()));
        registry.register(Arc::new(GithubAdapter::new()));
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
