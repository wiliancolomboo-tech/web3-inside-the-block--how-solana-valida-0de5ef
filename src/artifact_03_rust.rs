use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionOrderRecord {
    pub slot: u64,
    pub block_index: usize,
    pub signature: String,
    pub fee_lamports: u64,
    pub compute_units_consumed: u64,
    pub compute_units_requested: u32,
    pub micro_lamports_per_cu: u64,
    pub write_locked_accounts: Vec<String>,
    pub is_jito_bundle_candidate: bool,
    pub execution_success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountLockUtilization {
    pub account: String,
    pub total_cu_consumed: u64,
    pub total_transactions: u32,
    pub is_saturated: bool, // > 12M CU limit
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriorityInversionAlert {
    pub slot: u64,
    pub earlier_tx: String,
    pub earlier_fee_per_cu: u64,
    pub later_tx: String,
    pub later_fee_per_cu: u64,
    pub contested_accounts: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockAnalysisReport {
    pub slot: u64,
    pub leader_identity: String,
    pub total_transactions: usize,
    pub total_cu_consumed: u64,
    pub saturated_write_accounts: Vec<AccountLockUtilization>,
    pub priority_inversions_detected: Vec<PriorityInversionAlert>,
    pub detected_jito_bundles: usize,
    pub execution_duration_ms: u128,
}