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
}

impl SkillBrowser {
    pub fn new() -> Self {
        Self {
            skills: Vec::new(),
            selected: 0,
            loading: false,
            error: None,
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
    ) {
        self.loading = true;
        let url = format!("{}/v1/skills/search", daemon_url.trim_end_matches('/'));
        let body = serde_json::json!({"capability": query, "keywords": [query], "platform": "macos", "purrcode_version": "0.1.0"});
        let req = client.post(&url).bearer_auth(token).json(&body);
        match req.send().await {
            Ok(resp) => {
                if let Ok(val) = resp.json::<Value>().await {
                    self.skills = parse_candidates(&val);
                    self.error = None;
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
