import {
  Connection,
  ParsedTransactionWithMeta,
  PublicKey,
  VersionedBlockResponse
} from '@solana/web3.js';
import axios from 'axios';
import * as dotenv from 'dotenv';

dotenv.config();

interface ContentionMetrics {
  slot: number;
  totalTransactions: number;
  computeBudgetTxs: number;
  averagePriorityFee: number;
  topWriteLockHotspots: { account: string; writeCount: number }[];
  contentiousTransactions: number;
}

const RPC_HTTP_URL = process.env.SOLANA_RPC_HTTP || 'https://api.mainnet-beta.solana.com';
const WEBHOOK_URL = process.env.ENTERPRISE_WEBHOOK_URL || '';
const CONTENTION_ALERT_THRESHOLD = parseInt(process.env.CONTENTION_ALERT_THRESHOLD || '15', 10);

const COMPUTE_BUDGET_PROGRAM_ID = 'ComputeBudget111111111111111111111111111111';

export class BlockOrderTelemetryService {
  private connection: Connection;

  constructor() {
    this.connection = new Connection(RPC_HTTP_URL, {
      commitment: 'confirmed',
      disableRetryOnRateLimit: false,
    });
  }

  public async startInspector(): Promise<void> {
    console.log(`[SBIS-Telemetry] Connected to Solana RPC: ${RPC_HTTP_URL}`);
    console.log(`[SBIS-Telemetry] Alert threshold: ${CONTENTION_ALERT_THRESHOLD} writes/account`);

    this.connection.onSlotUpdate((slotUpdate) => {
      if (slotUpdate.type === 'completed') {
        this.inspectSlot(slotUpdate.slot).catch((err) => {
          console.error(`[Error] Slot ${slotUpdate.slot} analysis failed:`, err.message);
        });
      }
    });
  }

  public async inspectSlot(slot: number): Promise<void> {
    let block: VersionedBlockResponse | null = null;
    try {
      block = await this.connection.getBlock(slot, {
        maxSupportedTransactionVersion: 0,
        rewards: false,
        transactionDetails: 'full',
      });
    } catch {
      // Slot was skipped or unavailable on this specific RPC node
      return;
    }

    if (!block || !block.transactions) {
      return;
    }

    const writeLockFrequency = new Map<string, number>();
    let totalPriorityFees = 0;
    let computeBudgetCount = 0;
    let contentiousCount = 0;

    for (const tx of block.transactions) {
      const message = tx.transaction.message;
      const accountKeys = message.getAccountKeys({
        accountKeysFromLookups: tx.meta?.loadedAddresses,
      });

      let hasComputeBudget = false;

      // Extract account locks
      for (let i = 0; i < accountKeys.length; i++) {
        const isWritable = message.isAccountWritable(i);
        const pubkeyStr = accountKeys.get(i)!.toBase58();

        if (pubkeyStr === COMPUTE_BUDGET_PROGRAM_ID) {
          hasComputeBudget = true;
        }

        if (isWritable) {
          const currentCount = writeLockFrequency.get(pubkeyStr) || 0;
          writeLockFrequency.set(pubkeyStr, currentCount + 1);
        }
      }

      if (hasComputeBudget) {
        computeBudgetCount++;
        // Rough priority fee extraction from paid transaction fees - base 5000 lamports
        const paidFee = tx.meta?.fee || 5000;
        const priorityFee = Math.max(0, paidFee - 5000);
        totalPriorityFees += priorityFee;
      }
    }

    // Rank hotspots
    const hotspots = Array.from(writeLockFrequency.entries())
      .map(([account, writeCount]) => ({ account, writeCount }))
      .sort((a, b) => b.writeCount - a.writeCount)
      .slice(0, 5);

    // Transactions hitting the top hotspot are strictly serialized
    if (hotspots.length > 0 && hotspots[0].writeCount >= CONTENTION_ALERT_THRESHOLD) {
      contentiousCount = hotspots[0].writeCount;
      const metrics: ContentionMetrics = {
        slot,
        totalTransactions: block.transactions.length,
        computeBudgetTxs: computeBudgetCount,
        averagePriorityFee: computeBudgetCount > 0 ? Math.round(totalPriorityFees / computeBudgetCount) : 0,
        topWriteLockHotspots: hotspots,
        contentiousTransactions: contentiousCount,
      };

      await this.dispatchEnterpriseAlert(metrics);
    }
  }

  private async dispatchEnterpriseAlert(metrics: ContentionMetrics): Promise<void> {
    console.warn(`[ALERT] High Account Contention in Slot ${metrics.slot}! Hotspot: ${metrics.topWriteLockHotspots[0].account}`);

    if (!WEBHOOK_URL) {
      console.log('[Notice] No ENTERPRISE_WEBHOOK_URL set. Skipping external POST.');
      return;
    }

    const payload = {
      content: `🚨 **Solana BankingStage Contention Alert: Slot #${metrics.slot}**`,
      embeds: [
        {
          title: 'Account Write-Lock Contention Spike Detected',
          color: 16711680,
          fields: [
            { name: 'Slot', value: `${metrics.slot}`, inline: true },
            { name: 'Total Block Txs', value: `${metrics.totalTransactions}`, inline: true },
            { name: 'Avg Priority Fee (Lamports)', value: `${metrics.averagePriorityFee}`, inline: true },
            {
              name: 'Primary Contended Account (Write Bottleneck)',
              value: `\`${metrics.topWriteLockHotspots[0].account}\` (${metrics.topWriteLockHotspots[0].writeCount} serialized writes)`,
            },
            {
              name: 'Top Hotspots Matrix',
              value: metrics.topWriteLockHotspots
                .map((h, i) => `${i + 1}. \`${h.account.slice(0, 16)}...\`: **${h.writeCount} writes**`)
                .join('\n'),
            },
          ],
          footer: { text: 'SBIS Real-Time Validator Telemetry Engine' },
          timestamp: new Date().toISOString(),
        },
      ],
    };

    try {
      await axios.post(WEBHOOK_URL, payload, { timeout: 4000 });
      console.log(`[SBIS-Telemetry] Alert successfully dispatched to enterprise webhook.`);
    } catch (err: any) {
      console.error('[Error] Failed to dispatch webhook alert:', err.message);
    }
  }
}

// Direct Execution Hook
if (require.main === module) {
  const service = new BlockOrderTelemetryService();
  service.startInspector().catch((err) => {
    console.error('Fatal Telemetry Error:', err);
    process.exit(1);
  });
}