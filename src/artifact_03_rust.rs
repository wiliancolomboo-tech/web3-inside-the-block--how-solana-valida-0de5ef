use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use std::cmp::Ordering;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountAccessList {
    pub writable_accounts: Vec<Pubkey>,
    pub readonly_accounts: Vec<Pubkey>,
}

#[derive(Debug, Clone)]
pub struct SimulatedTransaction {
    pub signature: Signature,
    pub payer: Pubkey,
    pub access_list: AccountAccessList,
    pub compute_units_requested: u32,
    pub compute_unit_price_micro_lamports: u64, // Priority fee rate
    pub is_jito_bundle: bool,
    pub jito_tip_lamports: u64,
    pub simulated_execution_cost_units: u32,
}

impl SimulatedTransaction {
    pub fn get_total_priority_fee(&self) -> u64 {
        ((self.compute_units_requested as u128 * self.compute_unit_price_micro_lamports as u128)
            / 1_000_000) as u64
    }

    pub fn priority_score(&self) -> u128 {
        if self.is_jito_bundle {
            // Jito tips are prioritized above normal fee-market transactions
            u128::MAX - 1_000_000_000 + (self.jito_tip_lamports as u128)
        } else {
            self.compute_unit_price_micro_lamports as u128
        }
    }
}

impl PartialEq for SimulatedTransaction {
    fn eq(&self, other: &Self) -> bool {
        self.signature == other.signature
    }
}

impl Eq for SimulatedTransaction {}

impl PartialOrd for SimulatedTransaction {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SimulatedTransaction {
    fn cmp(&self, other: &Self) -> Ordering {
        self.priority_score()
            .cmp(&other.priority_score())
            .then_with(|| other.signature.cmp(&self.signature))
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ScheduledExecutionRecord {
    pub slot: u64,
    pub thread_id: usize,
    pub batch_id: u32,
    pub signature: String,
    pub compute_units_consumed: u32,
    pub priority_fee_micro_lamports: u64,
    pub is_bundle: bool,
    pub write_locks: Vec<String>,
}