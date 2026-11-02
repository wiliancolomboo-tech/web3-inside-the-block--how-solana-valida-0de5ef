use crate::types::BlockAnalysisReport;
use anyhow::{Context, Result};
use reqwest::Client;
use serde_json::json;
use tracing::{error, info};

pub struct EnterpriseAlertDispatcher {
    http_client: Client,
    slack_webhook_url: Option<String>,
    discord_webhook_url: Option<String>,
    resend_api_key: Option<String>,
    resend_recipient: Option<String>,
}

impl EnterpriseAlertDispatcher {
    pub fn new(
        slack_webhook_url: Option<String>,
        discord_webhook_url: Option<String>,
        resend_api_key: Option<String>,
        resend_recipient: Option<String>,
    ) -> Self {
        Self {
            http_client: Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap(),
            slack_webhook_url,
            discord_webhook_url,
            resend_api_key,
            resend_recipient,
        }
    }

    pub async fn dispatch_block_report(&self, report: &BlockAnalysisReport) -> Result<()> {
        let has_saturation = report.saturated_write_accounts.iter().any(|a| a.is_saturated);
        let has_inversions = !report.priority_inversions_detected.is_empty();

        if !has_saturation && !has_inversions {
            return Ok(());
        }

        let message = format!(
            "🚨 [Solana Slot Forensics] Slot #{} Leader: `{}`\n\
            • Total Txs: {} | Consumed CU: {} / 48M ({}%)\n\
            • Saturated Write Locks (>12M CU): {}\n\
            • Priority Inversions Detected: {}\n\
            • Jito Bundles Executed: {}",
            report.slot,
            report.leader_identity,
            report.total_transactions,
            report.total_cu_consumed,
            (report.total_cu_consumed * 100) / 48_000_000,
            report.saturated_write_accounts.len(),
            report.priority_inversions_detected.len(),
            report.detected_jito_bundles,
        );

        // 1. Dispatch to Slack Webhook
        if let Some(url) = &self.slack_webhook_url {
            let payload = json!({ "text": message });
            let _ = self.http_client.post(url).json(&payload).send().await;
            info!(target: "dispatcher", "Dispatched Slack alert for slot {}", report.slot);
        }

        // 2. Dispatch to Discord Webhook
        if let Some(url) = &self.discord_webhook_url {
            let payload = json!({ "content": message });
            let _ = self.http_client.post(url).json(&payload).send().await;
            info!(target: "dispatcher", "Dispatched Discord alert for slot {}", report.slot);
        }

        // 3. Dispatch to Resend Enterprise Email API
        if let (Some(api_key), Some(recipient)) = (&self.resend_api_key, &self.resend_recipient) {
            let html_body = format!(
                "<h3>Solana Block Ordering Forensic Report - Slot #{}</h3>\
                <p><b>Leader:</b> <code>{}</code></p>\
                <p><b>Block Compute Units:</b> {}</p>\
                <h4>Saturated Accounts (>12M CU write limit):</h4>\
                <ul>{}</ul>\
                <h4>Sample Priority Inversion Incidents:</h4>\
                <ul>{}</ul>",
                report.slot,
                report.leader_identity,
                report.total_cu_consumed,
                report.saturated_write_accounts.iter().map(|a| format!("<li>Account: <code>{}</code> ({} CU)</li>", a.account, a.total_cu_consumed)).collect::<Vec<_>>().join(""),
                report.priority_inversions_detected.iter().take(3).map(|i| format!("<li>Reason: <b>{}</b> | Late Tx: <code>{}</code> ({} uLamports/CU) vs Early: <code>{}</code> ({} uLamports/CU)</li>", i.reason, i.later_tx, i.later_fee_per_cu, i.earlier_tx, i.earlier_fee_per_cu)).collect::<Vec<_>>().join("")
            );

            let resend_payload = json!({
                "from": "alerts@Claude Opus 5-solana.internal",
                "to": [recipient],
                "subject": format!("CRITICAL: Solana Slot #{} Contention & Reorder Alert", report.slot),
                "html": html_body
            });

            let resp = self.http_client
                .post("https://api.resend.com/emails")
                .bearer_auth(api_key)
                .json(&resend_payload)
                .send()
                .await;

            match resp {
                Ok(r) if r.status().is_success() => {
                    info!(target: "dispatcher", "Dispatched Resend enterprise email for slot {}", report.slot);
                }
                Ok(r) => {
                    error!(target: "dispatcher", "Resend API returned error status: {}", r.status());
                }
                Err(e) => {
                    error!(target: "dispatcher", "Failed to invoke Resend API: {:?}", e);
                }
            }
        }

        Ok(())
    }
}