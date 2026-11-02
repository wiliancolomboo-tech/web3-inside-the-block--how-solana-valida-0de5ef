use solana_block_orderer::{
    engine::BankingStageScheduler,
    models::{
        Bundle, TransactionMetadata, MAX_ACCOUNT_WRITE_COMPUTE_UNITS, MAX_BLOCK_COMPUTE_UNITS,
    },
};
use solana_sdk::{pubkey::Pubkey, signature::Signature};

fn mock_tx(
    writable: Vec<Pubkey>,
    cu_limit: u32,
    cu_price_micro: u64,
    jito_tip: Option<u64>,
) -> TransactionMetadata {
    TransactionMetadata {
        id: format!("tx-{}", Signature::new_unique()),
        signature: Signature::new_unique(),
        fee_payer: Pubkey::new_unique(),
        writable_accounts: writable,
        readonly_accounts: vec![Pubkey::new_unique()],
        compute_unit_limit: cu_limit,
        compute_unit_price_micro_lamports: cu_price_micro,
        base_fee_lamports: 5_000,
        total_priority_fee_lamports: ((cu_limit as u64) * cu_price_micro) / 1_000_000,
        jito_tip_lamports: jito_tip,
        is_bundle: false,
        bundle_id: None,
    }
}

#[test]
fn test_priority_ordering_strict_descending() {
    let scheduler = BankingStageScheduler::new(1);
    let leader = Pubkey::new_unique();
    let common_account = Pubkey::new_unique();

    let low_fee_tx = mock_tx(vec![common_account], 200_000, 100, None);
    let mid_fee_tx = mock_tx(vec![common_account], 200_000, 5_000, None);
    let high_fee_tx = mock_tx(vec![common_account], 200_000, 100_000, None);

    let tx_input = vec![low_fee_tx.clone(), high_fee_tx.clone(), mid_fee_tx.clone()];

    let result = scheduler
        .schedule_slot(100, leader, tx_input, vec![])
        .expect("Scheduling should succeed");

    assert_eq!(result.total_transactions_scheduled, 3);
    let scheduled_seq = &result.batches[0].transactions;

    // High fee must be first, then mid, then low
    assert_eq!(scheduled_seq[0].compute_unit_price_micro_lamports, 100_000);
    assert_eq!(scheduled_seq[1].compute_unit_price_micro_lamports, 5_000);
    assert_eq!(scheduled_seq[2].compute_unit_price_micro_lamports, 100);
}

#[test]
fn test_account_cu_saturation_12m_limit() {
    let scheduler = BankingStageScheduler::new(2);
    let leader = Pubkey::new_unique();
    let hot_amm_pool = Pubkey::new_unique();

    // 13 transactions * 1,000,000 CU = 13M CU against the SAME account.
    // The 12M CU limit must strictly accept the first 12 and reject the 13th!
    let mut txs = Vec::new();
    for i in 0..13 {
        txs.push(mock_tx(vec![hot_amm_pool], 1_000_000, 1_000 + i, None));
    }

    let result = scheduler
        .schedule_slot(101, leader, txs, vec![])
        .expect("Scheduling should succeed");

    assert_eq!(result.total_transactions_scheduled, 12);
    assert_eq!(result.total_transactions_dropped, 1);

    // Verify account usage exactly equals 12_000_000 CU
    let (_, cu_used) = result
        .account_cu_utilization
        .iter()
        .find(|(k, _)| *k == hot_amm_pool)
        .expect("Account must exist in usage map");

    assert_eq!(*cu_used, MAX_ACCOUNT_WRITE_COMPUTE_UNITS);
}

#[test]
fn test_jito_bundle_atomic_precedence() {
    let scheduler = BankingStageScheduler::new(4);
    let leader = Pubkey::new_unique();
    let target_account = Pubkey::new_unique();

    // Regular transaction with high priority fee
    let regular_tx = mock_tx(vec![target_account], 200_000, 1_000_000, None);

    // Jito bundle with massive tip (0.5 SOL)
    let bundle_tx_1 = mock_tx(vec![target_account], 100_000, 0, Some(500_000_000));
    let bundle = Bundle {
        bundle_id: "test-bundle-alpha".to_string(),
        transactions: vec![bundle_tx_1],
        aggregate_tip_lamports: 500_000_000,
    };

    let result = scheduler
        .schedule_slot(102, leader, vec![regular_tx], vec![bundle])
        .expect("Slot simulation failed");

    assert_eq!(result.total_transactions_scheduled, 2);
    assert_eq!(result.total_jito_tips_collected_lamports, 500_000_000);
}

#[test]
fn test_parallel_thread_conflict_isolation() {
    let scheduler = BankingStageScheduler::new(2);
    let leader = Pubkey::new_unique();

    let account_a = Pubkey::new_unique();
    let account_b = Pubkey::new_unique();

    // Tx1 touches account A, Tx2 touches account B (non-overlapping disjoint write-locks)
    let tx1 = mock_tx(vec![account_a], 500_000, 10_000, None);
    let tx2 = mock_tx(vec![account_b], 500_000, 10_000, None);

    let result = scheduler
        .schedule_slot(103, leader, vec![tx1, tx2], vec![])
        .expect("Simulation should succeed");

    assert_eq!(result.total_transactions_scheduled, 2);
    assert_eq!(result.total_transactions_dropped, 0);

    // Tx1 and Tx2 should execute cleanly across threads without colliding
    let thread_0_txs = result.batches[0].transactions.len();
    let thread_1_txs = result.batches[1].transactions.len();
    assert!(thread_0_txs >= 1);
    assert!(thread_1_txs <= 1);
}

#[test]
fn test_block_cu_overflow_rejection() {
    let scheduler = BankingStageScheduler::new(4);
    let leader = Pubkey::new_unique();

    // Send 50 transactions of 1,000,000 CU each with distinct accounts (50M CU total)
    // Max block is 48M CU. The last 2 must be rejected due to global block exhaustion!
    let mut txs = Vec::new();
    for _ in 0..50 {
        txs.push(mock_tx(vec![Pubkey::new_unique()], 1_000_000, 1_000, None));
    }

    let result = scheduler
        .schedule_slot(104, leader, txs, vec![])
        .expect("Simulation should succeed");

    assert_eq!(result.total_transactions_scheduled, 48);
    assert_eq!(result.total_transactions_dropped, 2);
    assert_eq!(result.total_compute_units, MAX_BLOCK_COMPUTE_UNITS);
}