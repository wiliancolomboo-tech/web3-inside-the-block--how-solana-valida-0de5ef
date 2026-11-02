export interface BlockAnalysisResult {
  slot: number;
  blockhash: string;
  leader: string;
  transactionCount: number;
  totalComputeUnitsConsumed: number;
  maxComputeUnitsAllocated: number;
  meanPriorityFeeMicroLamports: number;
  medianPriorityFeeMicroLamports: number;
  topContendedWritableAccounts: {
    account: string;
    contentionCount: number;
  }[];
  jitoTipDetected: boolean;
}

export interface EnterpriseAlertPayload {
  severity: "INFO" | "WARNING" | "CRITICAL";
  title: string;
  message: string;
  slot: number;
  hotAccounts: string[];
  metrics: {
    txCount: number;
    avgCuPrice: number;
  };
}