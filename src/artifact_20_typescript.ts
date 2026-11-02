import { auditSlotOrdering, BlockTransactionDetail } from "../src/services/orderAudit";

function runTests() {
  console.log("=== Running TypeScript Unit Tests for Slot Order Auditor ===");

  // Test 1: Anomaly triggered when an account exceeds 40% write density
  const contestedTxns: BlockTransactionDetail[] = [
    { signature: "tx1", cuRequested: 200000, priorityFeeMicroLamports: 10, writeKeys: ["HOT_DEX"], isJitoBundle: false },
    { signature: "tx2", cuRequested: 200000, priorityFeeMicroLamports: 10, writeKeys: ["HOT_DEX"], isJitoBundle: false },
    { signature: "tx3", cuRequested: 200000, priorityFeeMicroLamports: 10, writeKeys: ["OTHER"], isJitoBundle: false },
  ];

  const result1 = auditSlotOrdering(1001, contestedTxns);
  if (!result1.anomalyDetected) {
    throw new Error("Assertion Failed: Expected anomalyDetected to be TRUE for > 40% contention");
  }
  console.log("✓ Test 1 Passed: Severe write contention correctly flagged as anomaly.");

  // Test 2: Normal balanced execution
  const balancedTxns: BlockTransactionDetail[] = [
    { signature: "txA", cuRequested: 200000, priorityFeeMicroLamports: 10, writeKeys: ["ACC_1"], isJitoBundle: false },
    { signature: "txB", cuRequested: 200000, priorityFeeMicroLamports: 10, writeKeys: ["ACC_2"], isJitoBundle: false },
    { signature: "txC", cuRequested: 200000, priorityFeeMicroLamports: 10, writeKeys: ["ACC_3"], isJitoBundle: false },
    { signature: "txD", cuRequested: 200000, priorityFeeMicroLamports: 10, writeKeys: ["ACC_4"], isJitoBundle: false },
  ];

  const result2 = auditSlotOrdering(1002, balancedTxns);
  if (result2.anomalyDetected) {
    throw new Error("Assertion Failed: Expected anomalyDetected to be FALSE for balanced traffic");
  }
  console.log("✓ Test 2 Passed: Disjoint transactions validated without false positives.");
  console.log("All QA Matrix tests passed successfully (100% assertions valid).");
}

runTests();