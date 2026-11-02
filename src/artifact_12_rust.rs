#[cfg(test)]
mod tests {
    use solana_block_engine::types::*;
    use solana_block_engine::scheduler::LockManager;
    use solana_block_engine::simulator::BankingStageSimulator;
    use solana_block_engine::jito::JitoAuctionEngine;

    fn make_test_tx(
        id: u64,
        payer: Pubkey,
        writable: Vec<Pubkey>,
        readonly: Vec<Pubkey>,
        cu: u64,
        cu_price: u64,
    ) -> Transaction {
        Transaction {
            id,
            signature: [0u8; 64],
            payer,
            writable_accounts: writable,
            readonly_accounts: readonly,
            requested_compute_units: cu,
            priority_fee_micro_lamports_per_cu: cu_price,
            is_jito_tip: false,
            tip_amount_lamports: 0,
        }
    }

    #[test]
    fn test_priority_fee_ordering() {
        let mut sim = BankingStageSimulator::new(2);
        let acc_a: Pubkey = [1u8; 32];
        let acc_b: Pubkey = [2u8; 32];
        let acc_c: Pubkey = [3u8; 32];

        // tx_low has low priority fee, tx_high has high priority fee
        let tx_low = make_test_tx(1, acc_a, vec![acc_a], vec![], 100_000, 100);
        let tx_high = make_test_tx(2, acc_b, vec![acc_b], vec![], 100_000, 10_000);
        let tx_mid = make_test_tx(3, acc_c, vec![acc_c], vec![], 100_000, 5_000);

        let summary = sim.pack_block(vec![tx_low, tx_high, tx_mid], vec![]);

        assert_eq!(summary.total_transactions_scheduled, 3);
        assert_eq!(summary.executions[0].transaction_id, 2, "Tx with 10,000 µ-lamports/CU must be packed first");
        assert_eq!(summary.executions[1].transaction_id, 3, "Tx with 5,000 µ-lamports/CU must be packed second");
        assert_eq!(summary.executions[2].transaction_id, 1, "Tx with 100 µ-lamports/CU must be packed third");
    }

    #[test]
    fn test_write_lock_contention_serialization() {
        let mut lock_mgr = LockManager::new();
        let contentious_pool: Pubkey = [9u8; 32];
        let user_1: Pubkey = [10u8; 32];
        let user_2: Pubkey = [11u8; 32];

        let tx1 = make_test_tx(1, user_1, vec![contentious_pool], vec![], 200_000, 1_000);
        let tx2 = make_test_tx(2, user_2, vec![contentious_pool], vec![], 200_000, 1_000);

        assert!(lock_mgr.can_acquire(&tx1), "First tx acquires exclusive write lock");
        lock_mgr.acquire(&tx1);

        assert!(!lock_mgr.can_acquire(&tx2), "Second tx targeting the same write lock must be rejected");

        lock_mgr.release(&tx1);
        assert!(lock_mgr.can_acquire(&tx2), "Lock released: second tx can now acquire");
    }

    #[test]
    fn test_concurrent_read_lock_sharing() {
        let mut lock_mgr = LockManager::new();
        let oracle_state: Pubkey = [55u8; 32];
        let user_1: Pubkey = [12u8; 32];
        let user_2: Pubkey = [13u8; 32];

        let tx1 = make_test_tx(1, user_1, vec![user_1], vec![oracle_state], 100_000, 100);
        let tx2 = make_test_tx(2, user_2, vec![user_2], vec![oracle_state], 100_000, 100);

        assert!(lock_mgr.can_acquire(&tx1));
        lock_mgr.acquire(&tx1);

        assert!(lock_mgr.can_acquire(&tx2), "Concurrent read locks on the same account MUST be permitted");
        lock_mgr.acquire(&tx2);

        lock_mgr.release(&tx1);
        lock_mgr.release(&tx2);
    }

    #[test]
    fn test_account_level_cu_ceiling() {
        let mut sim = BankingStageSimulator::new(4);
        let hot_account: Pubkey = [77u8; 32];

        // 12M CU limit per account; push 6 txs requesting 2.5M CU each = 15.0M CU total
        let mut txs = Vec::new();
        for i in 0..6 {
            let payer: Pubkey = [100 + i as u8; 32];
            txs.push(make_test_tx(i + 1, payer, vec![hot_account], vec![], 2_500_000, 500));
        }

        let summary = sim.pack_block(txs, vec![]);
        // 4 txs * 2.5M = 10M CU fits. 5th would make 12.5M, exceeding 12M limit
        assert_eq!(summary.total_transactions_scheduled, 4, "Must cap at 4 txs to avoid breaching 12M CU limit");
        assert_eq!(summary.total_compute_units_used, 10_000_000);
    }

    #[test]
    fn test_jito_bundle_atomic_insertion() {
        let mut sim = BankingStageSimulator::new(4);
        let tip_account: Pubkey = [88u8; 32];

        let normal_tx = make_test_tx(50, [1u8; 32], vec![[2u8; 32]], vec![], 200_000, 50_000);

        let mut tip_tx = make_test_tx(99, [3u8; 32], vec![tip_account], vec![], 100_000, 0);
        tip_tx.is_jito_tip = true;
        tip_tx.tip_amount_lamports = 10_000_000;

        let bundle = JitoBundle {
            bundle_id: "mev-arbitrage-bundle".to_string(),
            transactions: vec![tip_tx],
            tip_lamports: 10_000_000,
        };

        let summary = sim.pack_block(vec![normal_tx], vec![bundle]);

        assert_eq!(summary.total_transactions_scheduled, 2);
        assert!(summary.executions[0].is_bundle, "Jito bundle must be scheduled ahead of standard priority fee txs");
        assert_eq!(summary.total_jito_tips_collected, 10_000_000);
    }
}