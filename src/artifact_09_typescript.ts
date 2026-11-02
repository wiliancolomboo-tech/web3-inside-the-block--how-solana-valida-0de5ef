import { Connection, BlockResponse, ParsedTransactionWithMeta } from '@solana/web3.js';
import axios from 'axios';
import * as dotenv from 'dotenv';
import * as http from 'http';
import { Registry, Counter, Gauge, collectDefaultMetrics } from 'prom-client';

dotenv.config();

const SOLANA_RPC_ENDPOINT = process.env.SOLANA_RPC_ENDPOINT || 'https://api.mainnet-beta.solana.com';
const DISCORD_WEBHOOK_URL = process.env.DISCORD_WEBHOOK_URL || '';
const METRICS_PORT = parseInt(process.env.METRICS_PORT || '9090', 10);

// Prometheus Metric Setup
const registry = new Registry();
collectDefaultMetrics({ register: registry });

const totalBlocksAnalyzed = new Counter({
  name: 'solana_blocks_analyzed_total',
  help: 'Total number of confirmed blocks ingested and analyzed for ordering',
  registers: [registry],
});

const lockContentionScore = new Gauge({
  name: 'solana_account_lock_contention_ratio',
  help: 'Ratio of transactions targeting the top 5 most contended writable accounts in the block',
  labelNames: ['slot'],
  registers: [registry],
});

const avgPriorityFeeGauge = new Gauge({
  name: 'solana_avg_priority_fee_micro_lamports',
  help: 'Average Compute Unit Price in micro-lamports within the block',
  labelNames: ['slot'],
  registers: [registry],
});

export interface BlockAnalysisResult {
  slot: u64 | number;
  totalTransactions: number;
  voteTransactions: number;
  nonVoteTransactions: number;
  topContendedAccounts: { pubkey: string; writeCount: number }[];
  averagePriorityFee: number;
  contentionRatio: number;
}

export class BlockOrderingAuditor {
  private connection: Connection;

  constructor(rpcUrl: string) {
    this.connection = new Connection(rpcUrl, {
      commitment: 'confirmed',
      disableRetryOnRateLimit: false,
    });
  }

  public async analyzeSlot(slot: number): Promise<BlockAnalysisResult | null> {
    try {
      const block: BlockResponse | null = await this.connection.getBlock(slot, {
        maxSupportedTransactionVersion: 0,
        rewards: false,
        transactionDetails: 'full',
      });

      if (!block || !block.transactions) {
        return null;
      }

      let voteTxCount = 0;
      let nonVoteTxCount = 0;
      let totalPriorityFeeSum = 0;
      let nonVoteWithBudgetCount = 0;
      const writeAccountFrequencies: Map<string, number> = new Map();

      for (const tx of block.transactions) {
        const message = tx.transaction.message;
        const accountKeys = message.getAccountKeys();
        
        // Detect vote transaction (touches Vote Program)
        const isVote = accountKeys.staticAccountKeys.some(
          (key) => key.toBase58() === 'Vote111111111111111111111111111111111111111'
        );

        if (isVote) {
          voteTxCount++;
          continue;
        }

        nonVoteTxCount++;

        // Track write locks across non-vote transactions
        accountKeys.staticAccountKeys.forEach((key, idx) => {
          if (message.isAccountWritable(idx)) {
            const pk = key.toBase58();
            writeAccountFrequencies.set(pk, (writeAccountFrequencies.get(pk) || 0) + 1);
          }
        });

        // Compute Unit Price Extraction from ComputeBudgetProgram instructions
        const instructions = message.compiledInstructions;
        for (const ix of instructions) {
          const programId = accountKeys.staticAccountKeys[ix.programIdIndex]?.toBase58();
          if (programId === 'ComputeBudget111111111111111111111111111111') {
            // Discriminator 3 corresponds to SetComputeUnitPrice: [3, ...8-byte u64 le]
            if (ix.data[0] === 3 && ix.data.length >= 9) {
              const view = new DataView(Uint8Array.from(ix.data).buffer);
              const microLamports = Number(view.getBigUint64(1, true));
              totalPriorityFeeSum += microLamports;
              nonVoteWithBudgetCount++;
            }
          }
        }
      }

      // Sort accounts by write contention
      const sortedContention = Array.from(writeAccountFrequencies.entries())
        .map(([pubkey, writeCount]) => ({ pubkey, writeCount }))
        .sort((a, b) => b.writeCount - a.writeCount);

      const top5Contended = sortedContention.slice(0, 5);
      const top5WritesSum = top5Contended.reduce((acc, curr) => acc + curr.writeCount, 0);
      const contentionRatio = nonVoteTxCount > 0 ? top5WritesSum / (nonVoteTxCount * 3) : 0;
      const averagePriorityFee = nonVoteWithBudgetCount > 0 ? totalPriorityFeeSum / nonVoteWithBudgetCount : 0;

      // Update Prometheus Metrics
      totalBlocksAnalyzed.inc();
      lockContentionScore.set({ slot: slot.toString() }, contentionRatio);
      avgPriorityFeeGauge.set({ slot: slot.toString() }, averagePriorityFee);

      return {
        slot,
        totalTransactions: block.transactions.length,
        voteTransactions: voteTxCount,
        nonVoteTransactions: nonVoteTxCount,
        topContendedAccounts: top5Contended,
        averagePriorityFee,
        contentionRatio,
      };
    } catch (err: any) {
      console.error(`[AUDITOR ERROR] Failed to fetch or parse slot ${slot}:`, err.message);
      return null;
    }
  }

  public async notifyContentionAlert(result: BlockAnalysisResult): Promise<void> {
    if (!DISCORD_WEBHOOK_URL) return;

    if (result.contentionRatio > 0.45) {
      const payload = {
        username: 'Solana Validator Banking Stage Monitor',
        avatar_url: 'https://solana.com/src/img/branding/solanaLogoMark.png',
        embeds: [
          {
            title: `⚠️ High Account Write Lock Contention Detected in Slot ${result.slot}`,
            color: 0xff3b30,
            fields: [
              { name: 'Total Non-Vote Txs', value: `${result.nonVoteTransactions}`, inline: true },
              { name: 'Contention Ratio', value: `${(result.contentionRatio * 100).toFixed(2)}%`, inline: true },
              { name: 'Avg Priority Fee', value: `${result.averagePriorityFee.toFixed(0)} µL/CU`, inline: true },
              {
                name: 'Most Contended Writable Account (Hotspot)',
                value: `\`${result.topContendedAccounts[0]?.pubkey || 'None'}\` (${result.topContendedAccounts[0]?.writeCount || 0} writes)`,
              },
            ],
            footer: { text: 'Solana TPU Scheduling & Ordering Telemetry Service' },
            timestamp: new Date().toISOString(),
          },
        ],
      };

      try {
        await axios.post(DISCORD_WEBHOOK_URL, payload);
      } catch (err: any) {
        console.error('[WEBHOOK ERROR] Unable to dispatch alert:', err.message);
      }
    }
  }
}

// Service Runner
async function main() {
  console.log(`[BOOT] Initializing Solana Block Ordering Telemetry Daemon...`);
  console.log(`[BOOT] RPC Target: ${SOLANA_RPC_ENDPOINT}`);

  const auditor = new BlockOrderingAuditor(SOLANA_RPC_ENDPOINT);

  // Prometheus Exporter Server
  const server = http.createServer(async (req, res) => {
    if (req.url === '/metrics') {
      res.writeHead(200, { 'Content-Type': registry.contentType });
      res.end(await registry.metrics());
    } else {
      res.writeHead(404);
      res.end();
    }
  });

  server.listen(METRICS_PORT, () => {
    console.log(`[PROMETHEUS] Metrics server listening on http://0.0.0.0:${METRICS_PORT}/metrics`);
  });

  // Polling block loop
  let lastProcessedSlot = 0;
  setInterval(async () => {
    try {
      const currentSlot = await auditor['connection'].getSlot('confirmed');
      if (currentSlot > lastProcessedSlot) {
        if (lastProcessedSlot === 0) lastProcessedSlot = currentSlot - 1;
        
        for (let s = lastProcessedSlot + 1; s <= currentSlot; s++) {
          const result = await auditor.analyzeSlot(s);
          if (result) {
            console.log(
              `[SLOT ${result.slot}] Non-Vote: ${result.nonVoteTransactions} | Avg Fee: ${result.averagePriorityFee.toFixed(1)} µL/CU | Hot Account: ${result.topContendedAccounts[0]?.pubkey.slice(0, 8)}... (${result.topContendedAccounts[0]?.writeCount} writes)`
            );
            await auditor.notifyContentionAlert(result);
          }
          lastProcessedSlot = s;
        }
      }
    } catch (e: any) {
      console.warn(`[POLL WARNING] Exception in main loop: ${e.message}`);
    }
  }, 1000);
}

if (require.main === module) {
  main();
}