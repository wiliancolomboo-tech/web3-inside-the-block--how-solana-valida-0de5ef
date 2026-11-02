use serde::{Deserialize, Serialize};
use std::fmt;

pub type Pubkey = [u8; 32];
pub type Signature = [u8; 64];

pub const MAX_BLOCK_COMPUTE_UNITS: u64 = 48_000_000;
pub const MAX_ACCOUNT_COMPUTE_UNITS: u64 = 12_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccountMeta {
    pub pubkey: Pubkey,
    pub is_signer: bool,
    pub is_writable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    pub signatures: Vec<Signature>,
    pub account_keys: Vec<AccountMeta>,
    pub compute_unit_limit: u64,
    pub compute_unit_price: u64, // In microLamports per CU
    pub tip_lamports: u64,        // Jito bundle direct tip
    pub payload_id: u64,
}

impl Transaction {
    pub fn new(
        account_keys: Vec<AccountMeta>,
        compute_unit_limit: u64,
        compute_unit_price: u64,
        tip_lamports: u64,
        payload_id: u64,
    ) -> Self {
        Self {
            signatures: vec![[0u8; 64]],
            account_keys,
            compute_unit_limit,
            compute_unit_price,
            tip_lamports,
            payload_id,
        }
    }

    /// Calculate effective priority fee score for queue ranking
    pub fn priority_score(&self) -> u128 {
        (self.compute_unit_price as u128)
            .saturating_mul(self.compute_unit_limit as u128)
            .saturating_add(self.tip_lamports as u128)
    }

    pub fn write_accounts(&self) -> Vec<Pubkey> {
        self.account_keys
            .iter()
            .filter(|meta| meta.is_writable)
            .map(|meta| meta.pubkey)
            .collect()
    }

    pub fn read_accounts(&self) -> Vec<Pubkey> {
        self.account_keys
            .iter()
            .filter(|meta| !meta.is_writable)
            .map(|meta| meta.pubkey)
            .collect()
    }
}

impl fmt::Display for Transaction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Tx[id={}, cu_limit={}, cu_price={} uL/CU, tip={} lamports]",
            self.payload_id, self.compute_unit_limit, self.compute_unit_price, self.tip_lamports
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Bundle {
    pub transactions: Vec<Transaction>,
    pub bundle_tip: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub num_hashes: u64,
    pub hash: [u8; 32],
    pub transactions: Vec<Transaction>,
}