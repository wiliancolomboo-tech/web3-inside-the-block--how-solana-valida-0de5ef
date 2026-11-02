use crate::lock_table::LockTable;
use crate::types::{Bundle, Transaction, MAX_ACCOUNT_COMPUTE_UNITS};

#[derive(Debug)]
pub enum BundleValidationResult {
    Valid,
    ConflictingWriteLock,
    ExceededAccountLimit,
    EmptyBundle,
}

pub struct BundleProcessor;

impl BundleProcessor {
    /// Validates if an atomic bundle can be scheduled atomically without partial lock failure
    pub fn simulate_bundle_locks(
        bundle: &Bundle,
        lock_table: &mut LockTable,
    ) -> BundleValidationResult {
        if bundle.transactions.is_empty() {
            return BundleValidationResult::EmptyBundle;
        }

        let mut acquired_reads = Vec::new();
        let mut acquired_writes = Vec::new();
        let mut success = true;

        for tx in &bundle.transactions {
            let writes = tx.write_accounts();
            let reads = tx.read_accounts();
            if lock_table.try_lock(&reads, &writes, tx.compute_unit_limit, MAX_ACCOUNT_COMPUTE_UNITS).is_err() {
                success = false;
                break;
            }
            acquired_reads.push(reads);
            acquired_writes.push(writes);
        }

        // Revert acquired locks during simulation
        for (reads, writes) in acquired_reads.iter().zip(acquired_writes.iter()) {
            lock_table.unlock(reads, writes);
        }

        if success {
            BundleValidationResult::Valid
        } else {
            BundleValidationResult::ConflictingWriteLock
        }
    }
}