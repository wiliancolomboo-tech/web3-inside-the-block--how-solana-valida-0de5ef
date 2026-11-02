import axios from "axios";

interface TransactionCandidate {
  id: string;
  sender: string;
  write_locks: string[];
  read_locks: string[];
  compute_units: number;
  priority_fee_micro_lamports: number;
  origin: "PublicQUIC";
  arrival_timestamp_ns: number;
}

export class SolanaOrderingClient {
  private readonly sbeiEndpoint: string;
  private readonly slackWebhookUrl: string;

  constructor(sbeiEndpoint: string, slackWebhookUrl: string) {
    this.sbeiEndpoint = sbeiEndpoint;
    this.slackWebhookUrl = slackWebhookUrl;
  }

  /**
   * Calculates the exact micro-lamport fee required to overcome existing write-lock congestion.
   */
  public calculateOptimalPriorityFee(
    targetAccount: string,
    requestedCu: number,
    existingContenders: { priorityFee: number; cu: number }[]
  ): number {
    // Sort descending by priority fee per CU
    const sorted = [...existingContenders].sort((a, b) => b.priorityFee / b.cu - a.priorityFee / a.cu);

    let accumulatedCu = 0;
    let cutoffFeePerCu = 0;

    for (const item of sorted) {
      if (accumulatedCu + item.cu > 12_000_000 - requestedCu) {
        // This transaction is right at the 12M CU boundary
        cutoffFeePerCu = item.priorityFee / item.cu;
        break;
      }
      accumulatedCu += item.cu;
    }

    // Bid 10% above the cutoff fee to guarantee top-of-block inclusion
    const recommendedFeePerCu = Math.ceil(cutoffFeePerCu * 1.1) + 1;
    return recommendedFeePerCu * requestedCu;
  }

  /**
   * Dispatches automated telemetry to enterprise Slack
   */
  public async alertCongestion(account: string, currentCu: number) {
    const saturationPct = ((currentCu / 12_000_000) * 100).toFixed(1);
    await axios.post(this.slackWebhookUrl, {
      text: `🚨 *High Write-Lock Saturation Detected!* Account \`${account}\` is at *${saturationPct}%* of its 12M compute unit limit for the pending block.`,
    });
  }
}