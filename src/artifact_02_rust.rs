// File: src/engine/mod.rs
use std::collections::{HashMap, HashSet};
use std::cmp::Ordering;
use serde::{Deserialize, Serialize};

pub const MAX_BLOCK_COMPUTE_UNITS: u64 = 48_000_000;
pub const MAX_ACCOUNT_COMPUTE_UNITS: u64 = 12_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Pubkey(pub [u8; 32]);

impl Pubkey {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionMessage {
    pub signature: String,
    pub payer: Pubkey,
    pub writable_accounts: Vec<Pubkey>,
    pub readonly_accounts: Vec<Pubkey>,
    pub compute_units: u64,
    pub priority_fee_micro_lamports: u64, // Micro-lamports per CU
    pub is_jito_bundle: bool,
    pub bundle_id: Option<String>,
}

impl TransactionMessage {
    pub fn calculate_priority_score(&self) -> u128 {
        // Priority Score is directly proportional to micro-lamports per CU
        self.priority_fee_micro_lamports as u128
    }

    pub fn total_fee(&self) -> u64 {
        // Base signature fee (5000 lamports) + priority fee
        let priority_fee = (self.compute_units as u128 * self.priority_fee_micro_lamports as u128 / 1_000_000) as u64;
        5_000 + priority_fee
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderedTransaction {
    pub order_index: usize,
    pub thread_id: usize,
    pub signature: String,
    pub compute_units_consumed: u64,
    pub fee_collected: u64,
    pub is_bundle: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockAssemblySummary {
    pub total_transactions_scheduled: usize,
    pub total_transactions_rejected: usize,
    pub total_compute_units_consumed: u64,
    pub total_fees_collected_lamports: u64,
    pub account_cu_utilization: HashMap<String, u64>,
    pub thread_distribution: HashMap<usize, usize>,
    pub rejections_by_reason: HashMap<String, usize>,
}

#[derive(Debug)]
pub struct BankingScheduler {
    num_threads: usize,
    current_block_cu: u64,
    account_cu_tracker: HashMap<Pubkey, u64>,
    // Thread index -> Set of active write locks
    active_write_locks: HashMap<usize, HashSet<Pubkey>>,
    // Thread index -> Set of active read locks
    active_read_locks: HashMap<usize, HashSet<Pubkey>>,
}

impl BankingScheduler {
    pub fn new(num_threads: usize) -> Self {
        let mut active_write_locks = HashMap::new();
        let mut active_read_locks = HashMap::new();
        for i in 0..num_threads {
            active_write_locks.insert(i, HashSet::new());
            active_read_locks.insert(i, HashSet::new());
        }

        Self {
            num_threads,
            current_block_cu: 0,
            account_cu_tracker: HashMap::new(),
            active_write_locks,
            active_read_locks,
        }
    }

    /// Primary entry point: Packs a block from a pool of incoming transactions and Jito bundles
    pub fn pack_block(
        &mut self,
        mut transactions: Vec<TransactionMessage>,
    ) -> (Vec<OrderedTransaction>, BlockAssemblySummary) {
        let mut scheduled_txs = Vec::new();
        let mut rejected_counts: HashMap<String, usize> = HashMap::new();
        let mut thread_dist: HashMap<usize, usize> = HashMap::new();

        // 1. Separate Jito Bundles from regular transactions
        // Jito bundles maintain strict intra-bundle ordering and are prioritized at slot start
        let mut bundles: HashMap<String, Vec<TransactionMessage>> = HashMap::new();
        let mut standard_txs: Vec<TransactionMessage> = Vec::new();

        for tx in transactions.drain(..) {
            if tx.is_jito_bundle && tx.bundle_id.is_some() {
                let b_id = tx.bundle_id.clone().unwrap();
                bundles.entry(b_id).or_default().push(tx);
            } else {
                standard_txs.push(tx);
            }
        }

        // 2. Sort standard transactions by Priority Score (Micro-lamports / CU) Descending
        standard_txs.sort_by(|a, b| {
            b.calculate_priority_score()
                .cmp(&a.calculate_priority_score())
                .then_with(|| a.signature.cmp(&b.signature))
        });

        // 3. Process Jito Bundles first (Atomic Block-Engine preemption)
        for (_bundle_id, bundle_transactions) in bundles {
            let mut can_schedule_bundle = true;
            let mut bundle_cu_demand = 0;

            // Bundle dry-run validation
            for tx in &bundle_transactions {
                bundle_cu_demand += tx.compute_units;
                if self.current_block_cu + bundle_cu_demand > MAX_BLOCK_COMPUTE_UNITS {
                    can_schedule_bundle = false;
                    *rejected_counts.entry("BlockComputeLimitExceeded".to_string()).or_insert(0) += 1;
                    break;
                }

                // Check Account-Level CU Limits
                for w_acc in &tx.writable_accounts {
                    let current_cu = self.account_cu_tracker.get(w_acc).cloned().unwrap_or(0);
                    if current_cu + tx.compute_units > MAX_ACCOUNT_COMPUTE_UNITS {
                        can_schedule_bundle = false;
                        *rejected_counts.entry("AccountComputeLimitExceeded".to_string()).or_insert(0) += 1;
                        break;
                    }
                }
            }

            if can_schedule_bundle {
                // Pin bundle to an available worker thread atomically
                let assigned_thread = 0; // Jito bundles are prioritized on designated execution thread
                for tx in bundle_transactions {
                    self.commit_transaction(&tx);
                    let order_idx = scheduled_txs.len();
                    *thread_dist.entry(assigned_thread).or_insert(0) += 1;

                    scheduled_txs.push(OrderedTransaction {
                        order_index: order_idx,
                        thread_id: assigned_thread,
                        signature: tx.signature.clone(),
                        compute_units_consumed: tx.compute_units,
                        fee_collected: tx.total_fee(),
                        is_bundle: true,
                    });
                }
            }
        }

        // 4. Schedule Standard Transactions across Parallel Execution Threads
        for tx in standard_txs {
            // Check global block compute budget
            if self.current_block_cu + tx.compute_units > MAX_BLOCK_COMPUTE_UNITS {
                *rejected_counts.entry("BlockComputeLimitExceeded".to_string()).or_insert(0) += 1;
                continue;
            }

            // Check account-level write-limits
            let mut exceeds_account_limit = false;
            for w_acc in &tx.writable_accounts {
                let current_acc_cu = self.account_cu_tracker.get(w_acc).cloned().unwrap_or(0);
                if current_acc_cu + tx.compute_units > MAX_ACCOUNT_COMPUTE_UNITS {
                    exceeds_account_limit = true;
                    break;
                }
            }

            if exceeds_account_limit {
                *rejected_counts.entry("AccountComputeLimitExceeded".to_string()).or_insert(0) += 1;
                continue;
            }

            // Lock contention resolution & thread assignment
            match self.find_available_thread(&tx) {
                Some(thread_id) => {
                    self.commit_transaction(&tx);
                    let order_idx = scheduled_txs.len();
                    *thread_dist.entry(thread_id).or_insert(0) += 1;

                    scheduled_txs.push(OrderedTransaction {
                        order_index: order_idx,
                        thread_id,
                        signature: tx.signature.clone(),
                        compute_units_consumed: tx.compute_units,
                        fee_collected: tx.total_fee(),
                        is_bundle: false,
                    });
                }
                None => {
                    // Contention starvation: transaction conflicts with running write locks across all threads
                    *rejected_counts.entry("LockContentionStarvation".to_string()).or_insert(0) += 1;
                }
            }
        }

        let total_fees = scheduled_txs.iter().map(|t| t.fee_collected).sum();
        let total_cu = self.current_block_cu;
        let mut account_cu_formatted = HashMap::new();
        for (k, v) in &self.account_cu_tracker {
            account_cu_formatted.insert(k.to_hex(), *v);
        }

        let summary = BlockAssemblySummary {
            total_transactions_scheduled: scheduled_txs.len(),
            total_transactions_rejected: rejected_counts.values().sum(),
            total_compute_units_consumed: total_cu,
            total_fees_collected_lamports: total_fees,
            account_cu_utilization: account_cu_formatted,
            thread_distribution: thread_dist,
            rejections_by_reason: rejected_counts,
        };

        (scheduled_txs, summary)
    }

    /// Evaluates if a transaction can be scheduled without data race or lock contention
    fn find_available_thread(&mut self, tx: &TransactionMessage) -> Option<usize> {
        for thread_idx in 0..self.num_threads {
            let active_w_locks = &self.active_write_locks[&thread_idx];
            let active_r_locks = &self.active_read_locks[&thread_idx];

            let mut has_conflict = false;

            // 1. Conflict if Tx write-account is being read or written on this thread
            for w in &tx.writable_accounts {
                if active_w_locks.contains(w) || active_r_locks.contains(w) {
                    has_conflict = true;
                    break;
                }
            }

            // 2. Conflict if Tx read-account is being written on this thread
            if !has_conflict {
                for r in &tx.readonly_accounts {
                    if active_w_locks.contains(r) {
                        has_conflict = true;
                        break;
                    }
                }
            }

            if !has_conflict {
                // Thread free for execution: Acquire non-blocking locks
                let w_set = self.active_write_locks.get_mut(&thread_idx).unwrap();
                for w in &tx.writable_accounts {
                    w_set.insert(w.clone());
                }
                let r_set = self.active_read_locks.get_mut(&thread_idx).unwrap();
                for r in &tx.readonly_accounts {
                    r_set.insert(r.clone());
                }
                return Some(thread_idx);
            }
        }
        None
    }

    fn commit_transaction(&mut self, tx: &TransactionMessage) {
        self.current_block_cu += tx.compute_units;
        for w_acc in &tx.writable_accounts {
            let counter = self.account_cu_tracker.entry(w_acc.clone()).or_insert(0);
            *counter += tx.compute_units;
        }
    }
}