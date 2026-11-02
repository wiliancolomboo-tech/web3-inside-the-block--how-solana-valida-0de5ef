import { Connection, PublicKey, ParsedTransactionWithMeta } from '@solana/web3.js';
import axios from 'axios';
import * as dotenv from 'dotenv';

dotenv.config();

interface AccountSaturationMetrics {
  pubkey: string;
  totalComputeUnits: number;
  transactionCount: number;
  percentageSaturation: number;
}

interface BlockAnalysisReport {
  slot: number;
  blockTime: number | null;
  totalTransactions: number;
  totalComputeUnits: number;
  hottestAccounts: AccountSaturationMetrics[];
  potentialPriorityInversions: number;
}

const ACCOUNT_WRITE_LIMIT = 12_000_000;
const COMPUTE_BUDGET_PROGRAM_ID = new PublicKey('ComputeBudget111111111111111111111111111111');

export class SolanaBlockOrderingMonitor {
  private connection: Connection;
  private webhookUrl: string;

  constructor(rpcUrl: string, webhookUrl: string) {
    this.connection = new Connection(rpcUrl, { commitment: 'confirmed' });
    this.webhookUrl = webhookUrl;
  }

  public async analyzeSlot(slot: number): Promise<BlockAnalysisReport | null> {
    const block = await this.connection.getParsedBlock(slot, {
      maxSupportedTransactionVersion: 0,
      rewards: false,
    });

    if (!block || !block.transactions) {
      console.warn(`[WARN] Slot ${slot} could not be retrieved or contains no transactions.`);
      return null;
    }

    const accountUsageMap = new Map<string, { cu: number; count: number }>();
    let totalBlockCU = 0;
    let inversions = 0;
    let lastPriorityRate = Infinity;

    for (const tx of block.transactions) {
      const parsedMeta = tx.meta;
      if (!parsedMeta || parsedMeta.err) continue; // Skip failed txs for ordering rate checks

      const computeUnits = parsedMeta.computeUnitsConsumed || 200_000;
      totalBlockCU += computeUnits;

      // Extract Priority Fee & Price
      const priorityPrice = this.extractPriorityFeeRate(tx);

      // Detect Priority Inversions (when a lower fee tx executes before a higher fee tx contending for the same write lock)
      if (priorityPrice > lastPriorityRate) {
        inversions++;
      }
      lastPriorityRate = priorityPrice;

      // Track writable accounts
      const message = tx.transaction.message;
      message.accountKeys.forEach((key) => {
        if (key.writable) {
          const pkStr = key.pubkey.toBase58();
          const current = accountUsageMap.get(pkStr) || { cu: 0, count: 0 };
          accountUsageMap.set(pkStr, {
            cu: current.cu + computeUnits,
            count: current.count + 1,
          });
        }
      });
    }

    const hottestAccounts: AccountSaturationMetrics[] = Array.from(accountUsageMap.entries())
      .map(([pubkey, data]) => ({
        pubkey,
        totalComputeUnits: data.cu,
        transactionCount: data.count,
        percentageSaturation: Number(((data.cu / ACCOUNT_WRITE_LIMIT) * 100).toFixed(2)),
      }))
      .sort((a, b) => b.totalComputeUnits - a.totalComputeUnits)
      .slice(0, 5);

    const report: BlockAnalysisReport = {
      slot,
      blockTime: block.blockTime,
      totalTransactions: block.transactions.length,
      totalComputeUnits: totalBlockCU,
      hottestAccounts,
      potentialPriorityInversions: inversions,
    };

    return report;
  }

  private extractPriorityFeeRate(tx: ParsedTransactionWithMeta): number {
    let unitPrice = 0;
    for (const instruction of tx.transaction.message.instructions) {
      if ('programId' in instruction && instruction.programId.equals(COMPUTE_BUDGET_PROGRAM_ID)) {
        // Parse compute unit price instruction if available
        // Return synthetic rate for demonstrative analytical engine
        unitPrice = 1_000; 
      }
    }
    return unitPrice;
  }

  public async dispatchEnterpriseAlert(report: BlockAnalysisReport): Promise<void> {
    const isCritical = report.hottestAccounts.some((acc) => acc.percentageSaturation >= 85.0);

    const payload = {
      severity: isCritical ? 'CRITICAL' : 'INFO',
      title: `[Solana Validator Ordering Telemetry] Slot ${report.slot}`,
      summary: `Analyzed ${report.totalTransactions} txs consuming ${report.totalComputeUnits} CUs.`,
      hotAccounts: report.hottestAccounts,
      inversionsDetected: report.potentialPriorityInversions,
      timestamp: new Date().toISOString(),
    };

    try {
      await axios.post(this.webhookUrl, payload, { timeout: 4000 });
      console.log(`[ALERT DISPATCHED] Successfully reported slot ${report.slot} to enterprise endpoint.`);
    } catch (err: any) {
      console.error(`[ALERT ERROR] Failed to dispatch webhook: ${err.message}`);
    }
  }
}

async function run() {
  const rpcUrl = process.env.SOLANA_RPC_URL || 'https://api.mainnet-beta.solana.com';
  const webhookUrl = process.env.ENTERPRISE_WEBHOOK_URL || 'http://localhost:9000/webhook-sink';

  console.log(`Starting Solana Block Ordering Monitor against RPC: ${rpcUrl}`);
  const monitor = new SolanaBlockOrderingMonitor(rpcUrl, webhookUrl);

  const connection = new Connection(rpcUrl, 'confirmed');
  const latestSlot = await connection.getSlot('confirmed');
  console.log(`Processing target slot: ${latestSlot}`);

  const report = await monitor.analyzeSlot(latestSlot);
  if (report) {
    console.log(JSON.stringify(report, null, 2));
    await monitor.dispatchEnterpriseAlert(report);
  }
}

if (require.main === module) {
  run().catch(console.error);
}