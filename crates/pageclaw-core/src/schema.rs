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
}
