//! Shared catalog contracts and validation for declared parameter types and bounds.
use serde_json::{json, Value};

pub(super) fn output_schema(name: &str) -> Option<Value> {
    let warning = json!({"type":"object","required":["code","message"],"properties":{"code":{"type":"string"},"message":{"type":"string"}}});
    let document = json!({"type":"object","required":["url","final_url","status","content","extraction","token_count","truncated"],
    "properties":{
        "url":{"type":"string"},"final_url":{"type":"string"},"status":{"type":"integer"},
        "content":{"type":"string"},"token_count":{"type":"integer"},"truncated":{"type":"boolean"},
        "extraction":{"type":"object","required":["method","transport"],"properties":{"method":{"type":"string"},"transport":{"type":"string"},"adapter":{"type":"string"}}},
        "warnings":{"type":"array","items":warning.clone()}
    }});
    let result_item = json!({"type":"object","required":["title","url","snippet","engine"],"properties":{
        "title":{"type":"string"},"url":{"type":"string"},"snippet":{"type":"string"},"engine":{"const":"bing"}
    }});
    let job = json!({"type":"object","required":["job_id","tool","state","created_at","started_at","finished_at","elapsed_ms","progress","result","error"],"properties":{
        "job_id":{"type":"string"},"tool":{"type":"string"},
        "state":{"enum":["queued","running","completed","failed","cancelled"]},
        "created_at":{"type":"string"},"started_at":{"type":["string","null"]},
        "finished_at":{"type":["string","null"]},"elapsed_ms":{"type":"integer","minimum":0},
        "progress":{"type":"object","required":["completed","total","message"],"properties":{
            "completed":{"type":"integer","minimum":0},"total":{"type":["integer","null"]},
            "message":{"type":["string","null"]}
        }},
        "result":{},"error":{"type":["string","null"]}
    }});
    let success = match name {
        "list_wechat_articles" => {
            json!({"type":"object","required":["source","scope","account_history_complete","listing_complete","complete","articles","stop_reason"],"properties":{
                "source":{"type":"string"},"scope":{"type":"string"},"account_history_complete":{"const":false},
                "listing_complete":{"type":"boolean"},"complete":{"type":"boolean"},"articles":{"type":"array"},"stop_reason":{"type":"string"}
            }})
        }
        "sync_collection" => {
            json!({"type":"object","required":["generation","complete","events","deletions_applied"],"properties":{
                "generation":{"type":"integer"},"complete":{"type":"boolean"},"events":{"type":"array"},"deletions_applied":{"type":"boolean"}
            }})
        }
        "collect_pages" => {
            json!({"type":"object","required":["records","complete","listing_complete","stop_reason","pages_fetched"],"properties":{
                "records":{"type":"array"},"complete":{"type":"boolean"},"listing_complete":{"type":"boolean"},"stop_reason":{"type":"string"},"pages_fetched":{"type":"integer"}
            }})
        }
        "fetch_webpage" | "clean_html" | "browse_page" => {
            json!({"type":"object","required":["document"],"properties":{"document":document}})
        }
        "web_search" => {
            json!({"type":"object","required":["query","effective_query","status","results","diagnostics"],"properties":{
                "query":{"type":"string"},"effective_query":{"type":"string"},"status":{"enum":["results","no_results","filtered"]},
                "results":{"type":"array","items":result_item},"diagnostics":{"type":"array","items":{"type":"object"}},
                "warnings":{"type":"array","items":warning}
            }})
        }
        "batch_fetch_webpages" => {
            let report = json!({"type":"object","required":["total","succeeded","failed","resumed","fetched","results"],"properties":{
                "total":{"type":"integer"},"succeeded":{"type":"integer"},"failed":{"type":"integer"},
                "resumed":{"type":"integer"},"fetched":{"type":"integer"},
                "results":{"type":"array","items":{"type":"object","required":["index","url","document","error","exit_code"],"properties":{
                    "index":{"type":"integer","minimum":0},"url":{"type":"string"},"exit_code":{"type":"integer"},
                    "document":{"anyOf":[document,{"type":"null"}]},"error":{"type":["string","null"]}
                }}}
            }});
            json!({"anyOf":[report,{"type":"object","required":["total","succeeded","failed","resumed","fetched","streamed"],"properties":{
                "total":{"type":"integer"},"succeeded":{"type":"integer"},"failed":{"type":"integer"},
                "resumed":{"type":"integer"},"fetched":{"type":"integer"},"streamed":{"const":true}
            }}]})
        }
        "jobs_start" | "jobs_status" | "jobs_cancel" => job,
        "jobs_list" => {
            json!({"type":"object","required":["total","active","jobs"],"properties":{
                "total":{"type":"integer","minimum":0},"active":{"type":"integer","minimum":0},
                "jobs":{"type":"array","items":job}
            }})
        }
        _ => return None,
    };
    Some(
        json!({"type":"object","anyOf":[success,{"type":"object","required":["error"],"properties":{
            "error":{"type":"object","required":["code","message","retryable","exit_code"],"properties":{
                "code":{"type":"string"},"message":{"type":"string"},"retryable":{"type":"boolean"},
                "exit_code":{"type":"integer"},"retry_after_secs":{"type":"integer","minimum":0}
            }}
        }}]}),
    )
}

/// Validate the subset of JSON Schema used by the built-in catalog. Not a general JSON Schema engine.
pub(super) fn validate_value(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    if let Some(alternatives) = schema.get("anyOf").and_then(Value::as_array) {
        if !alternatives
            .iter()
            .any(|schema| validate_value(schema, value, path).is_ok())
        {
            return Err(format!("{path} does not match any allowed shape"));
        }
    }
    if let Some(types) = schema.get("type") {
        let matches = |kind: &str| match kind {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "boolean" => value.is_boolean(),
            "integer" => value.is_i64() || value.is_u64(),
            "number" => value.is_number(),
            "null" => value.is_null(),
            _ => false,
        };
        let valid = types.as_str().map(matches).unwrap_or_else(|| {
            types
                .as_array()
                .is_some_and(|types| types.iter().filter_map(Value::as_str).any(matches))
        });
        if !valid {
            return Err(format!("{path} has an invalid type; expected {types}"));
        }
    }
    if schema
        .get("const")
        .is_some_and(|expected| expected != value)
    {
        return Err(format!("{path} has an unsupported value"));
    }
    if schema
        .get("enum")
        .and_then(Value::as_array)
        .is_some_and(|choices| !choices.contains(value))
    {
        return Err(format!("{path} has an unsupported value"));
    }
    if let Some(number) = value.as_f64() {
        if schema
            .get("minimum")
            .and_then(Value::as_f64)
            .is_some_and(|min| number < min)
            || schema
                .get("maximum")
                .and_then(Value::as_f64)
                .is_some_and(|max| number > max)
        {
            return Err(format!("{path} is outside the allowed range"));
        }
    }
    if let Some(text) = value.as_str() {
        if schema
            .get("minLength")
            .and_then(Value::as_u64)
            .is_some_and(|min| text.chars().count() < min as usize)
        {
            return Err(format!("{path} is too short"));
        }
    }
    if let Some(object) = value.as_object() {
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for field in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(field) {
                    return Err(format!("Missing required parameter {path}.{field}"));
                }
            }
        }
        for (field, value) in object {
            if let Some(property) = schema.get("properties").and_then(|p| p.get(field)) {
                validate_value(property, value, &format!("{path}.{field}"))?;
            } else if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
                return Err(format!("Unknown argument {field} in {path}"));
            }
        }
    }
    if let Some(array) = value.as_array() {
        if schema
            .get("minItems")
            .and_then(Value::as_u64)
            .is_some_and(|min| array.len() < min as usize)
        {
            return Err(format!("{path} must contain more items"));
        }
        if let Some(items) = schema.get("items") {
            for (index, value) in array.iter().enumerate() {
                validate_value(items, value, &format!("{path}[{index}]"))?;
            }
        }
    }
    Ok(())
}

pub(super) fn validate_arguments(name: &str, args: &Value) -> Result<(), crate::TklessError> {
    let tool = super::tools::get_available_tools()
        .into_iter()
        .chain(super::tools::get_dynamic_discovery_tools())
        .find(|tool| tool.name == name);
    if let Some(tool) = tool {
        validate_value(&tool.input_schema, args, name)
            .map_err(crate::TklessError::InvalidArgument)?;
    }
    if args.get("schema_strict").and_then(Value::as_bool) == Some(true)
        && args.get("schema").is_none()
    {
        return Err(crate::TklessError::InvalidArgument(
            "schema_strict requires schema".into(),
        ));
    }
    if name == "web_search"
        && args.get("fetch_content").and_then(Value::as_bool) != Some(true)
        && (args.get("dedup").and_then(Value::as_bool) == Some(true)
            || args.get("max_tokens_per_page").is_some()
            || args.get("schema").is_some())
    {
        return Err(crate::TklessError::InvalidArgument(
            "dedup, max_tokens_per_page and schema require fetch_content: true".into(),
        ));
    }
    Ok(())
}
