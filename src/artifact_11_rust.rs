use solana_block_ordering_engine::{
    models::{Transaction, TransactionOrigin, MAX_ACCOUNT_COMPUTE_UNITS, MAX_BLOCK_COMPUTE_UNITS},
    scheduler::BankingStageScheduler,
    jito::JitoAuctionEngine,
};

#[test]
fn test_priority_fee_ordering_precedence() {
    let scheduler = BankingStageScheduler::new(2);

    let low_fee_tx = Transaction::new(
        "tx_low", "sender1", vec!["ACC_A"], vec![], 200_000, 10_000, TransactionOrigin::PublicQUIC, 100
    );
    let high_fee_tx = Transaction::new(
        "tx_high", "sender2", vec!["ACC_A"], vec![], 200_000, 500_000, TransactionOrigin::PublicQUIC, 105
    );

    let result = scheduler.schedule_and_pack(vec![], vec![low_fee_tx, high_fee_tx]);

    assert_eq!(result.total_executed_transactions, 2);
    // Highest fee must execute first
    assert_eq!(result.execution_log[0].transaction_id, "tx_high");
    assert_eq!(result.execution_log[1].transaction_id, "tx_low");
}

#[test]
fn test_account_compute_unit_limit_saturation() {
    let scheduler = BankingStageScheduler::new(4);

    // Each tx consumes 2,000,000 CU on "HOT_AMM".
    // 12M limit allows exactly 6 transactions.
    let mut pool = Vec::new();
    for i in 1..=10 {
        pool.push(Transaction::new(
            &format!("tx_{}", i),
            "trader",
            vec!["HOT_AMM"],
            vec![],
            2_000_000,
            i * 1_000,
            TransactionOrigin::PublicQUIC,
            i,
        ));
    }

    let result = scheduler.schedule_and_pack(vec![], pool);

    assert_eq!(result.total_executed_transactions, 6);
    assert_eq!(result.rejected_account_limit, 4);
    assert_eq!(*result.write_lock_cu_distribution.get("HOT_AMM").unwrap(), MAX_ACCOUNT_COMPUTE_UNITS);
}

#[test]
fn test_block_compute_unit_limit_saturation() {
    let scheduler = BankingStageScheduler::new(4);

    // Create 5 transactions across independent accounts, each taking 10M CU (total 50M CU).
    // The block limit is 48M CU. Only 4 transactions (40M CU) can fit.
    let mut pool = Vec::new();
    for i in 1..=5 {
        pool.push(Transaction::new(
            &format!("tx_whale_{}", i),
            "trader",
            vec![&format!("ISOLATED_ACC_{}", i)],
            vec![],
            10_000_000,
            50_000,
            TransactionOrigin::PublicQUIC,
            i,
        ));
    }

    let result = scheduler.schedule_and_pack(vec![], pool);

    assert_eq!(result.total_executed_transactions, 4);
    assert_eq!(result.rejected_block_limit, 1);
    assert_eq!(result.total_compute_units_consumed, 40_000_000);
}

#[test]
fn test_jito_bundle_atomicity_and_top_of_block_slotting() {
    let scheduler = BankingStageScheduler::new(2);

    let bundle = JitoAuctionEngine::build_bundle(
        "bundle_mev_1",
        2_500_000,
        vec![
            ("tx_b1", "searcher", vec!["ACC_SHARED"], vec![], 500_000),
            ("tx_b2", "searcher", vec!["ACC_SHARED"], vec![], 500_000),
        ],
    );

    let public_tx = Transaction::new(
        "tx_pub", "public", vec!["ACC_SHARED"], vec![], 500_000, 999_999_999, TransactionOrigin::PublicQUIC, 1
    );

    let result = scheduler.schedule_and_pack(vec![bundle], vec![public_tx]);

    assert_eq!(result.total_executed_transactions, 3);
    // Bundle must be packed first despite lower nominal micro-lamport priority fee
    assert_eq!(result.execution_log[0].transaction_id, "tx_b1");
    assert_eq!(result.execution_log[1].transaction_id, "tx_b2");
    assert_eq!(result.execution_log[2].transaction_id, "tx_pub");
    assert_eq!(result.total_jito_tips_collected, 2_500_000);
}