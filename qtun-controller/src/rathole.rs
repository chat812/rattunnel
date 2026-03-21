use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct ServiceInfo {
    pub name: String,
    pub bind_addr: String,
    pub service_type: String,
    pub state: String,
}

#[derive(Debug, Serialize)]
struct AddRequest {
    bind_addr: String,
    local_addr: String,
    require_approval: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_id: Option<String>,
}

pub struct RatholeClient {
    client: Client,
    base: String,
}

impl RatholeClient {
    pub fn new(base: &str) -> Self {
        Self {
            client: Client::new(),
            base: base.to_string(),
        }
    }

    pub async fn list(&self) -> Result<Vec<ServiceInfo>> {
        let resp = self
            .client
            .get(format!("{}/api/v1/services", self.base))
            .send()
            .await?
            .json::<Vec<ServiceInfo>>()
            .await?;
        Ok(resp)
    }

    pub async fn list_agents(&self) -> Result<Vec<ServiceInfo>> {
        let resp = self
            .client
            .get(format!("{}/api/v1/agents", self.base))
            .send()
            .await?
            .json::<Vec<ServiceInfo>>()
            .await?;
        Ok(resp)
    }

    pub async fn add(&self, name: &str, bind_addr: &str, local_addr: &str, require_approval: bool, agent_id: Option<&str>) -> Result<()> {
        let body = AddRequest {
            bind_addr: bind_addr.to_string(),
            local_addr: local_addr.to_string(),
            require_approval,
            agent_id: agent_id.map(|s| s.to_string()),
        };
        let resp = self
            .client
            .put(format!("{}/api/v1/services/{}", self.base, name))
            .json(&body)
            .send()
            .await?;
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("rathole error: {}", text);
        }
        Ok(())
    }

    pub async fn remove(&self, name: &str) -> Result<()> {
        let resp = self
            .client
            .delete(format!("{}/api/v1/services/{}", self.base, name))
            .send()
            .await?;
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("rathole error: {}", text);
        }
        Ok(())
    }

    pub async fn approve_connection(&self, id: &str) -> Result<()> {
        let resp = self
            .client
            .post(format!("{}/api/v1/pending/{}/approve", self.base, id))
            .send()
            .await?;
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("rathole approve error: {}", text);
        }
        Ok(())
    }

    pub async fn register_agent(&self, agent_id: &str, token: &str) -> Result<()> {
        let body = serde_json::json!({ "token": token });
        let resp = self
            .client
            .put(format!("{}/api/v1/agents/{}", self.base, agent_id))
            .json(&body)
            .send()
            .await?;
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("rathole register agent error: {}", text);
        }
        Ok(())
    }

    pub async fn unregister_agent(&self, agent_id: &str) -> Result<()> {
        let resp = self
            .client
            .delete(format!("{}/api/v1/agents/{}", self.base, agent_id))
            .send()
            .await?;
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("rathole unregister agent error: {}", text);
        }
        Ok(())
    }

    pub async fn create_setup_code(&self, agent_id: &str, token: &str, setup_code: &str, remote_addr: &str) -> Result<()> {
        let body = serde_json::json!({
            "agent_id": agent_id,
            "token": token,
            "setup_code": setup_code,
            "remote_addr": remote_addr,
        });
        let resp = self
            .client
            .post(format!("{}/api/v1/setup", self.base))
            .json(&body)
            .send()
            .await?;
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("rathole setup code error: {}", text);
        }
        Ok(())
    }

    pub async fn clear_approved(&self, service_name: &str) -> Result<()> {
        let resp = self
            .client
            .delete(format!("{}/api/v1/approved/{}", self.base, service_name))
            .send()
            .await?;
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("rathole clear_approved error: {}", text);
        }
        Ok(())
    }

    pub async fn deny_connection(&self, id: &str) -> Result<()> {
        let resp = self
            .client
            .post(format!("{}/api/v1/pending/{}/deny", self.base, id))
            .send()
            .await?;
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("rathole deny error: {}", text);
        }
        Ok(())
    }
}
