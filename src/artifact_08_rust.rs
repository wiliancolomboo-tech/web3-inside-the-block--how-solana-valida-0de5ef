use crate::bundle::BundleProcessor;
use crate::lock_table::LockTable;
use crate::types::{
    AccountMeta, Bundle, Entry, Pubkey, Transaction, MAX_ACCOUNT_COMPUTE_UNITS,
    MAX_BLOCK_COMPUTE_UNITS,
};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::BinaryHeap;

#[derive(Debug, Eq, PartialEq)]
struct PrioritizedTx {
    priority_score: u128,
    arrival_order: usize,
    transaction: Transaction,
}

impl Ord for PrioritizedTx {
    fn cmp(&self, other: &Self) -> Ordering {
        self.priority_score
            .cmp(&other.priority_score)
            .then_with(|| other.arrival_order.cmp(&self.arrival_order))
    }
}

impl PartialOrd for PrioritizedTx {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub struct BankingScheduler {
    prio_queue: BinaryHeap<PrioritizedTx>,
    bundle_queue: Vec<Bundle>,
    lock_table: LockTable,
    total_block_cu: u64,
    counter: usize,
    current_poh_hash: [u8; 32],
}

impl BankingScheduler {
    pub fn new(genesis_hash: [u8; 32]) -> Self {
        Self {
            prio_queue: BinaryHeap::new(),
            bundle_queue: Vec::new(),
            lock_table: LockTable::new(),
            total_block_cu: 0,
            counter: 0,
            current_poh_hash: genesis_hash,
        }
    }

    pub fn insert_transaction(&mut self, tx: Transaction) {
        let score = tx.priority_score();
        self.prio_queue.push(PrioritizedTx {
            priority_score: score,
            arrival_order: self.counter,
            transaction: tx,
        });
        self.counter += 1;
    }

    pub fn insert_bundle(&mut self, bundle: Bundle) {
        self.bundle_queue.push(bundle);
    }

    /// Process next block batch: Interleaves Jito Bundles first, followed by priority queue drain
    pub fn schedule_and_execute_block(&mut self) -> Vec<Entry> {
        let mut executed_transactions: Vec<Transaction> = Vec::new();
        let mut entries = Vec::new();

        // 1. Process MEV Bundles (Atomic Priority Ingestion)
        // Sort bundles by bundle tip descending
        self.bundle_queue.sort_by(|a, b| b.bundle_tip.cmp(&a.bundle_tip));

        let bundles = std::mem::take(&mut self.bundle_queue);
        for bundle in bundles {
            let total_bundle_cu: u64 = bundle.transactions.iter().map(|t| t.compute_unit_limit).sum();
            if self.total_block_cu.saturating_add(total_bundle_cu) > MAX_BLOCK_COMPUTE_UNITS {
                continue; // Block CU limit reached
            }

            // Simulate lock contention
            if let crate::bundle::BundleValidationResult::Valid =
                BundleProcessor::simulate_bundle_locks(&bundle, &mut self.lock_table)
            {
                // Lock and execute all transactions in bundle
                for tx in bundle.transactions {
                    let writes = tx.write_accounts();
                    let reads = tx.read_accounts();
                    self.lock_table
                        .try_lock(&reads, &writes, tx.compute_unit_limit, MAX_ACCOUNT_COMPUTE_UNITS)
                        .expect("Pre-validated locks must succeed");

                    self.total_block_cu += tx.compute_unit_limit;
                    executed_transactions.push(tx.clone());

                    // Release locks once executed
                    self.lock_table.unlock(&reads, &writes);
                }
            }
        }

        // 2. Process Standard Transaction Priority Queue with Non-Blocking Worker Slots
        let mut deferred_transactions: Vec<PrioritizedTx> = Vec::new();

        while let Some(prioritized) = self.prio_queue.pop() {
            let tx = prioritized.transaction;
            if self.total_block_cu.saturating_add(tx.compute_unit_limit) > MAX_BLOCK_COMPUTE_UNITS {
                // Block is full
                break;
            }

            let writes = tx.write_accounts();
            let reads = tx.read_accounts();

            // Try to acquire write/read locks without stalling parallel threads
            match self.lock_table.try_lock(
                &reads,
                &writes,
                tx.compute_unit_limit,
                MAX_ACCOUNT_COMPUTE_UNITS,
            ) {
                Ok(()) => {
                    // Executed successfully in parallel stage
                    self.total_block_cu += tx.compute_unit_limit;
                    executed_transactions.push(tx);
                    self.lock_table.unlock(&reads, &writes);
                }
                Err(_) => {
                    // Write-lock conflict or single-account CU limit hit -> defer for next batch
                    deferred_transactions.push(PrioritizedTx {
                        priority_score: prioritized.priority_score,
                        arrival_order: prioritized.arrival_order,
                        transaction: tx,
                    });
                }
            }
        }

        // Re-insert deferred transactions for next scheduler cycle
        for deferred in deferred_transactions {
            self.prio_queue.push(deferred);
        }

        // 3. PoH Micro-Batching: Chunk executed txs into entries
        if !executed_transactions.is_empty() {
            for chunk in executed_transactions.chunks(64) {
                let mut hasher = Sha256::new();
                hasher.update(&self.current_poh_hash);
                for tx in chunk {
                    hasher.update(&tx.payload_id.to_le_bytes());
                }
                let new_hash: [u8; 32] = hasher.finalize().into();
                self.current_poh_hash = new_hash;

                entries.push(Entry {
                    num_hashes: 128, // Standard Solana micro-tick step
                    hash: new_hash,
                    transactions: chunk.to_vec(),
                });
            }
        }

        entries
    }

    pub fn get_total_block_cu(&self) -> u64 {
        self.total_block_cu
    }
}