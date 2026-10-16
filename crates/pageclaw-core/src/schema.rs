//! Schema-Driven Entity Extraction module.
//!
//! Enables LLMs and AI Agents to extract high-precision structured JSON entities
//! directly from webpage DOM using declarative CSS selector mappings, attribute extraction,
//! and nested object/array schemas, drastically cutting token consumption.

use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use url::Url;

use crate::error::PageclawError;

/// Definition of a single field extraction rule.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum FieldDef {
    /// Concise selector string, e.g. "h1.title", "a.link @href", or "[.tag-item]"
    Simple(String),
    /// Detailed configuration object
    Detail(FieldDetail),
}

/// Detailed extraction specification for a field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct FieldDetail {
    /// CSS selector for target element(s)
    pub selector: String,
    /// Extraction target: "text" (default), "html", "attribute", or "regex"
    #[serde(default = "default_extract_kind")]
    pub extract: String,
    /// HTML attribute name when extract == "attribute" (e.g. "href", "src", "data-id")
    pub attribute: Option<String>,
    /// Optional regular expression pattern to capture a specific substring
    pub regex: Option<String>,
    /// Whether to extract all matching elements as a JSON Array
    #[serde(default)]
    pub multiple: bool,
    /// Nested child schema if this element represents a container or array of objects
    pub fields: Option<HashMap<String, FieldDef>>,
    /// Default fallback value if target element is not found
    pub default: Option<Value>,
}

fn default_extract_kind() -> String {
    "text".to_string()
}

/// A complete extraction schema defining root fields and optional list repetition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ExtractionSchema {
    /// Optional schema title/identifier
    pub name: Option<String>,
    /// Root fields mapping
    #[serde(default)]
    pub fields: HashMap<String, FieldDef>,
    /// Optional container selector for repeating list items (e.g. ".product-card", "table tbody tr")
    pub items_selector: Option<String>,
}

impl ExtractionSchema {
    /// Parses a schema from either a JSON string or a file path.
    pub fn parse(input: &str) -> Result<Self, PageclawError> {
        let trimmed = input.trim();
        // If it starts with '{', parse directly as inline JSON
        if trimmed.starts_with('{') {
            serde_json::from_str(trimmed)
                .map_err(|e| PageclawError::SchemaError(format!("解析 JSON 提取 Schema 失败: {e}")))
        } else {
            // Otherwise treat as a file path
            let content = std::fs::read_to_string(trimmed).map_err(|e| {
                PageclawError::SchemaError(format!("读取 Schema 文件 '{trimmed}' 失败: {e}"))
            })?;
            serde_json::from_str(&content)
                .map_err(|e| PageclawError::SchemaError(format!("解析 Schema 文件内容失败: {e}")))
        }
    }
}

/// Extracts structured data from an HTML document using an ExtractionSchema.
pub fn extract_entities(
    html: &str,
    schema: &ExtractionSchema,
    base_url: Option<&str>,
) -> Result<Value, PageclawError> {
    let doc = Html::parse_document(html);
    let parsed_base = base_url.and_then(|u| Url::parse(u).ok());

    // 1. Repeating list items mode
    if let Some(ref items_sel) = schema.items_selector {
        let selector = Selector::parse(items_sel).map_err(|e| {
            PageclawError::SchemaError(format!("无效的 items_selector '{items_sel}': {e:?}"))
        })?;

        let mut items = Vec::new();
        for element in doc.select(&selector) {
            let item_obj =
                extract_fields_from_element(&element, &schema.fields, parsed_base.as_ref())?;
            items.push(item_obj);
        }
        return Ok(Value::Array(items));
    }

    // 2. Object root mode
    let root_element = doc.root_element();
    extract_fields_from_element(&root_element, &schema.fields, parsed_base.as_ref())
}

/// Extracts fields from a DOM element container according to field definitions.
fn extract_fields_from_element(
    container: &ElementRef,
    fields: &HashMap<String, FieldDef>,
    base_url: Option<&Url>,
) -> Result<Value, PageclawError> {
    let mut map = Map::new();

    for (field_name, field_def) in fields {
        let (detail, is_array_shortcut) = normalize_field_def(field_def);
        let extracted_value =
            extract_single_field(container, &detail, is_array_shortcut, base_url)?;
        map.insert(field_name.clone(), extracted_value);
    }

    Ok(Value::Object(map))
}

/// Normalizes a FieldDef into a FieldDetail and checks for array shortcuts.
fn normalize_field_def(def: &FieldDef) -> (FieldDetail, bool) {
    match def {
        FieldDef::Detail(detail) => (detail.clone(), detail.multiple),
        FieldDef::Simple(s) => {
            let trimmed = s.trim();
            // Check for array shortcut e.g. "[li.item]" or ".item[]"
            let (unbracketed, is_array) = if trimmed.starts_with('[') && trimmed.ends_with(']') {
                (&trimmed[1..trimmed.len() - 1], true)
            } else if let Some(stripped) = trimmed.strip_suffix("[]") {
                (stripped, true)
            } else {
                (trimmed, false)
            };

            // Check for attribute shortcut e.g. "a.link @href" or "img @src"
            let (css_sel, attr) = if let Some((css, attr)) = unbracketed.split_once('@') {
                (css.trim().to_string(), Some(attr.trim().to_string()))
            } else {
                (unbracketed.trim().to_string(), None)
            };

            let extract = if attr.is_some() {
                "attribute".to_string()
            } else {
                "text".to_string()
            };

            (
                FieldDetail {
                    selector: css_sel,
                    extract,
                    attribute: attr,
                    regex: None,
                    multiple: is_array,
                    fields: None,
                    default: None,
                },
                is_array,
            )
        }
    }
}

/// Extracts a single field from a DOM container.
fn extract_single_field(
    container: &ElementRef,
    detail: &FieldDetail,
    multiple: bool,
    base_url: Option<&Url>,
) -> Result<Value, PageclawError> {
    if detail.selector.is_empty() {
        return Ok(detail.default.clone().unwrap_or(Value::Null));
    }

    let selector = Selector::parse(&detail.selector).map_err(|e| {
        PageclawError::SchemaError(format!("无效的 CSS 选择器 '{}': {e:?}", detail.selector))
    })?;

    // Nested object schema
    if let Some(ref nested_fields) = detail.fields {
        if multiple {
            let mut list = Vec::new();
            for elem in container.select(&selector) {
                let nested_obj = extract_fields_from_element(&elem, nested_fields, base_url)?;
                list.push(nested_obj);
            }
            return Ok(Value::Array(list));
        } else if let Some(elem) = container.select(&selector).next() {
            return extract_fields_from_element(&elem, nested_fields, base_url);
        } else {
            return Ok(detail.default.clone().unwrap_or(Value::Null));
        }
    }

    // Leaf value extraction (text / attribute / html / regex)
    if multiple {
        let mut list = Vec::new();
        for elem in container.select(&selector) {
            if let Some(val) = extract_leaf_value(&elem, detail, base_url) {
                list.push(Value::String(val));
            }
        }
        Ok(Value::Array(list))
    } else if let Some(elem) = container.select(&selector).next() {
        if let Some(val) = extract_leaf_value(&elem, detail, base_url) {
            Ok(Value::String(val))
        } else {
            Ok(detail.default.clone().unwrap_or(Value::Null))
        }
    } else {
        Ok(detail.default.clone().unwrap_or(Value::Null))
    }
}

/// Extracts the string value from a matched DOM element.
fn extract_leaf_value(
    elem: &ElementRef,
    detail: &FieldDetail,
    base_url: Option<&Url>,
) -> Option<String> {
    let raw = match detail.extract.as_str() {
        "html" => elem.html(),
        "attribute" | "attr" => {
            let attr_name = detail.attribute.as_deref().unwrap_or("href");
            let attr_val = elem.value().attr(attr_name)?;
            // Automatically resolve relative URLs for link/source attributes
            if let Some(base) = base_url {
                if attr_name == "href" || attr_name == "src" || attr_name == "data-src" {
                    if let Ok(resolved) = base.join(attr_val) {
                        return Some(resolved.to_string());
                    }
                }
            }
            attr_val.to_string()
        }
        _ => {
            // Text content with whitespace trimmed
            elem.text().collect::<Vec<_>>().join(" ").trim().to_string()
        }
    };

    if let Some(ref re_pattern) = detail.regex {
        if let Ok(re) = Regex::new(re_pattern) {
            if let Some(caps) = re.captures(&raw) {
                if let Some(matched) = caps.get(1).or_else(|| caps.get(0)) {
                    return Some(matched.as_str().to_string());
                }
            }
        }
    }

    Some(raw)
}

/// Analysis report containing DOM structural skeleton and suggested extraction schema.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScaffoldReport {
    /// Detected repeating list container CSS selector, if any
    pub suggested_items_selector: Option<String>,
    /// Number of elements found matching the repeating container
    pub items_count: usize,
    /// Suggested schema template
    pub suggested_schema: ExtractionSchema,
    /// Markdown-formatted human/agent readable summary
    pub summary_markdown: String,
}

/// Scaffolds and analyzes the DOM structure of an HTML page, detecting repeating lists
/// and generating a suggested ExtractionSchema template to save AI Agent tokens.
pub fn scaffold_dom_skeleton(html: &str) -> ScaffoldReport {
    let doc = Html::parse_document(html);

    // 1. Analyze candidate repeating containers
    let mut candidate_counts: HashMap<String, usize> = HashMap::new();
    let mut candidate_scores: HashMap<String, i32> = HashMap::new();

    let all_elements_sel = Selector::parse("*[class]").unwrap();
    for el in doc.select(&all_elements_sel) {
        let tag = el.value().name();
        // Ignore noise tags
        if tag == "script"
            || tag == "style"
            || tag == "svg"
            || tag == "path"
            || tag == "span"
            || tag == "i"
        {
            continue;
        }

        if let Some(class_attr) = el.value().attr("class") {
            for cls in class_attr.split_whitespace() {
                let trimmed = cls.trim();
                // Filter out utility classes like tailwind or too short classes
                if trimmed.len() < 3 || trimmed.starts_with('_') {
                    continue;
                }
                let selector_key = format!("{tag}.{trimmed}");
                *candidate_counts.entry(selector_key).or_insert(0) += 1;
            }
        }
    }

    // Score candidates based on count and child diversity
    for (sel_str, count) in &candidate_counts {
        if *count >= 2 && *count <= 500 {
            if let Ok(sel) = Selector::parse(sel_str) {
                let mut score = *count as i32;
                if let Some(first) = doc.select(&sel).next() {
                    // Check meaningful children
                    for child in first.children() {
                        if let Some(child_elem) = ElementRef::wrap(child) {
                            let name = child_elem.value().name();
                            if name.starts_with('h') {
                                score += 30;
                            } else if name == "a" {
                                score += 20;
                            } else if name == "img" {
                                score += 15;
                            } else if name == "p" {
                                score += 10;
                            }
                        }
                    }
                }
                candidate_scores.insert(sel_str.clone(), score);
            }
        }
    }

    // Pick the best repeating candidate
    let best_repeating = candidate_scores
        .into_iter()
        .max_by_key(|(_, score)| *score)
        .filter(|(_, score)| *score >= 15);

    let mut suggested_fields = HashMap::new();
    let mut suggested_items_selector = None;
    let mut items_count = 0;

    if let Some((repeating_sel, _)) = best_repeating {
        if let Ok(sel) = Selector::parse(&repeating_sel) {
            items_count = doc.select(&sel).count();
            suggested_items_selector = Some(repeating_sel.clone());

            // Inspect the first matching item to discover internal fields
            if let Some(item_elem) = doc.select(&sel).next() {
                // Discover title
                if let Some(title_sel) = find_child_matching(
                    &item_elem,
                    &[
                        "h1",
                        "h2",
                        "h3",
                        "h4",
                        "[class*='title']",
                        "[class*='name']",
                        "a",
                    ],
                ) {
                    suggested_fields.insert("title".to_string(), FieldDef::Simple(title_sel));
                }
                // Discover link
                if let Some(link_sel) = find_child_matching(
                    &item_elem,
                    &["a[href]", "[class*='link']", "[class*='url']"],
                ) {
                    suggested_fields.insert(
                        "url".to_string(),
                        FieldDef::Simple(format!("{link_sel} @href")),
                    );
                }
                // Discover price
                if let Some(price_sel) = find_child_matching(
                    &item_elem,
                    &[
                        "[class*='price']",
                        "[class*='amount']",
                        "[class*='cost']",
                        ".money",
                    ],
                ) {
                    suggested_fields.insert("price".to_string(), FieldDef::Simple(price_sel));
                }
                // Discover image
                if let Some(img_sel) = find_child_matching(
                    &item_elem,
                    &[
                        "img[src]",
                        "img[data-src]",
                        "[class*='cover'] img",
                        "[class*='thumb'] img",
                    ],
                ) {
                    suggested_fields.insert(
                        "image".to_string(),
                        FieldDef::Simple(format!("{img_sel} @src")),
                    );
                }
                // Discover date
                if let Some(date_sel) =
                    find_child_matching(&item_elem, &["time", "[class*='date']", "[class*='time']"])
                {
                    suggested_fields.insert("date".to_string(), FieldDef::Simple(date_sel));
                }
                // Discover description
                if let Some(desc_sel) = find_child_matching(
                    &item_elem,
                    &[
                        "p",
                        "[class*='desc']",
                        "[class*='summary']",
                        "[class*='abstract']",
                    ],
                ) {
                    suggested_fields.insert("summary".to_string(), FieldDef::Simple(desc_sel));
                }
            }
        }
    } else {
        // Fallback to single page root object scaffolding
        if Selector::parse("h1")
            .ok()
            .and_then(|s| doc.select(&s).next())
            .is_some()
        {
            suggested_fields.insert("title".to_string(), FieldDef::Simple("h1".to_string()));
        }
        if Selector::parse("article")
            .ok()
            .and_then(|s| doc.select(&s).next())
            .is_some()
        {
            suggested_fields.insert(
                "content".to_string(),
                FieldDef::Simple("article".to_string()),
            );
        } else if Selector::parse(".content")
            .ok()
            .and_then(|s| doc.select(&s).next())
            .is_some()
        {
            suggested_fields.insert(
                "content".to_string(),
                FieldDef::Simple(".content".to_string()),
            );
        }
        if Selector::parse("time")
            .ok()
            .and_then(|s| doc.select(&s).next())
            .is_some()
        {
            suggested_fields.insert("date".to_string(), FieldDef::Simple("time".to_string()));
        }
        if Selector::parse("[class*='author']")
            .ok()
            .and_then(|s| doc.select(&s).next())
            .is_some()
        {
            suggested_fields.insert(
                "author".to_string(),
                FieldDef::Simple("[class*='author']".to_string()),
            );
        }
    }

    let suggested_schema = ExtractionSchema {
        name: Some("auto_scaffolded_schema".to_string()),
        items_selector: suggested_items_selector.clone(),
        fields: suggested_fields.clone(),
    };

    let schema_json = serde_json::to_string_pretty(&suggested_schema).unwrap_or_default();

    let mut summary = String::from(
        "### 页面 DOM 结构骨架与建议抽取 Schema (DOM Skeleton & Suggested Schema)\n\n",
    );
    if let Some(ref items_sel) = suggested_items_selector {
        summary.push_str(&format!(
            "- **探测到重复列表容器**: `{items_sel}` (匹配到 {items_count} 项条目)\n"
        ));
    } else {
        summary.push_str("- **页面类型**: 单页结构对象 (未探测到显著的重复列表容器)\n");
    }

    summary.push_str("- **建议抽取的字段映射清单**:\n");
    for (k, v) in &suggested_fields {
        match v {
            FieldDef::Simple(sel) => summary.push_str(&format!("  - `{k}`: `{sel}`\n")),
            FieldDef::Detail(d) => summary.push_str(&format!("  - `{k}`: `{}`\n", d.selector)),
        }
    }

    summary.push_str("\n#### 建议使用的 Schema 模版 (Ready-to-use JSON Schema):\n\n```json\n");
    summary.push_str(&schema_json);
    summary.push_str("\n```\n\n> 💡 提示: 可直接通过 `pageclaw <URL> --schema '<JSON>'` 或 MCP `fetch_webpage(..., schema=...)` 极速抽取纯净结构化数据。");

    ScaffoldReport {
        suggested_items_selector,
        items_count,
        suggested_schema,
        summary_markdown: summary,
    }
}

/// Helper function to find the first matching child selector among candidate patterns.
fn find_child_matching(container: &ElementRef, candidate_patterns: &[&str]) -> Option<String> {
    for pattern in candidate_patterns {
        if let Ok(sel) = Selector::parse(pattern) {
            if let Some(matched) = container.select(&sel).next() {
                // Build a simple relative CSS selector
                let tag = matched.value().name();
                if let Some(cls) = matched.value().attr("class") {
                    if let Some(first_cls) = cls.split_whitespace().next() {
                        if first_cls.len() >= 3 {
                            return Some(format!("{tag}.{first_cls}"));
                        }
                    }
                }
                return Some(tag.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_object_extraction() {
        let html = r#"
        <div class="product">
            <h1 class="title">  超级机械键盘 Pro  </h1>
            <div class="price-box">
                <span class="currency">¥</span>
                <span class="amount">699.00</span>
            </div>
            <a class="buy-link" href="/buy/12345">立即购买</a>
            <ul class="tags">
                <li class="tag">机械轴</li>
                <li class="tag">RGB 背光</li>
                <li class="tag">无线双模</li>
            </ul>
        </div>
        "#;

        let schema_json = r#"
        {
            "fields": {
                "title": "h1.title",
                "price": ".price-box .amount",
                "buy_url": "a.buy-link @href",
                "tags": "[ul.tags li.tag]"
            }
        }
        "#;

        let schema = ExtractionSchema::parse(schema_json).unwrap();
        let entity =
            extract_entities(html, &schema, Some("https://mall.example.com/item/1")).unwrap();

        assert_eq!(entity["title"], "超级机械键盘 Pro");
        assert_eq!(entity["price"], "699.00");
        assert_eq!(entity["buy_url"], "https://mall.example.com/buy/12345");
        let tags = entity["tags"].as_array().unwrap();
        assert_eq!(tags.len(), 3);
        assert_eq!(tags[0], "机械轴");
        assert_eq!(tags[1], "RGB 背光");
        assert_eq!(tags[2], "无线双模");
    }

    #[test]
    fn test_schema_repeating_items_extraction() {
        let html = r#"
        <div class="news-list">
            <article class="post">
                <h2 class="heading">Rust 1.85 发布</h2>
                <span class="author">Alex</span>
                <a class="link" href="/posts/rust-185">阅读</a>
            </article>
            <article class="post">
                <h2 class="heading">Tokio 异步最佳实践</h2>
                <span class="author">Bob</span>
                <a class="link" href="/posts/tokio-practices">阅读</a>
            </article>
        </div>
        "#;

        let schema_json = r#"
        {
            "items_selector": "article.post",
            "fields": {
                "title": "h2.heading",
                "author": ".author",
                "url": "a.link @href"
            }
        }
        "#;

        let schema = ExtractionSchema::parse(schema_json).unwrap();
        let entities = extract_entities(html, &schema, Some("https://news.example.com/")).unwrap();
        let list = entities.as_array().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0]["title"], "Rust 1.85 发布");
        assert_eq!(list[0]["author"], "Alex");
        assert_eq!(list[0]["url"], "https://news.example.com/posts/rust-185");
        assert_eq!(list[1]["title"], "Tokio 异步最佳实践");
    }

    #[test]
    fn test_schema_regex_extraction() {
        let html = r#"<div class="rating">用户评分：4.9 分（共 1200 人评价）</div>"#;

        let schema_json = r#"
        {
            "fields": {
                "score": {
                    "selector": ".rating",
                    "extract": "text",
                    "regex": "([0-9]+\\.[0-9]+)"
                }
            }
        }
        "#;

        let schema = ExtractionSchema::parse(schema_json).unwrap();
        let entity = extract_entities(html, &schema, None).unwrap();
        assert_eq!(entity["score"], "4.9");
    }

    #[test]
    fn test_scaffold_dom_skeleton() {
        let html = r#"
        <div class="product-grid">
            <div class="product-item">
                <h3 class="product-title">无线降噪耳机</h3>
                <span class="price-val">¥1299</span>
                <a class="item-link" href="/item/101">详情</a>
                <img class="item-thumb" src="/img/101.jpg" alt="耳机">
            </div>
            <div class="product-item">
                <h3 class="product-title">机械键盘</h3>
                <span class="price-val">¥699</span>
                <a class="item-link" href="/item/102">详情</a>
                <img class="item-thumb" src="/img/102.jpg" alt="键盘">
            </div>
            <div class="product-item">
                <h3 class="product-title">人体工学鼠标</h3>
                <span class="price-val">¥399</span>
                <a class="item-link" href="/item/103">详情</a>
                <img class="item-thumb" src="/img/103.jpg" alt="鼠标">
            </div>
        </div>
        "#;

        let report = scaffold_dom_skeleton(html);
        assert_eq!(report.items_count, 3);
        assert!(report
            .suggested_items_selector
            .as_deref()
            .unwrap()
            .contains("product-item"));
        let schema = report.suggested_schema;
        assert_eq!(schema.items_selector, report.suggested_items_selector);
        assert!(schema.fields.contains_key("title"));
        assert!(schema.fields.contains_key("price"));
        assert!(schema.fields.contains_key("url"));
        assert!(schema.fields.contains_key("image"));
        assert!(report.summary_markdown.contains("product-item"));
        assert!(report.summary_markdown.contains("```json"));
    }
}
