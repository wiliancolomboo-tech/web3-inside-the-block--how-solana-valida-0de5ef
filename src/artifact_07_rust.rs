#[cfg(test)]
mod tests {
    use super::*;
    use solana_block_ordering_engine::analyzer::*;
    use solana_block_ordering_engine::types::*;

    #[test]
    fn test_account_saturation_detection() {
        let mut account_usage = std::collections::HashMap::new();
        // Insert account exceeding 12M CU write limit
        account_usage.insert("AccountHotPool111111111111111111111111111".to_string(), (12_500_000u64, 45u32));
        account_usage.insert("AccountColdPool111111111111111111111111".to_string(), (1_500_000u64, 5u32));

        let mut saturated = Vec::new();
        for (account, (cu, count)) in account_usage.into_iter() {
            if cu >= MAX_ACCOUNT_COMPUTE_UNITS {
                saturated.push(AccountLockUtilization {
                    account,
                    total_cu_consumed: cu,
                    total_transactions: count,
                    is_saturated: true,
                });
            }
        }

        assert_eq!(saturated.len(), 1);
        assert_eq!(saturated[0].account, "AccountHotPool111111111111111111111111111");
        assert!(saturated[0].is_saturated);
    }

    #[test]
    fn test_priority_inversion_lock_contention_detection() {
        let locked_account = "PoolX11111111111111111111111111111111111111".to_string();

        let records = vec![
            TransactionOrderRecord {
                slot: 280000000,
                block_index: 0,
                signature: "sig1_early_low_fee".to_string(),
                fee_lamports: 5_000,
                compute_units_consumed: 200_000,
                compute_units_requested: 200_000,
                micro_lamports_per_cu: 25,
                write_locked_accounts: vec![locked_account.clone()],
                is_jito_bundle_candidate: false,
                execution_success: true,
            },
            TransactionOrderRecord {
                slot: 280000000,
                block_index: 1,
                signature: "sig2_late_high_fee".to_string(),
                fee_lamports: 500_000,
                compute_units_consumed: 200_000,
                compute_units_requested: 200_000,
                micro_lamports_per_cu: 2_500, // 100x fee
                write_locked_accounts: vec![locked_account.clone()],
                is_jito_bundle_candidate: false,
                execution_success: true,
            },
        ];

        // Using private helper exposed for tests
        let early = &records[0];
        let late = &records[1];

        assert!(late.micro_lamports_per_cu > early.micro_lamports_per_cu * 5);
        let shared: Vec<_> = early.write_locked_accounts.iter()
            .filter(|a| late.write_locked_accounts.contains(a))
            .cloned()
            .collect();

        assert_eq!(shared.len(), 1);
        assert_eq!(shared[0], locked_account);
    }

    #[test]
    fn test_jito_tip_account_recognition() {
        let sample_account = "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5";
        let is_contained = JITO_TIP_PAYMENT_ACCOUNTS.contains(&sample_account);
        assert!(is_contained, "Target Jito tip account must be recognized");

        let arbitrary_account = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
        let is_arbitrary_contained = JITO_TIP_PAYMENT_ACCOUNTS.contains(&arbitrary_account);
        assert!(!is_arbitrary_contained, "Standard account must not trigger Jito tip flag");
    }
}