// File: tests/analyzer.test.ts
import { BlockSchedulerAnalyzer } from '../src/analyzer';
import { EnterpriseAlertService } from '../src/alertService';
import { AnalyzedTransaction } from '../src/types';

describe("Solana Transaction Ordering Analyzer Test Suite", () => {
  let alertService: EnterpriseAlertService;
  let analyzer: BlockSchedulerAnalyzer;

  beforeEach(() => {
    alertService = new EnterpriseAlertService();
    analyzer = new BlockSchedulerAnalyzer(alertService);
  });

  test("Identifies Write Lock Contention and Priority Inversion", () => {
    const mockTxs: AnalyzedTransaction[] = [
      {
        signature: "sig1",
        slot: 100,
        indexInBlock: 0,
        cuRequested: 100000,
        cuConsumed: 80000,
        priorityFeeMicroLamports: 10,
        totalTipLamports: 0,
        writableAccounts: ["HOT_POOL_KEY"],
        readableAccounts: [],
        isJitoBundle: false
      },
      {
        signature: "sig2",
        slot: 100,
        indexInBlock: 1,
        cuRequested: 100000,
        cuConsumed: 80000,
        priorityFeeMicroLamports: 50000,
        totalTipLamports: 0,
        writableAccounts: ["HOT_POOL_KEY"],
        readableAccounts: [],
        isJitoBundle: false
      }
    ];

    const report = analyzer.analyzeBlockOrdering(100, mockTxs);
    expect(report.priorityInversionDetected).toBe(true);
    expect(report.hotAccounts[0].account).toBe("HOT_POOL_KEY");
    expect(report.hotAccounts[0].contentionCount).toBe(2);
  });
});