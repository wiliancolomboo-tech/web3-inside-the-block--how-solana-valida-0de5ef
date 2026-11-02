use solana_ordering_engine::scheduler::{BankingStageEngine, MAX_ACCOUNT_COMPUTE_UNITS, MAX_BLOCK_COMPUTE_UNITS};
use solana_ordering_engine::types::{pubkey_from_u8, AccountMeta, Instruction, JitoBundle, Transaction};

fn create_mock_tx(
    payer_id: u8,
    write_target: u8,
    cu_limit: u32,
    cu_price: u64,
    payload_id: u64,
) -> Transaction {
    Transaction {
        signatures: vec![[payer_id; 64]],
        fee_payer: pubkey_from_u8(payer_id),
        instructions: vec![Instruction {
            program_id: pubkey_from_u8(200),
            accounts: vec![
                AccountMeta {
                    pubkey: pubkey_from_u8(payer_id),
                    is_signer: true,
                    is_writable: true,
                },
                AccountMeta {
                    pubkey: pubkey_from_u8(write_target),
                    is_signer: false,
                    is_writable: true,
                },
            ],
            data: vec![],
        }],
        compute_unit_limit: cu_limit,
        compute_unit_price_micro_lamports: cu_price,
        payload_id,
    }
}

#[test]
fn test_priority_ordering_under_contention() {
    let mut engine = BankingStageEngine::new([1u8; 32]);
    let write_target = 99;

    // Tx A: Low Priority
    let tx_low = create_mock_tx(1, write_target, 200_000, 100, 10);
    // Tx B: High Priority
    let tx_high = create_mock_tx(2, write_target, 200_000, 50_000, 20);

    engine.push_transaction(tx_low);
    engine.push_transaction(tx_high);

    // Cycle 1: Should execute Tx B (tx_high) first due to Priority Fee sorting
    let cycle1 = engine.process_scheduler_cycle(1);
    assert_eq!(cycle1.len(), 1);
    assert_eq!(cycle1[0].tx_id, 20, "High priority transaction must be scheduled first");

    // Cycle 2: Should now execute Tx A (tx_low) once the write lock is released
    let cycle2 = engine.process_scheduler_cycle(1);
    assert_eq!(cycle2.len(), 1);
    assert_eq!(cycle2[0].tx_id, 10, "Contending low priority transaction executes after lock release");
}

#[test]
fn test_parallel_execution_for_disjoint_locks() {
    let mut engine = BankingStageEngine::new([2u8; 32]);

    // Tx 1 touches account 10
    let tx1 = create_mock_tx(1, 10, 100_000, 1_000, 1);
    // Tx 2 touches account 20 (Disjoint, non-conflicting write accounts)
    let tx2 = create_mock_tx(2, 20, 100_000, 1_000, 2);

    engine.push_transaction(tx1);
    engine.push_transaction(tx2);

    // Both should be scheduled in the same parallel execution batch
    let batch = engine.process_scheduler_cycle(2);
    assert_eq!(batch.len(), 2, "Disjoint transactions must execute within the same batch");
}

#[test]
fn test_account_compute_budget_saturation() {
    let mut engine = BankingStageEngine::new([3u8; 32]);
    let hot_account = 77;

    // Max per-account CU is 12,000,000.
    // Push transactions exceeding this limit
    let tx1 = create_mock_tx(1, hot_account, 8_000_000, 10_000, 101);
    let tx2 = create_mock_tx(2, hot_account, 6_000_000, 5_000, 102);

    engine.push_transaction(tx1);
    engine.push_transaction(tx2);

    let res1 = engine.process_scheduler_cycle(2);
    assert_eq!(res1.len(), 1, "Only first tx fits within account CU cap (8M <= 12M)");
    assert_eq!(res1[0].tx_id, 101);

    // Second tx exceeds account budget (8M + 6M = 14M > 12M) and must remain deferred
    let res2 = engine.process_scheduler_cycle(2);
    assert_eq!(res2.len(), 0, "Second transaction must be held back due to account CU cap saturation");
}

#[test]
fn test_atomic_jito_bundle_execution() {
    let mut engine = BankingStageEngine::new([4u8; 32]);

    let tx1 = create_mock_tx(1, 30, 150_000, 10_000, 501);
    let tx2 = create_mock_tx(2, 31, 150_000, 10_000, 502);

    let bundle = JitoBundle {
        bundle_id: "test-bundle-001".to_string(),
        transactions: vec![tx1, tx2],
        tip_lamports: 1_000_000,
    };

    let result = engine.execute_jito_bundle(bundle);
    assert!(result.is_ok(), "Jito bundle with non-conflicting internal txs must succeed");
    assert_eq!(result.unwrap().len(), 2);
}