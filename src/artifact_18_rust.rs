use solana_scheduler_engine::jito_engine::JitoBundleManager;
use solana_scheduler_engine::lock_manager::{AccountLockManager, LockError};
use solana_scheduler_engine::scheduler::BankingScheduler;
use solana_scheduler_engine::types::{BlockLimits, Transaction};

#[test]
fn test_lock_manager_read_write_exclusion() {
    let mut lock_mgr = AccountLockManager::new();
    let max_cu = 12_000_000;

    let tx_write = Transaction::new("tx_w", "alice", vec![], vec!["shared_acc"], 100_000, 10);
    let tx_conflicting_write = Transaction::new("tx_w_conf", "bob", vec![], vec!["shared_acc"], 100_000, 20);
    let tx_conflicting_read = Transaction::new("tx_r_conf", "charlie", vec!["shared_acc"], vec![], 100_000, 15);

    // Acquire write lock
    assert!(lock_mgr.try_acquire_locks(&tx_write, max_cu).is_ok());

    // Subsequent write on same account MUST fail
    let err_w = lock_mgr.try_acquire_locks(&tx_conflicting_write, max_cu);
    assert_eq!(err_w, Err(LockError::WriteLockConflict("shared_acc".to_string())));

    // Subsequent read on same account MUST fail
    let err_r = lock_mgr.try_acquire_locks(&tx_conflicting_read, max_cu);
    assert_eq!(err_r, Err(LockError::ReadLockConflict("shared_acc".to_string())));

    // Release lock
    lock_mgr.release_locks(&tx_write);

    // Now write succeeds
    assert!(lock_mgr.try_acquire_locks(&tx_conflicting_write, max_cu).is_ok());
    lock_mgr.release_locks(&tx_conflicting_write);
}

#[test]
fn test_account_compute_unit_cap() {
    let mut lock_mgr = AccountLockManager::new();
    let max_cu = 500_000; // Constrained for testing

    let tx1 = Transaction::new("tx1", "alice", vec![], vec!["target_acc"], 300_000, 10);
    let tx2 = Transaction::new("tx2", "bob", vec![], vec!["target_acc"], 300_000, 20);

    assert!(lock_mgr.try_acquire_locks(&tx1, max_cu).is_ok());
    lock_mgr.release_locks(&tx1);

    // Exceeds 500k limit (300k + 300k = 600k > 500k)
    let err = lock_mgr.try_acquire_locks(&tx2, max_cu);
    assert_eq!(err, Err(LockError::AccountCULimitExceeded("target_acc".to_string())));
}

#[test]
fn test_scheduler_legitimate_priority_inversion() {
    // Demonstrates that a lower fee transaction executes ahead of a locked high fee transaction
    let mut scheduler = BankingScheduler::new(2, Some(BlockLimits::default()));

    let tx_high_fee_conflict = Transaction::new(
        "tx_high_fee_locked",
        "alice",
        vec![],
        vec!["hot_amm_account"],
        200_000,
        1_000_000, // 1M uLamports/CU
    );

    let tx_low_fee_independent = Transaction::new(
        "tx_low_fee_free",
        "bob",
        vec![],
        vec!["cold_staking_account"],
        200_000,
        100, // 100 uLamports/CU
    );

    // Simulate active write-lock on hot_amm_account held externally
    let dummy_blocker = Transaction::new("blocker", "sys", vec![], vec!["hot_amm_account"], 100, 0);
    scheduler.lock_manager.try_acquire_locks(&dummy_blocker, 12_000_000).unwrap();

    let batch = vec![tx_high_fee_conflict, tx_low_fee_independent];
    let records = scheduler.schedule_and_execute(100, batch);

    // Assert that the high-fee transaction was deferred due to lock conflict
    let high_tx_record = records.iter().find(|r| r.transaction_id == "tx_high_fee_locked").unwrap();
    assert!(high_tx_record.lock_contention_deferred);

    // Assert that the low-fee transaction committed successfully
    let low_tx_record = records.iter().find(|r| r.transaction_id == "tx_low_fee_free").unwrap();
    assert_eq!(low_tx_record.execution_status, "COMMITTED_SUCCESS");
}

#[test]
fn test_jito_bundle_verification() {
    let jito = JitoBundleManager::new();
    let tip_acc = &jito.tip_accounts[0];

    // Invalid bundle: No tip
    let tx_no_tip = Transaction::new("tx1", "alice", vec![], vec!["accA"], 100_000, 10);
    assert!(jito.validate_bundle(&[tx_no_tip]).is_err());

    // Valid bundle with tip
    let tx_tip = Transaction::new_jito_bundle_tx(
        "tx_tip",
        "alice",
        vec![],
        vec![tip_acc],
        50_000,
        100,
        1_000_000,
        "bundle_1",
    );
    let result = jito.validate_bundle(&[tx_tip]);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 1_000_000);
}