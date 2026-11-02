mod analyzer;
mod integrations;
mod types;

use analyzer::BlockOrderingAnalyzer;
use clap::Parser;
use integrations::EnterpriseAlertDispatcher;
use solana_client::rpc_client::RpcClient;
use solana_client::rpc_config::RpcBlockConfig;
use solana_transaction_status::{TransactionDetails, UiTransactionEncoding};
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, Level};
use tracing_subscriber::FmtSubscriber;

#[derive(Parser, Debug)]
#[command(author, version, about = "Solana Validator Transaction Ordering Inspector & Forensic Monitor")]
struct Cli {
    #[arg(short, long, env = "SOLANA_RPC_URL", default_value = "https://api.mainnet-beta.solana.com")]
    rpc_url: String,

    #[arg(short, long, env = "START_SLOT")]
    slot: Option<u64>,

    #[arg(long, env = "SLACK_WEBHOOK_URL")]
    slack_webhook: Option<String>,

    #[arg(long, env = "DISCORD_WEBHOOK_URL")]
    discord_webhook: Option<String>,

    #[arg(long, env = "RESEND_API_KEY")]
    resend_api_key: Option<String>,

    #[arg(long, env = "RESEND_RECIPIENT")]
    resend_recipient: Option<String>,

    #[arg(long, default_value_t = false)]
    continuous: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    let cli = Cli::parse();
    info!("Starting Solana Validator Block Ordering Inspector...");
    info!("RPC Endpoint: {}", cli.rpc_url);

    let rpc_client = Arc::new(RpcClient::new_with_timeout(cli.rpc_url.clone(), Duration::from_secs(30)));
    let dispatcher = EnterpriseAlertDispatcher::new(
        cli.slack_webhook,
        cli.discord_webhook,
        cli.resend_api_key,
        cli.resend_recipient,
    );

    let mut current_slot = match cli.slot {
        Some(s) => s,
        None => rpc_client.get_slot()?,
    };

    loop {
        info!("Fetching and analyzing Slot #{}...", current_slot);

        let block_config = RpcBlockConfig {
            encoding: Some(UiTransactionEncoding::Json),
            transaction_details: Some(TransactionDetails::Full),
            rewards: Some(true),
            commitment: None,
            max_supported_transaction_version: Some(0),
        };

        match rpc_client.get_block_with_config(current_slot, block_config) {
            Ok(block) => {
                match BlockOrderingAnalyzer::parse_block(current_slot, &block) {
                    Ok(report) => {
                        info!(
                            "Slot #{} Processed: {} txs, {} CU, {} saturated write accounts, {} inversions in {} ms",
                            report.slot,
                            report.total_transactions,
                            report.total_cu_consumed,
                            report.saturated_write_accounts.len(),
                            report.priority_inversions_detected.len(),
                            report.execution_duration_ms
                        );

                        if let Err(e) = dispatcher.dispatch_block_report(&report).await {
                            error!("Error dispatching alert: {:?}", e);
                        }
                    }
                    Err(e) => {
                        error!("Failed to parse block {}: {:?}", current_slot, e);
                    }
                }
            }
            Err(e) => {
                info!("Slot {} was skipped or not yet available: {:?}", current_slot, e);
            }
        }

        if !cli.continuous {
            break;
        }

        current_slot += 1;
        tokio::time::sleep(Duration::from_millis(450)).await; // Match Solana slot pace
    }

    Ok(())
}