#[cfg(test)]
mod tests {
    use solana_block_engine::bundle_engine::{BundleExecutionError, JitoBundleEngine};
    use solana_block_engine::lock_table::AccountLockTable;
    use solana_block_engine::pubkey_from_seed;
    use solana_block_engine::scheduler::CentralScheduler;
    use solana_block_engine::types::{JitoBundle, Transaction};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::mpsc;

    #[test]
    fn test_lock_table_mutual_exclusion() {
        let mut lock_table = AccountLockTable::new();
        let account_a = pubkey_from_seed(1);
        let account_b = pubkey_from_seed(2);

        let mut writes_tx1 = std::collections::HashSet::new();
        writes_tx1.insert(account_a);
        let reads_tx1 = std::collections::HashSet::new();

        // Tx1 acquires write-lock on account_a
        let acquired = lock_table.try_acquire_locks(&writes_tx1, &reads_tx1);
        assert!(acquired, "Tx1 must successfully acquire unallocated write lock");
        assert_eq!(lock_table.active_write_lock_count(), 1);

        // Tx2 tries to acquire write-lock on account_a (Should fail)
        let mut writes_tx2 = std::collections::HashSet::new();
        writes_tx2.insert(account_a);
        let reads_tx2 = std::collections::HashSet::new();

        let conflict_acquired = lock_table.try_acquire_locks(&writes_tx2, &reads_tx2);
        assert!(!conflict_acquired, "Tx2 must be blocked by Tx1's active write lock");

        // Tx3 tries to acquire read-lock on account_a (Should fail because account_a is write-locked)
        let writes_tx3 = std::collections::HashSet::new();
        let mut reads_tx3 = std::collections::HashSet::new();
        reads_tx3.insert(account_a);

        let read_conflict = lock_table.try_acquire_locks(&writes_tx3, &reads_tx3);
        assert!(!read_conflict, "Tx3 must not read an account that is currently write-locked");

        // Tx4 touches disjoint account_b (Should succeed concurrently)
        let mut writes_tx4 = std::collections::HashSet::new();
        writes_tx4.insert(account_b);
        let reads_tx4 = std::collections::HashSet::new();

        let disjoint_acquired = lock_table.try_acquire_locks(&writes_tx4, &reads_tx4);
        assert!(disjoint_acquired, "Disjoint write locks must execute concurrently in Sealevel");

        // Tx1 finishes and releases lock
        lock_table.release_locks(&writes_tx1, &reads_tx1);
        assert_eq!(lock_table.active_write_lock_count(), 1); // account_b still held

        // Now Tx2 retries and succeeds
        let retry_acquired = lock_table.try_acquire_locks(&writes_tx2, &reads_tx2);
        assert!(retry_acquired, "Tx2 must acquire write lock after Tx1 releases it");
    }

    #[test]
    fn test_concurrent_read_locks_allowed() {
        let mut lock_table = AccountLockTable::new();
        let oracle_account = pubkey_from_seed(50);

        let writes = std::collections::HashSet::new();
        let mut reads = std::collections::HashSet::new();
        reads.insert(oracle_account);

        // Multiple transactions reading the same price oracle account
        assert!(lock_table.try_acquire_locks(&writes, &reads));
        assert!(lock_table.try_acquire_locks(&writes, &reads));
        assert_eq!(lock_table.active_read_lock_count(), 1);

        lock_table.release_locks(&writes, &reads);
        assert_eq!(lock_table.active_read_lock_count(), 1);

        lock_table.release_locks(&writes, &reads);
        assert_eq!(lock_table.active_read_lock_count(), 0);
    }

    #[test]
    fn test_jito_bundle_atomicity_and_lock_rejection() {
        let mut lock_table = AccountLockTable::new();
        let locked_account = pubkey_from_seed(9);

        // Emulate background transaction already holding locked_account
        let mut bg_writes = std::collections::HashSet::new();
        bg_writes.insert(locked_account);
        assert!(lock_table.try_acquire_locks(&bg_writes, &std::collections::HashSet::new()));

        let tip_acc = pubkey_from_seed(88);
        let bundle = JitoBundle {
            bundle_id: "test_bundle".to_string(),
            transactions: vec![
                Transaction::new(1, pubkey_from_seed(100), vec![pubkey_from_seed(20)], vec![], 50_000, 100, None),
                Transaction::new(2, pubkey_from_seed(100), vec![locked_account], vec![], 50_000, 100, None),
            ],
            tip_lamports: 5_000_000,
            tip_account: tip_acc,
        };

        // Bundle validation must reject the entire bundle atomically
        let result = JitoBundleEngine::validate_bundle_atomic_locks(&bundle, &lock_table);
        match result {
            Err(BundleExecutionError::LockContention(acc)) => {
                assert_eq!(acc, locked_account, "Contention account must match the locked resource");
            }
            _ => panic!("Bundle must fail atomically when any account is contested"),
        }
    }

    #[tokio::test]
    async fn test_central_scheduler_priority_dispatch() {
        let scheduler = Arc::new(CentralScheduler::new(2));
        let metrics = scheduler.metrics();
        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);

        let sched_clone = Arc::clone(&scheduler);
        let handle = tokio::spawn(async move {
            sched_clone.run_scheduler_pipeline(shutdown_rx).await;
        });

        // Enqueue 10 transactions with varying priority fees
        for i in 1..=10 {
            scheduler.enqueue_transaction(Transaction::new(
                i,
                pubkey_from_seed(i as u8),
                vec![pubkey_from_seed((100 + i) as u8)],
                vec![],
                100_000,
                i * 1_000, // 1,000 ul/CU to 10,000 ul/CU
                None,
            ));
        }

        tokio::time::sleep(Duration::from_millis(150)).await;
        let _ = shutdown_tx.send(()).await;
        handle.await.unwrap();

        assert_eq!(
            metrics.scheduled_tx_count.load(std::sync::atomic::Ordering::Relaxed),
            10,
            "All 10 disjoint transactions must be scheduled and executed"
        );
        assert_eq!(
            metrics.lock_contention_retries.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "No contention must occur for non-overlapping accounts"
        );
    }
}