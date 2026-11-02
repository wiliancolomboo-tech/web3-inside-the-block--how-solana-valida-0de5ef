import { Connection, BlockResponse, VersionedBlockResponse } from '@solana/web3.js';
import axios from 'axios';
import * as dotenv from 'dotenv';

dotenv.config();

interface BlockMetrics {
  slot: number;
  totalTransactions: number;
  voteTransactions: number;
  nonVoteTransactions: number;
  totalComputeUnitsConsumed: number;
  writableAccountsContention: Record<string, number>;
  topContendedAccount: string;
  maxContentionHits: number;
  estimatedPriorityFeesLamports: number;
}

class EnterpriseBlockInspector {
  private connection: Connection;
  private slackWebhookUrl?: string;
  private discordWebhookUrl?: string;
  private resendApiKey?: string;
  private resendRecipientEmail?: string;

  constructor() {
    const rpcUrl = process.env.SOLANA_RPC_URL || 'https://api.mainnet-beta.solana.com';
    this.connection = new Connection(rpcUrl, { commitment: 'confirmed' });
    this.slackWebhookUrl = process.env.SLACK_WEBHOOK_URL;
    this.discordWebhookUrl = process.env.DISCORD_WEBHOOK_URL;
    this.resendApiKey = process.env.RESEND_API_KEY;
    this.resendRecipientEmail = process.env.RESEND_RECIPIENT_EMAIL;

    console.log(`[Inspector Engine] Initialized against RPC: ${rpcUrl}`);
  }

  public async inspectSlot(slot: number): Promise<BlockMetrics | null> {
    try {
      const block = await this.connection.getBlock(slot, {
        maxSupportedTransactionVersion: 0,
        rewards: false,
        transactionDetails: 'full',
      });

      if (!block) {
        console.warn(`[Inspector Engine] Slot ${slot} not found or skipped.`);
        return null;
      }

      return this.decomposeBlock(slot, block);
    } catch (err: any) {
      console.error(`[Inspector Engine] Failed to fetch slot ${slot}:`, err.message);
      return null;
    }
  }

  private decomposeBlock(slot: number, block: VersionedBlockResponse): BlockMetrics {
    let nonVoteCount = 0;
    let voteCount = 0;
    let totalCU = 0;
    const accountLockHeatmap: Record<string, number> = {};

    for (const tx of block.transactions) {
      const message = tx.transaction.message;
      const accountKeys = message.getAccountKeys();
      
      // Heuristic detection: vote program account
      const isVote = accountKeys.staticAccountKeys.some(
        (key) => key.toBase58() === 'Vote111111111111111111111111111111111111111'
      );

      if (isVote) {
        voteCount++;
        continue;
      }

      nonVoteCount++;

      // Track compute units consumed if metadata is present
      if (tx.meta && tx.meta.computeUnitsConsumed) {
        totalCU += tx.meta.computeUnitsConsumed;
      }

      // Analyze account lock pressure for writable keys
      const numWritable = message.header.numRequiredSignatures - message.header.numReadonlySignedAccounts;
      for (let i = 0; i < numWritable; i++) {
        const pubkey = accountKeys.get(i)?.toBase58();
        if (pubkey) {
          accountLockHeatmap[pubkey] = (accountLockHeatmap[pubkey] || 0) + 1;
        }
      }
    }

    let topContendedAccount = 'None';
    let maxContentionHits = 0;
    for (const [pubkey, count] of Object.entries(accountLockHeatmap)) {
      if (count > maxContentionHits) {
        maxContentionHits = count;
        topContendedAccount = pubkey;
      }
    }

    return {
      slot,
      totalTransactions: block.transactions.length,
      voteTransactions: voteCount,
      nonVoteTransactions: nonVoteCount,
      totalComputeUnitsConsumed: totalCU,
      writableAccountsContention: accountLockHeatmap,
      topContendedAccount,
      maxContentionHits,
      estimatedPriorityFeesLamports: nonVoteCount * 5_000,
    };
  }

  public async dispatchAlerts(metrics: BlockMetrics): Promise<void> {
    const alertMessage = 
      `🚨 *Solana Validator Scheduling Alert* | Slot: *${metrics.slot}*\n` +
      `• *Non-Vote Txs:* ${metrics.nonVoteTransactions} (Total: ${metrics.totalTransactions})\n` +
      `• *Total CU Consumed:* ${metrics.totalComputeUnitsConsumed.toLocaleString()} / 48,000,000\n` +
      `• *Hot Writable Key:* \`${metrics.topContendedAccount}\` (${metrics.maxContentionHits} writes serialized)\n` +
      `• *Lock Contention Ratio:* ${((metrics.maxContentionHits / (metrics.nonVoteTransactions || 1)) * 100).toFixed(2)}%`;

    // 1. Dispatch to Slack
    if (this.slackWebhookUrl) {
      try {
        await axios.post(this.slackWebhookUrl, { text: alertMessage });
        console.log(`[Alert Mesh] Dispatched to Slack.`);
      } catch (err: any) {
        console.error(`[Alert Mesh] Slack Webhook Error:`, err.message);
      }
    }

    // 2. Dispatch to Discord
    if (this.discordWebhookUrl) {
      try {
        await axios.post(this.discordWebhookUrl, { content: alertMessage });
        console.log(`[Alert Mesh] Dispatched to Discord.`);
      } catch (err: any) {
        console.error(`[Alert Mesh] Discord Webhook Error:`, err.message);
      }
    }

    // 3. Dispatch to Resend API
    if (this.resendApiKey && this.resendRecipientEmail) {
      try {
        await axios.post(
          'https://api.resend.com/emails',
          {
            from: 'telemetry@solana-sequencer.enterprise.internal',
            to: [this.resendRecipientEmail],
            subject: `Solana High-Contention Slot Alert: ${metrics.slot}`,
            html: `
              <h2>Solana Slot Sequencing Analysis</h2>
              <p><strong>Slot:</strong> ${metrics.slot}</p>
              <p><strong>Compute Units:</strong> ${metrics.totalComputeUnitsConsumed} / 48,000,000</p>
              <p><strong>Top Contended Account:</strong> <code>${metrics.topContendedAccount}</code></p>
              <p><strong>Consecutive Write Requests:</strong> ${metrics.maxContentionHits}</p>
              <p><em>Immediate action: Adjust dynamic priority fees or randomize intermediate state accounts.</em></p>
            `,
          },
          {
            headers: {
              Authorization: `Bearer ${this.resendApiKey}`,
              'Content-Type': 'application/json',
            },
          }
        );
        console.log(`[Alert Mesh] Dispatched telemetry email via Resend.`);
      } catch (err: any) {
        console.error(`[Alert Mesh] Resend API Error:`, err.response?.data || err.message);
      }
    }
  }

  public async runMonitorLoop(pollIntervalMs: number = 3000): Promise<void> {
    console.log(`[Monitor Loop] Continuous polling started...`);
    let lastSlot = 0;

    setInterval(async () => {
      try {
        const currentSlot = await this.connection.getSlot('confirmed');
        if (currentSlot > lastSlot) {
          lastSlot = currentSlot;
          console.log(`\n--- Inspecting Confirmed Slot: ${currentSlot} ---`);
          const metrics = await this.inspectSlot(currentSlot);
          if (metrics) {
            console.log(`Transactions: ${metrics.totalTransactions} (${metrics.nonVoteTransactions} non-vote)`);
            console.log(`Peak Contended Lock: ${metrics.topContendedAccount} -> ${metrics.maxContentionHits} writes`);
            
            // Trigger alerts when high write serialization occurs
            if (metrics.maxContentionHits > 40) {
              await this.dispatchAlerts(metrics);
            }
          }
        }
      } catch (err: any) {
        console.error(`[Monitor Loop] Error:`, err.message);
      }
    }, pollIntervalMs);
  }
}

// Execution Entrypoint
if (require.main === module) {
  const inspector = new EnterpriseBlockInspector();
  inspector.runMonitorLoop(2000);
}

export { EnterpriseBlockInspector, BlockMetrics };