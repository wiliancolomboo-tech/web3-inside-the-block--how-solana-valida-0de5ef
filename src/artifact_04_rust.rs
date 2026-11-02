use crate::types::*;
use anyhow::{Context, Result};
use solana_sdk::compute_budget::ComputeBudgetInstruction;
use solana_transaction_status::{
    EncodedConfirmedBlock, EncodedTransactionWithStatusMeta, UiInstruction, UiMessage, UiParsedInstruction,
};
use std::collections::{HashMap, HashSet};

pub const MAX_BLOCK_COMPUTE_UNITS: u64 = 48_000_000;
pub const MAX_ACCOUNT_COMPUTE_UNITS: u64 = 12_000_000;
pub const JITO_TIP_PAYMENT_ACCOUNTS: [&str; 8] = [
    "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5",
    "HFqU5x63VTxvQssQn1WnQQ8KQuJa4C5JUMNYh3VDM1vo",
    "Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY",
    "ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt6iGPaS49",
    "DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh",
    "ADuUkR4vqLUMWXxW9gh6D6L8pWHLHzDvf6mAoTM36x3q",
    "DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL",
    "3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT",
];

pub struct BlockOrderingAnalyzer;

impl BlockOrderingAnalyzer {
    pub fn parse_block(slot: u64, block: &EncodedConfirmedBlock) -> Result<BlockAnalysisReport> {
        let start_time = std::time::Instant::now();
        let leader = block.rewards.as_ref()
            .and_then(|r| r.iter().find(|x| x.reward_type.as_deref() == Some("Fee")))
            .map(|x| x.pubkey.clone())
            .unwrap_or_else(|| "Unknown-Leader".to_string());

        let mut ordered_records: Vec<TransactionOrderRecord> = Vec::new();
        let mut account_usage: HashMap<String, (u64, u32)> = HashMap::new();
        let mut total_block_cu: u64 = 0;

        for (idx, tx_status) in block.transactions.iter().enumerate() {
            if let Some(record) = Self::extract_transaction_record(slot, idx, tx_status) {
                total_block_cu += record.compute_units_consumed;

                for acc in &record.write_locked_accounts {
                    let entry = account_usage.entry(acc.clone()).or_insert((0, 0));
                    entry.0 += record.compute_units_consumed;
                    entry.1 += 1;
                }

                ordered_records.push(record);
            }
        }

        // Identify saturated write accounts (> 12M CU)
        let mut saturated_accounts = Vec::new();
        for (account, (cu, count)) in account_usage.into_iter() {
            let is_saturated = cu >= MAX_ACCOUNT_COMPUTE_UNITS;
            if is_saturated || cu > (MAX_ACCOUNT_COMPUTE_UNITS * 8 / 10) {
                saturated_accounts.push(AccountLockUtilization {
                    account,
                    total_cu_consumed: cu,
                    total_transactions: count,
                    is_saturated,
                });
            }
        }

        // Detect Priority Inversions and Jito Bundles
        let priority_inversions = Self::detect_priority_inversions(&ordered_records);
        let detected_bundles = Self::count_jito_bundles(&ordered_records);

        Ok(BlockAnalysisReport {
            slot,
            leader_identity: leader,
            total_transactions: ordered_records.len(),
            total_cu_consumed: total_block_cu,
            saturated_write_accounts: saturated_accounts,
            priority_inversions_detected: priority_inversions,
            detected_jito_bundles: detected_bundles,
            execution_duration_ms: start_time.elapsed().as_millis(),
        })
    }

    fn extract_transaction_record(
        slot: u64,
        block_index: usize,
        tx_with_meta: &EncodedTransactionWithStatusMeta,
    ) -> Option<TransactionOrderRecord> {
        let meta = tx_with_meta.meta.as_ref()?;
        let cu_consumed = meta.compute_units_consumed.unwrap_or(0);
        let fee = meta.fee;
        let success = meta.err.is_none();

        let (signature, write_locked_accounts, requested_cu, micro_lamports) = match &tx_with_meta.transaction {
            solana_transaction_status::EncodedTransaction::Json(ui_tx) => {
                let sig = ui_tx.signatures.first().cloned().unwrap_or_default();
                let mut locks = Vec::new();
                let mut req_cu = 200_000u32; // Default Solana compute budget
                let mut fee_rate = 0u64;

                match &ui_tx.message {
                    UiMessage::Parsed(parsed_msg) => {
                        for acc in &parsed_msg.account_keys {
                            if acc.writable {
                                locks.push(acc.pubkey.clone());
                            }
                        }
                    }
                    UiMessage::Raw(raw_msg) => {
                        for (i, acc) in raw_msg.account_keys.iter().enumerate() {
                            let is_writable = i < (raw_msg.header.num_required_signatures - raw_msg.header.num_readonly_signed_accounts) as usize
                                || (i >= raw_msg.header.num_required_signatures as usize 
                                    && i < (raw_msg.account_keys.len() - raw_msg.header.num_readonly_unsigned_accounts as usize));
                            if is_writable {
                                locks.push(acc.clone());
                            }
                        }
                    }
                }

                // Parse compute budget directives if present
                if let Some(loaded_addresses) = &meta.loaded_addresses {
                    for acc in &loaded_addresses.writable {
                        locks.push(acc.clone());
                    }
                }

                (sig, locks, req_cu, fee_rate)
            }
            _ => return None,
        };

        let fee_per_cu = if requested_cu > 0 {
            (fee.saturating_mul(1_000_000)) / (requested_cu as u64)
        } else {
            0
        };

        // Determine candidate status for Jito Tip payment
        let is_jito = write_locked_accounts.iter().any(|acc| {
            JITO_TIP_PAYMENT_ACCOUNTS.contains(&acc.as_str())
        });

        Some(TransactionOrderRecord {
            slot,
            block_index,
            signature,
            fee_lamports: fee,
            compute_units_consumed: cu_consumed,
            compute_units_requested: requested_cu,
            micro_lamports_per_cu: fee_per_cu,
            write_locked_accounts,
            is_jito_bundle_candidate: is_jito,
            execution_success: success,
        })
    }

    fn detect_priority_inversions(records: &[TransactionOrderRecord]) -> Vec<PriorityInversionAlert> {
        let mut alerts = Vec::new();

        // Slide window over adjacent transactions to spot inverted priority execution
        for i in 0..records.len() {
            for j in (i + 1)..std::cmp::min(i + 15, records.len()) {
                let tx_early = &records[i];
                let tx_late = &records[j];

                // If late transaction paid significantly higher fee per CU (> 5x)
                // but executed later, identify if there was write-lock contention.
                if tx_late.micro_lamports_per_cu > (tx_early.micro_lamports_per_cu * 5)
                    && tx_early.micro_lamports_per_cu > 0
                {
                    let early_locks: HashSet<_> = tx_early.write_locked_accounts.iter().collect();
                    let late_locks: HashSet<_> = tx_late.write_locked_accounts.iter().collect();
                    let shared_locks: Vec<String> = early_locks
                        .intersection(&late_locks)
                        .map(|s| (*s).clone())
                        .collect();

                    let reason = if !shared_locks.is_empty() {
                        "Account Write-Lock Contention Delay".to_string()
                    } else if tx_early.is_jito_bundle_candidate {
                        "Jito Atomic Bundle Priority Inclusion".to_string()
                    } else {
                        "BankingStage Thread Channel Latency / QUIC Buffer Delay".to_string()
                    };

                    alerts.push(PriorityInversionAlert {
                        slot: tx_early.slot,
                        earlier_tx: tx_early.signature.clone(),
                        earlier_fee_per_cu: tx_early.micro_lamports_per_cu,
                        later_tx: tx_late.signature.clone(),
                        later_fee_per_cu: tx_late.micro_lamports_per_cu,
                        contested_accounts: shared_locks,
                        reason,
                    });

                    if alerts.len() >= 10 {
                        return alerts; // Bound maximum alerts per block to prevent alert storming
                    }
                }
            }
        }
        alerts
    }

    fn count_jito_bundles(records: &[TransactionOrderRecord]) -> usize {
        let mut bundles = 0;
        let mut in_bundle = false;

        for record in records {
            if record.is_jito_bundle_candidate {
                bundles += 1;
                in_bundle = false;
            } else if in_bundle {
                // Continuation of candidate bundle
            }
        }
        bundles
    }
}