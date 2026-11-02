#[cfg(test)]
mod tests {
    use solana_block_scheduler::types::*;
    use solana_block_scheduler::CentralizedScheduler;
    use solana_sdk::pubkey::Pubkey;

    fn create_test_tx(
        id_byte: u8,
        payer: Pubkey,
        reads: Vec<Pubkey>,
        writes: Vec<Pubkey>,
        cu_limit: u32,
        cu_price: u64,
        is_bundle: bool,
        tip: u64,
    ) -> SanitizedTx {
        let mut id = [0u8; 32];
        id[0] = id_byte;
        SanitizedTx {
            id: TransactionId::new(id),
            payer,
            read_locks: reads,
            write_locks: writes,
            compute_unit_limit: cu_limit,
            compute_unit_price: cu_price,
            is_jito_bundle: is_bundle,
            bundle_tip_lamports: tip,
        }
    }

    #[test]
    fn test_cu_price_sorting_order() {
        let scheduler = CentralizedScheduler::new(4);
        let payer = Pubkey::new_unique();
        let target = Pubkey::new_unique();

        let tx_low = create_test_tx(1, payer, vec![], vec![target], 200_000, 1_000, false, 0);
        let tx_high = create_test_tx(2, payer, vec![], vec![target], 200_000, 50_000, false, 0);
        let tx_mid = create_test_tx(3, payer, vec![], vec![target], 200_000, 10_000, false, 0);

        let report = scheduler.schedule_slot(100, vec![tx_low, tx_high, tx_mid]);

        // Only one can execute due to exclusive write lock on `target`
        assert_eq!(report.total_txs_scheduled, 1);
        let scheduled_batch = report.batches.iter().find(|b| !b.transactions.is_empty()).unwrap();
        // The transaction with 50,000 micro-lamports/CU must be chosen first
        assert_eq!(scheduled_batch.transactions[0].id.as_bytes()[0], 2);
    }

    #[test]
    fn test_jito_mev_bundle_precedence_over_priority_fee() {
        let scheduler = CentralizedScheduler::new(2);
        let payer = Pubkey::new_unique();
        let target = Pubkey::new_unique();

        // Standard tx with extreme priority fee
        let tx_standard = create_test_tx(1, payer, vec![], vec![target], 100_000, 500_000_000, false, 0);
        // MEV Bundle with tip
        let tx_bundle = create_test_tx(2, payer, vec![], vec![target], 100_000, 0, true, 10_000_000);

        let report = scheduler.schedule_slot(101, vec![tx_standard, tx_bundle]);

        assert_eq!(report.total_txs_scheduled, 1);
        let scheduled_batch = report.batches.iter().find(|b| !b.transactions.is_empty()).unwrap();
        // Bundle must execute prior to standard transaction
        assert_eq!(scheduled_batch.transactions[0].id.as_bytes()[0], 2);
        assert!(scheduled_batch.transactions[0].is_jito_bundle);
    }

    #[test]
    fn test_parallel_execution_of_disjoint_accounts() {
        let scheduler = CentralizedScheduler::new(4);
        let payer = Pubkey::new_unique();
        let acc_a = Pubkey::new_unique();
        let acc_b = Pubkey::new_unique();

        let tx1 = create_test_tx(1, payer, vec![], vec![acc_a], 200_000, 5_000, false, 0);
        let tx2 = create_test_tx(2, payer, vec![], vec![acc_b], 200_000, 5_000, false, 0);

        let report = scheduler.schedule_slot(102, vec![tx1, tx2]);

        // Both transactions touch completely orthogonal write locks and must execute in parallel
        assert_eq!(report.total_txs_scheduled, 2);
        assert_eq!(report.total_cu_scheduled, 400_000);
    }

    #[test]
    fn test_account_write_lock_saturation_enforcement() {
        let scheduler = CentralizedScheduler::new(4);
        let payer = Pubkey::new_unique();
        let hot_market = Pubkey::new_unique();

        // Push 10 transactions each requesting 1.5M CU to the same writable account
        // 8 txs * 1.5M = 12M CU (the hard limit per account per block)
        let mut txs = Vec::new();
        for i in 0..10 {
            txs.push(create_test_tx(
                i as u8,
                payer,
                vec![],
                vec![hot_market],
                1_500_000,
                10_000 + i as u64,
                false,
                0,
            ));
        }

        let report = scheduler.schedule_slot(103, txs);

        // First transaction consumes 1.5M CU, but since all 10 write to `hot_market`,
        // only one can be scheduled per parallel cycle unless deferred or pipelined.
        // Even when pipelined, total write capacity for `hot_market` cannot exceed 12M CU.
        let market_cu: u64 = *report
            .hot_accounts
            .iter()
            .find(|(k, _)| *k == hot_market)
            .map(|(_, cu)| cu)
            .unwrap_or(&0);

        assert!(market_cu <= MAX_ACCOUNT_WRITE_COMPUTE_UNITS);
    }
}