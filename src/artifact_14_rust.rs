use solana_order_engine::{
    AccountMeta, BankingStageScheduler, JitoBundleEvaluator, OrderEngineError,
    SWQoSAdmissionController, Transaction,
};
use std::collections::HashMap;

fn create_test_tx(
    sig_byte: u8,
    sender_byte: u8,
    writes: Vec<u8>,
    reads: Vec<u8>,
    cu_price: u64,
    tip: u64,
) -> Transaction {
    let mut signature = [0u8; 64];
    signature[0] = sig_byte;

    let mut sender = [0u8; 32];
    sender[0] = sender_byte;

    let mut accounts = Vec::new();
    for w in writes {
        let mut key = [0u8; 32];
        key[0] = w;
        accounts.push(AccountMeta {
            pubkey: key,
            is_writable: true,
            is_signer: false,
        });
    }
    for r in reads {
        let mut key = [0u8; 32];
        key[0] = r;
        accounts.push(AccountMeta {
            pubkey: key,
            is_writable: false,
            is_signer: false,
        });
    }

    Transaction::new(signature, sender, accounts, 200_000, cu_price, tip)
}

#[test]
fn test_parallel_execution_independent_locks() {
    let scheduler = BankingStageScheduler::new(4, 48_000_000);

    // 4 transactions accessing completely disjoint write accounts
    let tx1 = create_test_tx(1, 10, vec![1], vec![], 1_000, 0);
    let tx2 = create_test_tx(2, 20, vec![2], vec![], 1_000, 0);
    let tx3 = create_test_tx(3, 30, vec![3], vec![], 1_000, 0);
    let tx4 = create_test_tx(4, 40, vec![4], vec![], 1_000, 0);

    let report = scheduler
        .schedule_and_execute_slot(100, vec![tx1, tx2, tx3, tx4])
        .expect("Scheduling must succeed");

    assert_eq!(report.total_transactions_scheduled, 4);
    assert_eq!(report.write_lock_conflicts, 0);

    // Each tx should be placed on a distinct execution lane (0, 1, 2, 3)
    let mut assigned_lanes = std::collections::HashSet::new();
    for r in report.results {
        assigned_lanes.insert(r.thread_lane);
    }
    assert_eq!(assigned_lanes.len(), 4, "All 4 independent transactions must run in parallel lanes");
}

#[test]
fn test_write_lock_contention_and_priority_sorting() {
    let scheduler = BankingStageScheduler::new(2, 48_000_000);
    let contentious_account_id = 99u8;

    // Both tx1 and tx2 write to account 99.
    // tx1 has high priority fee (50,000), tx2 has low priority fee (100).
    let tx1_high_fee = create_test_tx(1, 10, vec![contentious_account_id], vec![], 50_000, 0);
    let tx2_low_fee = create_test_tx(2, 20, vec![contentious_account_id], vec![], 100, 0);

    // Submit in reverse priority order (low fee first)
    let batch = vec![tx2_low_fee, tx1_high_fee];
    let report = scheduler
        .schedule_and_execute_slot(101, batch)
        .expect("Scheduling must succeed");

    // Only 1 transaction can acquire the lock within this atomic evaluation round
    assert_eq!(report.total_transactions_scheduled, 1);
    assert_eq!(report.write_lock_conflicts, 1);

    // Ensure the transaction that won the execution slot is the high-fee transaction (tx1)
    let scheduled_tx = &report.results[0];
    assert_eq!(scheduled_tx.signature[0], 1, "Scheduler must pick high-fee transaction first");
}

#[test]
fn test_swqos_stake_weighted_quota_enforcement() {
    let mut stakes = HashMap::new();
    let staked_sender = [42u8; 32];
    stakes.insert(staked_sender, 2_000_000); // Entitled to 2 packets

    let mut admission = SWQoSAdmissionController::new(stakes, 1);

    let tx1 = Transaction::new([1u8; 64], staked_sender, vec![], 100_000, 100, 0);
    let tx2 = Transaction::new([2u8; 64], staked_sender, vec![], 100_000, 100, 0);
    let tx3 = Transaction::new([3u8; 64], staked_sender, vec![], 100_000, 100, 0);

    assert!(admission.admit_packet(&tx1).is_ok());
    assert!(admission.admit_packet(&tx2).is_ok());
    
    // Third packet must exceed stake quota
    let err = admission.admit_packet(&tx3).unwrap_err();
    match err {
        OrderEngineError::SWQoSQuotaExceeded(msg) => {
            assert!(msg.contains("Stake quota of 2 packets reached"));
        }
        _ => panic!("Expected SWQoSQuotaExceeded error"),
    }
}

#[test]
fn test_jito_bundle_atomic_tip_validation() {
    let tip_recipient = [88u8; 32];
    let evaluator = JitoBundleEvaluator::new(tip_recipient, 5_000);

    // Bundle with insufficient tip
    let tx_fail = Transaction::new(
        [10u8; 64],
        [1u8; 32],
        vec![AccountMeta {
            pubkey: tip_recipient,
            is_writable: true,
            is_signer: false,
        }],
        200_000,
        0,
        1_000, // Below 5,000 threshold
    );

    let res = evaluator.evaluate_bundle(&[tx_fail], 0);
    assert!(matches!(res, Err(OrderEngineError::BundleExecutionFailure(_))));

    // Bundle with valid tip meeting requirement
    let tx_ok = Transaction::new(
        [11u8; 64],
        [1u8; 32],
        vec![AccountMeta {
            pubkey: tip_recipient,
            is_writable: true,
            is_signer: false,
        }],
        200_000,
        0,
        5_000, // Exactly threshold
    );

    let success_res = evaluator.evaluate_bundle(&[tx_ok], 0).expect("Bundle evaluation must succeed");
    assert_eq!(success_res.len(), 1);
    assert!(success_res[0].is_bundle);
}