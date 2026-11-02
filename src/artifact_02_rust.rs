// File: src/engine.rs
//! Production-grade Solana Validator Transaction Ordering and Banking Stage Simulator.
//! Implements strict Sealevel Account Locking, Priority Fee per Compute Unit sorting,
//! Jito-style atomic bundle scheduling, and block entry generation.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

pub const MAX_BLOCK_COMPUTE_UNITS: u64 = 48_000_000;
pub const MAX_ACCOUNT_COMPUTE_UNITS: u64 = 12_000_000;
pub const MAX_TRANSACTION_COMPUTE_UNITS: u64 = 1_400_000;
pub const DEFAULT_CU_PRICE_MICRO_LAMPORTS: u64 = 1;

/// Unique 32-byte cryptographic identifier for accounts and signatures.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Pubkey(pub [u8; 32]);

impl Pubkey {
    pub fn new_from_slice(slice: &[u8]) -> Self {
        let mut key = [0u8; 32];
        let len = slice.len().min(32);
        key[..len].copy_from_slice(&slice[..len]);
        Pubkey(key)
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn short(&self) -> String {
        let hex = self.to_hex();
        format!("{}...{}", &hex[..4], &hex[hex.len() - 4..])
    }
}

impl std::fmt::Debug for Pubkey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.short())
    }
}

/// Transaction metadata specifying account access intent and execution economics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub signature: Pubkey,
    pub payer: Pubkey,
    pub writable_accounts: Vec<Pubkey>,
    pub readonly_accounts: Vec<Pubkey>,
    pub compute_unit_limit: u64,
    pub compute_unit_price: u64, // Micro-lamports per Compute Unit
    pub tip_lamports: u64,       // Jito-style Direct Tip
    pub is_bundle: bool,
    pub bundle_id: Option<String>,
}

impl Transaction {
    pub fn new(
        seed: &str,
        payer: Pubkey,
        writable: Vec<Pubkey>,
        readonly: Vec<Pubkey>,
        cu_limit: u64,
        cu_price: u64,
        tip: u64,
    ) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(seed.as_bytes());
        let digest = hasher.finalize();
        
        Self {
            signature: Pubkey::new_from_slice(&digest),
            payer,
            writable_accounts: writable,
            readonly_accounts: readonly,
            compute_unit_limit: cu_limit.min(MAX_TRANSACTION_COMPUTE_UNITS),
            compute_unit_price: cu_price,
            tip_lamports: tip,
            is_bundle: false,
            bundle_id: None,
        }
    }

    /// Effective priority fee calculation in total lamports.
    pub fn priority_fee_lamports(&self) -> u64 {
        ((self.compute_unit_limit as u128 * self.compute_unit_price as u128) / 1_000_000) as u64
    }

    /// Total economic yield generated for the block leader.
    pub fn total_reward(&self) -> u64 {
        self.priority_fee_lamports() + self.tip_lamports
    }

    /// Economic density used for validator prioritization: micro-lamports per CU.
    pub fn economic_density(&self) -> u64 {
        if self.compute_unit_limit == 0 {
            return 0;
        }
        let total_value_micro_lamports = (self.tip_lamports as u128 * 1_000_000)
            + (self.compute_unit_limit as u128 * self.compute_unit_price as u128);
        (total_value_micro_lamports / self.compute_unit_limit as u128) as u64
    }
}

/// An atomic bundle of transactions (e.g., Jito Block Engine Bundle).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bundle {
    pub id: String,
    pub transactions: Vec<Transaction>,
    pub expected_signer: Pubkey,
}

/// Execution entry inside the block representing sequential PoH ticks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockEntry {
    pub entry_hash: Pubkey,
    pub num_hashes: u64,
    pub transactions: Vec<Pubkey>,
    pub executed_cu: u64,
}

/// Dynamic tracker of active read and write locks across validator threads.
#[derive(Default)]
pub struct AccountLockTable {
    write_locks: HashSet<Pubkey>,
    read_locks: HashMap<Pubkey, usize>,
    account_cu_consumption: HashMap<Pubkey, u64>,
}

impl AccountLockTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Evaluates if a transaction can concurrently lock its declared accounts.
    pub fn can_lock(&self, tx: &Transaction) -> bool {
        // 1. Verify payer writable lock
        if self.write_locks.contains(&tx.payer) || self.read_locks.contains_key(&tx.payer) {
            return false;
        }

        // 2. Check writable account availability
        for acc in &tx.writable_accounts {
            if self.write_locks.contains(acc) || self.read_locks.contains_key(acc) {
                return false;
            }
            // Ensure single-account block CU limit isn't violated
            let current_cu = self.account_cu_consumption.get(acc).copied().unwrap_or(0);
            if current_cu + tx.compute_unit_limit > MAX_ACCOUNT_COMPUTE_UNITS {
                return false;
            }
        }

        // 3. Check read-only account availability (no active write lock)
        for acc in &tx.readonly_accounts {
            if self.write_locks.contains(acc) {
                return false;
            }
        }

        true
    }

    /// Acquires locks for the given transaction.
    pub fn acquire_locks(&mut self, tx: &Transaction) {
        self.write_locks.insert(tx.payer);
        for acc in &tx.writable_accounts {
            self.write_locks.insert(*acc);
            *self.account_cu_consumption.entry(*acc).or_insert(0) += tx.compute_unit_limit;
        }
        for acc in &tx.readonly_accounts {
            *self.read_locks.entry(*acc).or_insert(0) += 1;
        }
    }

    /// Releases locks after execution thread concludes.
    pub fn release_locks(&mut self, tx: &Transaction) {
        self.write_locks.remove(&tx.payer);
        for acc in &tx.writable_accounts {
            self.write_locks.remove(acc);
        }
        for acc in &tx.readonly_accounts {
            if let Some(count) = self.read_locks.get_mut(acc) {
                if *count <= 1 {
                    self.read_locks.remove(acc);
                } else {
                    *count -= 1;
                }
            }
        }
    }

    pub fn get_account_utilization(&self, acc: &Pubkey) -> (u64, f64) {
        let cu = self.account_cu_consumption.get(acc).copied().unwrap_or(0);
        let ratio = cu as f64 / MAX_ACCOUNT_COMPUTE_UNITS as f64;
        (cu, ratio)
    }
}

/// Block Engine state machine handling the full validator ordering and execution lifecycle.
pub struct BlockEngine {
    pub slot: u64,
    pub leader_identity: Pubkey,
    pub total_block_cu: AtomicU64,
    pub executed_transactions: Vec<Transaction>,
    pub entries: Vec<BlockEntry>,
    pub current_poh_hash: Pubkey,
    pub num_threads: usize,
    pub lock_table: AccountLockTable,
}

impl BlockEngine {
    pub fn new(slot: u64, leader_identity: Pubkey, num_threads: usize) -> Self {
        let genesis_poh = Pubkey::new_from_slice(b"SOLANA_GENESIS_POH_HASH_SEED_V1");
        Self {
            slot,
            leader_identity,
            total_block_cu: AtomicU64::new(0),
            executed_transactions: Vec::new(),
            entries: Vec::new(),
            current_poh_hash: genesis_poh,
            num_threads,
            lock_table: AccountLockTable::new(),
        }
    }

    /// Orders an unorganized transaction pool using standard Sealevel priority rules:
    /// Sort descending by Economic Density (Micro-lamports / CU) + FIFO arrival fallback.
    pub fn sort_mempool_vanilla(transactions: &mut [Transaction]) {
        transactions.sort_by(|a, b| {
            b.economic_density()
                .cmp(&a.economic_density())
                .then_with(|| b.tip_lamports.cmp(&a.tip_lamports))
        });
    }

    /// Simulates parallel Banking Stage multi-threaded scheduling across N worker threads.
    pub fn process_vanilla_batch(&mut self, mut pool: Vec<Transaction>) -> Vec<Transaction> {
        Self::sort_mempool_vanilla(&mut pool);
        let mut uncommitted = Vec::new();

        // Process through simulated multi-threaded batches
        let mut batch: Vec<Transaction> = Vec::new();
        let mut batch_cu: u64 = 0;

        for tx in pool {
            let current_total_cu = self.total_block_cu.load(Ordering::Relaxed);
            if current_total_cu + tx.compute_unit_limit > MAX_BLOCK_COMPUTE_UNITS {
                uncommitted.push(tx);
                continue;
            }

            if self.lock_table.can_lock(&tx) {
                self.lock_table.acquire_locks(&tx);
                batch_cu += tx.compute_unit_limit;
                self.total_block_cu.fetch_add(tx.compute_unit_limit, Ordering::Relaxed);
                batch.push(tx);

                // If batch reaches simulated thread capacity, flush to PoH entry
                if batch.len() >= self.num_threads * 4 {
                    self.commit_entry(&mut batch);
                    batch_cu = 0;
                }
            } else {
                // Lock contention: cannot execute concurrently in current execution window
                uncommitted.push(tx);
            }
        }

        if !batch.is_empty() {
            self.commit_entry(&mut batch);
        }

        uncommitted
    }

    /// Ingests and commits an atomic Jito bundle directly into the block ahead of regular transactions,
    /// enforcing all-or-nothing atomicity and account lock validity.
    pub fn process_bundle(&mut self, bundle: Bundle) -> Result<(), &'static str> {
        // 1. Check cumulative bundle CU
        let total_bundle_cu: u64 = bundle.transactions.iter().map(|t| t.compute_unit_limit).sum();
        let current_total = self.total_block_cu.load(Ordering::Relaxed);
        
        if current_total + total_bundle_cu > MAX_BLOCK_COMPUTE_UNITS {
            return Err("Bundle exceeds maximum remaining block compute budget");
        }

        // 2. Validate all-or-nothing account lock compatibility across the bundle
        for tx in &bundle.transactions {
            if !self.lock_table.can_lock(tx) {
                return Err("Account lock conflict detected; atomic bundle rejected");
            }
        }

        // 3. Atomically acquire locks and append
        let mut bundle_txs = bundle.transactions;
        for tx in &bundle_txs {
            self.lock_table.acquire_locks(tx);
            self.total_block_cu.fetch_add(tx.compute_unit_limit, Ordering::Relaxed);
        }

        self.commit_entry(&mut bundle_txs);
        Ok(())
    }

    /// Commits transactions into a Proof of History Entry and advances the continuous SHA-256 hash.
    fn commit_entry(&mut self, txs: &mut Vec<Transaction>) {
        if txs.is_empty() {
            return;
        }

        let mut hasher = Sha256::new();
        hasher.update(self.current_poh_hash.0);

        let mut sigs = Vec::new();
        let mut entry_cu = 0;

        for tx in txs.drain(..) {
            hasher.update(tx.signature.0);
            sigs.push(tx.signature);
            entry_cu += tx.compute_unit_limit;
            
            // Release locks back to the lock table
            self.lock_table.release_locks(&tx);
            self.executed_transactions.push(tx);
        }

        let digest = hasher.finalize();
        let next_poh = Pubkey::new_from_slice(&digest);
        self.current_poh_hash = next_poh;

        self.entries.push(BlockEntry {
            entry_hash: next_poh,
            num_hashes: 128, // Standard Solana entry tick resolution
            transactions: sigs,
            executed_cu: entry_cu,
        });
    }

    /// Aggregates execution statistics and top locked account profiles.
    pub fn get_block_telemetry(&self) -> BlockTelemetryReport {
        let total_reward_lamports: u64 = self.executed_transactions.iter().map(|t| t.total_reward()).sum();
        let total_tip_lamports: u64 = self.executed_transactions.iter().map(|t| t.tip_lamports).sum();
        let total_cu = self.total_block_cu.load(Ordering::Relaxed);

        let mut account_heats: Vec<(Pubkey, u64, f64)> = self
            .lock_table
            .account_cu_consumption
            .iter()
            .map(|(k, v)| (*k, *v, *v as f64 / MAX_ACCOUNT_COMPUTE_UNITS as f64))
            .collect();
        account_heats.sort_by(|a, b| b.1.cmp(&a.1));

        BlockTelemetryReport {
            slot: self.slot,
            leader: self.leader_identity.to_hex(),
            total_transactions: self.executed_transactions.len(),
            total_entries: self.entries.len(),
            total_compute_units: total_cu,
            block_cu_utilization: total_cu as f64 / MAX_BLOCK_COMPUTE_UNITS as f64,
            total_rewards_lamports: total_reward_lamports,
            total_tips_lamports: total_tip_lamports,
            hot_accounts: account_heats.into_iter().take(5).collect(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BlockTelemetryReport {
    pub slot: u64,
    pub leader: String,
    pub total_transactions: usize,
    pub total_entries: usize,
    pub total_compute_units: u64,
    pub block_cu_utilization: f64,
    pub total_rewards_lamports: u64,
    pub total_tips_lamports: u64,
    pub hot_accounts: Vec<(Pubkey, u64, f64)>,
}