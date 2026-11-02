#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Pubkey, Transaction, MAX_ACCOUNT_COMPUTE_UNITS, MAX_BLOCK_COMPUTE_UNITS};

    #[test]
    fn test_jito_bundle_precedence_over_regular_high_priority_fee() {
        let mut scheduler = BankingStageScheduler::new(4);
        let acc_a = Pubkey::from_u8_single(1);
        let acc_b = Pubkey::from_u8_single(2);

        let standard_tx = Transaction {
            id: "standard_tx".to_string(),
            read_accounts: vec![acc_a.clone()],
            write_accounts: vec![acc_b.clone()],
            requested_cu: 200_000,
            priority_fee_micro_lamports: 5_000_000, // Massive fee
            is_jito_bundle: false,
            jito_tip_lamports: 0,
        };

        let jito_bundle_tx = Transaction {
            id: "jito_bundle_tx".to_string(),
            read_accounts: vec![acc_a],
            write_accounts: vec![acc_b],
            requested_cu: 200_000,
            priority_fee_micro_lamports: 0,
            is_jito_bundle: true,
            jito_tip_lamports: 10_000, // Explicit tip
        };

        let summary = scheduler
            .schedule_and_pack_block(100, vec![standard_tx, jito_bundle_tx])
            .expect("Scheduling must succeed");

        assert_eq!(summary.executions.len(), 2);
        // Jito bundle must be positioned first in execution list
        assert_eq!(summary.executions[0].transaction_id, "jito_bundle_tx");
        assert_eq!(summary.executions[1].transaction_id, "standard_tx");
    }

    #[test]
    fn test_block_compute_unit_limit_capping() {
        let mut scheduler = BankingStageScheduler::new(4);
        let acc = Pubkey::from_u8_single(3);

        let huge_tx_1 = Transaction {
            id: "huge_1".to_string(),
            read_accounts: vec![],
            write_accounts: vec![acc.clone()],
            requested_cu: 10_000_000,
            priority_fee_micro_lamports: 100,
            is_jito_bundle: false,
            jito_tip_lamports: 0,
        };

        let huge_tx_2 = Transaction {
            id: "huge_2".to_string(),
            read_accounts: vec![],
            write_accounts: vec![Pubkey::from_u8_single(4)],
            requested_cu: 10_000_000,
            priority_fee_micro_lamports: 90,
            is_jito_bundle: false,
            jito_tip_lamports: 0,
        };

        let summary = scheduler
            .schedule_and_pack_block(101, vec![huge_tx_1, huge_tx_2])
            .expect("Should execute within 48M limit");

        assert_eq!(summary.total_transactions_packed, 2);
        assert_eq!(summary.total_cu_consumed, 20_000_000);
        assert_eq!(summary.dropped_transactions, 0);
    }

    #[test]
    fn test_account_compute_unit_limit_enforcement() {
        let mut scheduler = BankingStageScheduler::new(4);
        let hot_account = Pubkey::from_u8_single(99);

        // Account limit is 12M CU. We send 2 txs requesting 7M each to the same write-account.
        let tx1 = Transaction {
            id: "tx1".to_string(),
            read_accounts: vec![],
            write_accounts: vec![hot_account.clone()],
            requested_cu: 7_000_000,
            priority_fee_micro_lamports: 100,
            is_jito_bundle: false,
            jito_tip_lamports: 0,
        };

        let tx2 = Transaction {
            id: "tx2".to_string(),
            read_accounts: vec![],
            write_accounts: vec![hot_account.clone()],
            requested_cu: 7_000_000,
            priority_fee_micro_lamports: 50,
            is_jito_bundle: false,
            jito_tip_lamports: 0,
        };

        let summary = scheduler
            .schedule_and_pack_block(102, vec![tx1, tx2])
            .expect("Must process batch");

        // The second transaction should be rejected because 7M + 7M = 14M > 12M limit
        assert_eq!(summary.total_transactions_packed, 1);
        assert_eq!(summary.dropped_transactions, 1);
        assert_eq!(summary.executions[0].transaction_id, "tx1");
    }

    #[test]
    fn test_lock_contention_detection() {
        let mut graph = LockContentionGraph::new();
        let target = Pubkey::from_u8_single(42);

        let writer = Transaction {
            id: "writer".to_string(),
            read_accounts: vec![],
            write_accounts: vec![target.clone()],
            requested_cu: 100_000,
            priority_fee_micro_lamports: 1,
            is_jito_bundle: false,
            jito_tip_lamports: 0,
        };

        let reader = Transaction {
            id: "reader".to_string(),
            read_accounts: vec![target.clone()],
            write_accounts: vec![],
            requested_cu: 100_000,
            priority_fee_micro_lamports: 1,
            is_jito_bundle: false,
            jito_tip_lamports: 0,
        };

        assert!(graph.can_acquire_locks(&writer));
        graph.acquire_locks(&writer);

        // Cannot read or write while active write lock exists
        assert!(!graph.can_acquire_locks(&reader));
        graph.release_locks(&writer);

        // Now reader can acquire
        assert!(graph.can_acquire_locks(&reader));
    }
}