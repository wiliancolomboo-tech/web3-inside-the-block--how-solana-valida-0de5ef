// File: tests/scheduler_tests.rs
#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn mock_pubkey(seed: u8) -> Pubkey {
        let mut bytes = [0u8; 32];
        bytes[0] = seed;
        Pubkey::new(bytes)
    }

    #[test]
    fn test_priority_ordering_micro_lamports_per_cu() {
        let mut scheduler = BankingScheduler::new(4);
        let acc_a = mock_pubkey(1);
        let acc_b = mock_pubkey(2);

        // Tx1: Pays 10,000 micro-lamports / CU (Consumes 200,000 CU)
        let tx1 = TransactionMessage {
            signature: "tx_high_priority".to_string(),
            payer: mock_pubkey(10),
            writable_accounts: vec![acc_a.clone()],
            readonly_accounts: vec![],
            compute_units: 200_000,
            priority_fee_micro_lamports: 10_000,
            is_jito_bundle: false,
            bundle_id: None,
        };

        // Tx2: Pays 1,000 micro-lamports / CU (Consumes 200,000 CU)
        let tx2 = TransactionMessage {
            signature: "tx_low_priority".to_string(),
            payer: mock_pubkey(11),
            writable_accounts: vec![acc_b.clone()],
            readonly_accounts: vec![],
            compute_units: 200_000,
            priority_fee_micro_lamports: 1_000,
            is_jito_bundle: false,
            bundle_id: None,
        };

        let (ordered, summary) = scheduler.pack_block(vec![tx2, tx1]);

        assert_eq!(ordered.len(), 2);
        assert_eq!(ordered[0].signature, "tx_high_priority");
        assert_eq!(ordered[1].signature, "tx_low_priority");
        assert_eq!(summary.total_compute_units_consumed, 400_000);
    }

    #[test]
    fn test_account_compute_budget_cap_enforcement() {
        let mut scheduler = BankingScheduler::new(4);
        let hot_amm_pool = mock_pubkey(99);

        let mut tx_pool = Vec::new();
        // Generate 70 transactions each targeting the same AMM pool for 200_000 CU
        // 70 * 200,000 = 14,000,000 CU (Exceeds 12M CU account ceiling)
        for i in 0..70 {
            tx_pool.push(TransactionMessage {
                signature: format!("tx_amm_swap_{}", i),
                payer: mock_pubkey(i as u8 + 100),
                writable_accounts: vec![hot_amm_pool.clone()],
                readonly_accounts: vec![],
                compute_units: 200_000,
                priority_fee_micro_lamports: 500,
                is_jito_bundle: false,
                bundle_id: None,
            });
        }

        let (ordered, summary) = scheduler.pack_block(tx_pool);

        // Max 12,000,000 CU / 200,000 CU = 60 transactions allowed
        assert_eq!(ordered.len(), 60);
        assert_eq!(summary.total_transactions_scheduled, 60);
        assert_eq!(summary.total_transactions_rejected, 10);
        assert_eq!(
            *summary.rejections_by_reason.get("AccountComputeLimitExceeded").unwrap(),
            10
        );
    }

    #[test]
    fn test_jito_bundle_preemption_and_atomicity() {
        let mut scheduler = BankingScheduler::new(4);
        let shared_state = mock_pubkey(55);

        // Regular High Priority Transaction
        let regular_tx = TransactionMessage {
            signature: "tx_mempool_whale".to_string(),
            payer: mock_pubkey(12),
            writable_accounts: vec![shared_state.clone()],
            readonly_accounts: vec![],
            compute_units: 300_000,
            priority_fee_micro_lamports: 50_000, // Very high fee
            is_jito_bundle: false,
            bundle_id: None,
        };

        // Jito Bundle of 2 Transactions
        let bundle_tx_1 = TransactionMessage {
            signature: "tx_bundle_arb_leg_1".to_string(),
            payer: mock_pubkey(13),
            writable_accounts: vec![shared_state.clone()],
            readonly_accounts: vec![],
            compute_units: 150_000,
            priority_fee_micro_lamports: 5_000,
            is_jito_bundle: true,
            bundle_id: Some("bundle_alpha_88".to_string()),
        };

        let bundle_tx_2 = TransactionMessage {
            signature: "tx_bundle_tip_transfer".to_string(),
            payer: mock_pubkey(13),
            writable_accounts: vec![mock_pubkey(88)], // Jito Tip Account
            readonly_accounts: vec![],
            compute_units: 50_000,
            priority_fee_micro_lamports: 5_000,
            is_jito_bundle: true,
            bundle_id: Some("bundle_alpha_88".to_string()),
        };

        let (ordered, summary) = scheduler.pack_block(vec![regular_tx, bundle_tx_1, bundle_tx_2]);

        // Bundles must execute at top of the block sequence
        assert_eq!(ordered.len(), 3);
        assert_eq!(ordered[0].signature, "tx_bundle_arb_leg_1");
        assert!(ordered[0].is_bundle);
        assert_eq!(ordered[1].signature, "tx_bundle_tip_transfer");
        assert!(ordered[1].is_bundle);
        assert_eq!(ordered[2].signature, "tx_mempool_whale");
        assert!(!ordered[2].is_bundle);
    }
}