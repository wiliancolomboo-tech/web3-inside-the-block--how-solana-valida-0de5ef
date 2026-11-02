use solana_block_ordering_engine::bundle::BundleProcessor;
use solana_block_ordering_engine::lock_table::{LockError, LockTable};
use solana_block_ordering_engine::scheduler::BankingScheduler;
use solana_block_ordering_engine::types::{
    AccountMeta, Bundle, Pubkey, Transaction, MAX_ACCOUNT_COMPUTE_UNITS, MAX_BLOCK_COMPUTE_UNITS,
};

fn generate_dummy_pubkey(byte: u8) -> Pubkey {
    [byte; 32]
}

#[test]
fn test_lock_conflict_detection() {
    let mut lock_table = LockTable::new();
    let account_a = generate_dummy_pubkey(1);
    let account_b = generate_dummy_pubkey(2);

    // Acquire write lock on Account A
    let res1 = lock_table.try_lock(&[], &[account_a], 200_000, MAX_ACCOUNT_COMPUTE_UNITS);
    assert!(res1.is_ok(), "First write lock acquisition must succeed");

    // Second write lock on Account A must fail (contention)
    let res2 = lock_table.try_lock(&[], &[account_a], 200_000, MAX_ACCOUNT_COMPUTE_UNITS);
    assert_eq!(
        res2,
        Err(LockError::WriteConflict(account_a)),
        "Conflicting write lock must be rejected"
    );

    // Read lock on Account A must fail while write lock is held
    let res3 = lock_table.try_lock(&[account_a], &[], 200_000, MAX_ACCOUNT_COMPUTE_UNITS);
    assert_eq!(
        res3,
        Err(LockError::WriteConflict(account_a)),
        "Read lock must fail while write lock is active"
    );

    // Unrelated Account B write lock must succeed concurrently
    let res4 = lock_table.try_lock(&[], &[account_b], 200_000, MAX_ACCOUNT_COMPUTE_UNITS);
    assert!(res4.is_ok(), "Non-conflicting account locks must succeed");

    // Unlock Account A
    lock_table.unlock(&[], &[account_a]);

    // Now read lock on Account A must succeed
    let res5 = lock_table.try_lock(&[account_a], &[], 200_000, MAX_ACCOUNT_COMPUTE_UNITS);
    assert!(res5.is_ok(), "Read lock must succeed after write lock released");
}

#[test]
fn test_account_compute_budget_cap() {
    let mut lock_table = LockTable::new();
    let hot_account = generate_dummy_pubkey(99);

    // Allocate up to 10M CU (Allowed, cap is 12M)
    let res1 = lock_table.try_lock(&[], &[hot_account], 10_000_000, MAX_ACCOUNT_COMPUTE_UNITS);
    assert!(res1.is_ok());
    lock_table.unlock(&[], &[hot_account]);

    // Attempt to exceed MAX_ACCOUNT_COMPUTE_UNITS (12M + 1)
    let res2 = lock_table.try_lock(&[], &[hot_account], 3_000_000, MAX_ACCOUNT_COMPUTE_UNITS);
    assert_eq!(
        res2,
        Err(LockError::AccountComputeExceeded(hot_account)),
        "Must reject transactions that would push an account past 12M CU"
    );
}

#[test]
fn test_priority_queue_ordering() {
    let genesis_hash = [0u8; 32];
    let mut scheduler = BankingScheduler::new(genesis_hash);

    let account_x = generate_dummy_pubkey(10);
    let account_y = generate_dummy_pubkey(20);

    // Low fee transaction
    let tx_low = Transaction::new(
        vec![AccountMeta { pubkey: account_x, is_signer: true, is_writable: true }],
        200_000,
        1_000, // 1,000 microLamports/CU
        0,
        1,
    );

    // High fee transaction
    let tx_high = Transaction::new(
        vec![AccountMeta { pubkey: account_y, is_signer: true, is_writable: true }],
        200_000,
        50_000, // 50,000 microLamports/CU
        0,
        2,
    );

    // Insert low first, then high
    scheduler.insert_transaction(tx_low);
    scheduler.insert_transaction(tx_high);

    let entries = scheduler.schedule_and_execute_block();
    assert!(!entries.is_empty(), "Entries should have been produced");

    // The high-fee transaction must be scheduled first
    assert_eq!(entries[0].transactions[0].payload_id, 2);
    assert_eq!(entries[0].transactions[1].payload_id, 1);
}

#[test]
fn test_atomic_jito_bundle_interleaving() {
    let genesis_hash = [7u8; 32];
    let mut scheduler = BankingScheduler::new(genesis_hash);

    let arb_pool = generate_dummy_pubkey(55);

    // Normal transaction on arb_pool with high CU price
    let normal_tx = Transaction::new(
        vec![AccountMeta { pubkey: arb_pool, is_signer: true, is_writable: true }],
        200_000,
        100_000,
        0,
        101,
    );

    // Jito Bundle with tip targeting the same pool
    let bundle_tx1 = Transaction::new(
        vec![AccountMeta { pubkey: arb_pool, is_signer: true, is_writable: true }],
        200_000,
        1,
        0,
        201,
    );
    let bundle_tx2 = Transaction::new(
        vec![AccountMeta { pubkey: arb_pool, is_signer: true, is_writable: true }],
        200_000,
        1,
        0,
        202,
    );

    let bundle = Bundle {
        transactions: vec![bundle_tx1, bundle_tx2],
        bundle_tip: 50_000_000, // 0.05 SOL tip
    };

    scheduler.insert_transaction(normal_tx);
    scheduler.insert_bundle(bundle);

    let entries = scheduler.schedule_and_execute_block();

    // Verify Jito bundle transactions were interleaved at the very beginning
    assert_eq!(entries[0].transactions[0].payload_id, 201);
    assert_eq!(entries[0].transactions[1].payload_id, 202);
    assert_eq!(entries[0].transactions[2].payload_id, 101);
}

#[test]
fn test_block_cu_overflow_protection() {
    let genesis_hash = [3u8; 32];
    let mut scheduler = BankingScheduler::new(genesis_hash);

    // Attempt to schedule 50 transactions of 1,000,000 CU each (Total 50M > 48M limit)
    for i in 0..50 {
        let account = generate_dummy_pubkey((i % 250) as u8);
        let tx = Transaction::new(
            vec![AccountMeta { pubkey: account, is_signer: true, is_writable: true }],
            1_000_000,
            10,
            0,
            i as u64,
        );
        scheduler.insert_transaction(tx);
    }

    scheduler.schedule_and_execute_block();
    assert!(
        scheduler.get_total_block_cu() <= MAX_BLOCK_COMPUTE_UNITS,
        "Total block CU must never exceed 48,000,000 CU limit"
    );
}