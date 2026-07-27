//! Skill library browser for discovering, inspecting, and installing skills.

use serde_json::Value;

pub struct SkillEntry {
    pub skill_id: String,
    pub version: String,
    pub publisher: String,
    pub source: String,
    pub signature: String,
    pub permissions: String,
    pub network: String,
    pub risk: String,
    pub installed: bool,
}

pub struct SkillBrowser {
    pub skills: Vec<SkillEntry>,
    pub selected: usize,
    pub loading: bool,
    pub error: Option<String>,
    pub pending_search_action: Option<String>,
    pub pending_search_query: Option<String>,
}

impl SkillBrowser {
    pub fn new() -> Self {
        Self {
            skills: Vec::new(),
            selected: 0,
            loading: false,
            error: None,
            pending_search_action: None,
            pending_search_query: None,
        }
    }

    pub async fn load(&mut self, client: &reqwest::Client, daemon_url: &str, token: &str) {
        self.loading = true;
        let url = format!("{}/v1/skills", daemon_url.trim_end_matches('/'));
        let req = client.get(&url).bearer_auth(token);
        match req.send().await {
            Ok(resp) => {
                if let Ok(val) = resp.json::<Value>().await {
                    self.skills = parse_skills(&val);
                    self.error = None;
                }
            }
            Err(e) => {
                self.error = Some(e.to_string());
            }
        }
        self.loading = false;
    }

    pub async fn search(
        &mut self,
        client: &reqwest::Client,
        daemon_url: &str,
        token: &str,
        query: &str,
        session_id: Option<&str>,
        approve_pending: bool,
    ) {
        self.loading = true;
        let url = format!("{}/v1/skills/search", daemon_url.trim_end_matches('/'));
        let action_id = if approve_pending && self.pending_search_query.as_deref() == Some(query) {
            self.pending_search_action.clone()
        } else {
            None
        };
        if approve_pending && action_id.is_none() {
            self.loading = false;
            self.error = Some(
                "No exact pending search matches this query. Run /skills search <query> first."
                    .into(),
            );
            return;
        }
        let body = serde_json::json!({"session_id": session_id, "approved": approve_pending, "action_id": action_id, "capability": query, "keywords": [query], "platform": std::env::consts::OS, "purrcode_version": env!("CARGO_PKG_VERSION")});
        let req = client.post(&url).bearer_auth(token).json(&body);
        match req.send().await {
            Ok(resp) => {
                if let Ok(val) = resp.json::<Value>().await {
                    if val["requires_approval"].as_bool() == Some(true) {
                        self.skills.clear();
                        self.pending_search_action = val["action_id"].as_str().map(str::to_owned);
                        self.pending_search_query = Some(query.to_owned());
                        self.error = Some("Network approval required. Return to chat and use /skill-search-approve with the same query.".into());
                    } else {
                        self.skills = parse_candidates(&val);
                        self.pending_search_action = None;
                        self.pending_search_query = None;
                        self.error = None;
                    }
                }
            }
            Err(e) => {
                self.error = Some(e.to_string());
            }
        }
        self.loading = false;
    }
}

fn parse_skills(val: &Value) -> Vec<SkillEntry> {
    let arr = val.as_array().map(|a| a.to_vec()).unwrap_or_default();
    arr.iter()
        .filter_map(|v| {
            Some(SkillEntry {
                skill_id: v["skill_id"].as_str()?.to_string(),
                version: v["version"].as_str().unwrap_or("?").to_string(),
                publisher: v["publisher"].as_str().unwrap_or("unknown").to_string(),
                source: v["source_type"].as_str().unwrap_or("?").to_string(),
                signature: v["signature_status"]
                    .as_str()
                    .unwrap_or("unavailable")
                    .to_string(),
                permissions: v["approved_permissions"].to_string(),
                network: v["network_access"].as_str().unwrap_or("none").to_string(),
                risk: v["qualification_status"]
                    .as_str()
                    .unwrap_or("unverified")
                    .to_string(),
                installed: true,
            })
        })
        .collect()
}

fn parse_candidates(val: &Value) -> Vec<SkillEntry> {
    let arr = val.as_array().map(|a| a.to_vec()).unwrap_or_default();
    arr.iter()
        .map(|v| {
            let manifest = &v["manifest"];
            SkillEntry {
                skill_id: manifest["name"].as_str().unwrap_or("unknown").to_string(),
                version: manifest["version"].as_str().unwrap_or("?").to_string(),
                publisher: manifest["publisher"]
                    .as_str()
                    .unwrap_or("unknown")
                    .to_string(),
                source: manifest["source_type"].as_str().unwrap_or("?").to_string(),
                signature: manifest["signature_status"]
                    .as_str()
                    .unwrap_or("unavailable")
                    .to_string(),
                permissions: manifest["permissions"].to_string(),
                network: manifest["network_access"]
                    .as_str()
                    .unwrap_or("none")
                    .to_string(),
                risk: format!("{:.1}", v["score"].as_f64().unwrap_or(0.0)),
                installed: false,
            }
        })
        .collect()
}
