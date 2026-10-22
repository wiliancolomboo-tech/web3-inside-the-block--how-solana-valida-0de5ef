// File: src/cli/inspector.ts
import { Connection, PublicKey, BlockResponse, ConfirmedTransactionMeta } from '@solana/web3.js';
import axios from 'axios';
import { Command } from 'commander';

interface BlockInspectionMetrics {
  slot: number;
  blockTime: number | null | undefined;
  transactionCount: number;
  totalComputeUnits: number;
  totalPriorityFeesLamports: number;
  hotAccounts: Array<{ account: string; writeCount: number; estimatedCu: number }>;
  lockContentionIndex: number; // 0.0 - 1.0 (Higher means more concurrent write conflicts)
}

export class BlockOrderingInspector {
  private connection: Connection;
  private webhookUrl: string | undefined;

  constructor(rpcUrl: string, webhookUrl?: string) {
    this.connection = new Connection(rpcUrl, { commitment: 'confirmed' });
    this.webhookUrl = webhookUrl;
  }

  public async inspectSlot(slot: number): Promise<BlockInspectionMetrics> {
    const block: BlockResponse | null = await this.connection.getBlock(slot, {
      maxSupportedTransactionVersion: 0,
      transactionDetails: 'full',
      rewards: false,
    });

    if (!block || !block.transactions) {
      throw new Error(`Block at slot ${slot} could not be retrieved.`);
    }

    const accountWriteFrequency: Map<string, number> = new Map();
    let totalBlockCu = 0;
    let totalPriorityFees = 0;

    for (const tx of block.transactions) {
      const meta: ConfirmedTransactionMeta | null = tx.meta;
      if (!meta) continue;

      const cuConsumed = meta.computeUnitsConsumed || 200_000;
      totalBlockCu += cuConsumed;
      totalPriorityFees += (meta.fee > 5000 ? meta.fee - 5000 : 0);

      const accountKeys = tx.transaction.message.getAccountKeys();
      
      // Determine writable accounts from message header
      const header = tx.transaction.message.header;
      const numRequiredSignatures = header.numRequiredSignatures;
      const numReadonlySigned = header.numReadonlySignedAccounts;
      const numReadonlyUnsigned = header.numReadonlyUnsignedAccounts;
      const totalAccounts = accountKeys.length;

      const numWritableSigned = numRequiredSignatures - numReadonlySigned;
      const numWritableUnsigned = totalAccounts - numRequiredSignatures - numReadonlyUnsigned;

      // Extract writable public keys
      for (let i = 0; i < totalAccounts; i++) {
        const isWritable = (i < numWritableSigned) || (i >= numRequiredSignatures && i < numRequiredSignatures + numWritableUnsigned);
        if (isWritable) {
          const pubkeyStr = accountKeys.get(i)!.toBase58();
          accountWriteFrequency.set(pubkeyStr, (accountWriteFrequency.get(pubkeyStr) || 0) + 1);
        }
      }
    }

    // Sort hot accounts by write conflicts
    const hotAccounts = Array.from(accountWriteFrequency.entries())
      .map(([account, writeCount]) => ({
        account,
        writeCount,
        estimatedCu: writeCount * 200_000,
      }))
      .sort((a, b) => b.writeCount - a.writeCount)
      .slice(0, 5);

    // Compute Lock Contention Index (Ratio of writes concentrated on top 3 accounts)
    const topWrites = hotAccounts.slice(0, 3).reduce((acc, curr) => acc + curr.writeCount, 0);
    const totalWrites = Array.from(accountWriteFrequency.values()).reduce((a, b) => a + b, 0);
    const lockContentionIndex = totalWrites > 0 ? parseFloat((topWrites / totalWrites).toFixed(4)) : 0;

    const metrics: BlockInspectionMetrics = {
      slot,
      blockTime: block.blockTime,
      transactionCount: block.transactions.length,
      totalComputeUnits: totalBlockCu,
      totalPriorityFeesLamports: totalPriorityFees,
      hotAccounts,
      lockContentionIndex,
    };

    if (this.webhookUrl) {
      await this.dispatchEnterpriseAlert(metrics);
    }

    return metrics;
  }

  private async dispatchEnterpriseAlert(metrics: BlockInspectionMetrics): Promise<void> {
    const payload = {
      text: `🚨 *Solana Validator Block Inspection Report: Slot ${metrics.slot}*`,
      blocks: [
        {
          type: "section",
          text: {
            type: "mrkdwn",
            text: `*Slot:* ${metrics.slot}\n*Transactions Processed:* ${metrics.transactionCount}\n*Block Compute Load:* ${((metrics.totalComputeUnits / 48_000_000) * 100).toFixed(2)}% (48M CU limit)\n*Lock Contention Index:* ${metrics.lockContentionIndex} (High contention if > 0.35)\n*Total Priority Fees:* ${(metrics.totalPriorityFeesLamports / 1e9).toFixed(4)} SOL`
          }
        },
        {
          type: "section",
          text: {
            type: "mrkdwn",
            text: `*Top Contended Write Locks:*\n${metrics.hotAccounts.map((a, i) => `${i + 1}. \`${a.account}\` (${a.writeCount} writes)`).join("\n")}`
          }
        }
      ]
    };

    try {
      await axios.post(this.webhookUrl!, payload, { timeout: 5000 });
    } catch (err: any) {
      console.error(`[Alert Dispatch Failure]: ${err.message}`);
    }
  }
}

// CLI Execution Entrypoint
const program = new Command();
program
  .name('solana-block-inspector')
  .description('Deep Transaction Ordering & Account Lock Contention Inspector')
  .version('1.0.0');

program
  .command('analyze')
  .description('Inspect a specific slot or the latest finalized block')
  .option('-r, --rpc <url>', 'Solana RPC Endpoint', 'https://api.mainnet-beta.solana.com')
  .option('-s, --slot <number>', 'Slot number to analyze')
  .option('-w, --webhook <url>', 'Slack/Discord Enterprise Webhook URL')
  .action(async (options) => {
    const inspector = new BlockOrderingInspector(options.rpc, options.webhook);
    try {
      const slot = options.slot ? parseInt(options.slot, 10) : await (await new Connection(options.rpc)).getSlot();
      console.log(`[INFO] Analyzing Block Slot: ${slot}...`);
      const results = await inspector.inspectSlot(slot);
      console.log(JSON.stringify(results, null, 2));
    } catch (e: any) {
      console.error(`[ERROR]: ${e.message}`);
      process.exit(1);
    }
  });

program.parse(process.argv);