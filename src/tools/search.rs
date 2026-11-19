use super::{Tool, ToolResult};
use crate::security::WorkspacePolicy;
use async_trait::async_trait;
use jwalk::WalkDir;
use regex::Regex;
pub struct SearchPathsTool;
#[async_trait]
impl Tool for SearchPathsTool {
    fn name(&self) -> &'static str {
        "search_paths"
    }
    fn description(&self) -> &'static str {
        "Search workspace paths using a regular expression."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","required":["pattern"],"properties":{"pattern":{"type":"string"},"max_results":{"type":"integer"}}})
    }
    async fn call(&self, p: serde_json::Value, policy: &WorkspacePolicy) -> ToolResult {
        let Some(pattern) = p.get("pattern").and_then(|v| v.as_str()) else {
            return ToolResult::error("pattern is required");
        };
        let re = match Regex::new(pattern) {
            Ok(r) => r,
            Err(e) => return ToolResult::error(e.to_string()),
        };
        let limit = p
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(100)
            .min(1000) as usize;
        let mut found = Vec::new();
        for e in WalkDir::new(policy.root()).skip_hidden(false) {
            match e {
                Ok(e) if e.depth() > 0 => {
                    let path = e.path();
                    let rel = path
                        .strip_prefix(policy.root())
                        .unwrap_or(path.as_path())
                        .to_string_lossy()
                        .replace('\\', "/");
                    if re.is_match(&rel) {
                        found.push(rel);
                        if found.len() >= limit {
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
        ToolResult::ok(found.join("\n"))
    }
}
