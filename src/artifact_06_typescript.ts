#!/usr/bin/env node
import { Connection, BlockResponse, VersionedBlockResponse } from '@solana/web3.js';
import axios from 'axios';
import { Resend } from 'resend';
import { Command } from 'commander';
import * as dotenv from 'dotenv';

dotenv.config();

interface AnalysisMetrics {
  slot: number;
  totalTransactions: number;
  priorityFeeCount: number;
  jitoTipsIdentified: number;
  averagePriorityFeeMicroLamports: number;
  maxPriorityFee: number;
  potentialInversions: number;
  writeLockHotspots: Record<string, number>;
}

class ForensicsEngine {
  private connection: Connection;
  private resend: Resend | null = null;
  private slackWebhookUrl: string | null = null;

  constructor(rpcUrl: string, resendApiKey?: string, slackWebhook?: string) {
    this.connection = new Connection(rpcUrl, 'confirmed');
    if (resendApiKey) {
      this.resend = new Resend(resendApiKey);
    }
    if (slackWebhook) {
      this.slackWebhookUrl = slackWebhook;
    }
  }

  public async analyzeSlot(slot: number): Promise<AnalysisMetrics | null> {
    try {
      const block: VersionedBlockResponse | null = await this.connection.getBlock(slot, {
        maxSupportedTransactionVersion: 0,
        transactionDetails: 'full',
      });

      if (!block || !block.transactions) {
        console.warn(`[!] Slot ${slot} empty or unavailable.`);
        return null;
      }

      let priorityFeeCount = 0;
      let totalPriorityFee = 0;
      let maxPriorityFee = 0;
      let jitoTips = 0;
      let inversions = 0;
      const writeAccountCounts: Record<string, number> = {};

      let previousFeeRate = Infinity;

      for (let i = 0; i < block.transactions.length; i++) {
        const tx = block.transactions[i];
        const meta = tx.meta;
        if (!meta) continue;

        let computeUnitPrice = 0;
        let isJito = false;

        // Check for ComputeBudget program in compiled message instructions
        const staticAccountKeys = tx.transaction.message.staticAccountKeys.map((k) => k.toBase58());
        const hasComputeBudget = staticAccountKeys.some((k) =>
          k === 'ComputeBudget111111111111111111111111111111'
        );

        if (hasComputeBudget && meta.computeUnitsConsumed) {
          // Priority fee = total fee minus base 5000 lamport fee
          const feePaid = meta.fee;
          const priorityLamports = Math.max(0, feePaid - 5000);
          computeUnitPrice = Math.floor((priorityLamports * 1_000_000) / meta.computeUnitsConsumed);

          if (computeUnitPrice > 0) {
            priorityFeeCount++;
            totalPriorityFee += computeUnitPrice;
            if (computeUnitPrice > maxPriorityFee) {
              maxPriorityFee = computeUnitPrice;
            }
          }
        }

        // Tally write-locks
        const accountKeys = tx.transaction.message.staticAccountKeys;
        const header = tx.transaction.message.header;
        const numWriteSigners = header.numRequiredSignatures - header.numReadonlySignedAccounts;
        const numWriteNonSigners =
          accountKeys.length -
          header.numRequiredSignatures -
          header.numReadonlyUnsignedAccounts;

        for (let j = 0; j < numWriteSigners; j++) {
          const key = accountKeys[j].toBase58();
          writeAccountCounts[key] = (writeAccountCounts[key] || 0) + 1;
        }

        for (let j = header.numRequiredSignatures; j < header.numRequiredSignatures + numWriteNonSigners; j++) {
          const key = accountKeys[j].toBase58();
          writeAccountCounts[key] = (writeAccountCounts[key] || 0) + 1;
        }

        // Check for Jito Tip program matches
        const jitoTipAddresses = new Set([
          '96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5',
          'HFqU5x63VTxvQssQnCe2cvUueqcY3BWUB6UhRjdfRLit',
          'Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY',
          'ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt6iGPaW44',
          'DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh',
          'ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt',
          'DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL',
          '3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT',
        ]);

        for (const key of staticAccountKeys) {
          if (jitoTipAddresses.has(key)) {
            isJito = true;
            jitoTips++;
            break;
          }
        }

        // Priority Inversion Detection: A lower priority fee appears ahead of a higher fee
        // without a clear write-lock conflict.
        if (!isJito && computeUnitPrice > previousFeeRate) {
          inversions++;
        }
        previousFeeRate = computeUnitPrice;
      }

      // Find top 3 contested write accounts
      const sortedHotspots = Object.entries(writeAccountCounts)
        .sort((a, b) => b[1] - a[1])
        .slice(0, 3);

      const hotspotsObj: Record<string, number> = {};
      for (const [key, count] of sortedHotspots) {
        hotspotsObj[key] = count;
      }

      const metrics: AnalysisMetrics = {
        slot,
        totalTransactions: block.transactions.length,
        priorityFeeCount,
        jitoTipsIdentified: jitoTips,
        averagePriorityFeeMicroLamports:
          priorityFeeCount > 0 ? Math.floor(totalPriorityFee / priorityFeeCount) : 0,
        maxPriorityFee,
        potentialInversions: inversions,
        writeLockHotspots: hotspotsObj,
      };

      return metrics;
    } catch (err: any) {
      console.error(`[Error] Failed to analyze slot ${slot}:`, err.message);
      return null;
    }
  }

  public async emitAlerts(metrics: AnalysisMetrics, alertEmailTo?: string): Promise<void> {
    const summary = `
🚨 *Solana Block Forensics Alert* 🚨
• *Slot:* \`${metrics.slot}\`
• *Total TXs:* ${metrics.totalTransactions}
• *Priority TXs:* ${metrics.priorityFeeCount}
• *Jito Bundles:* ${metrics.jitoTipsIdentified}
• *Priority Inversions:* ${metrics.potentialInversions}
• *Max Priority Fee:* ${metrics.maxPriorityFee} µ-lamports/CU
• *Top Write Hotspots:* 
${Object.entries(metrics.writeLockHotspots)
  .map(([k, count]) => `  - \`${k}\`: ${count} writes`)
  .join('\n')}
    `;

    console.log(summary);

    // Slack Notification
    if (this.slackWebhookUrl) {
      try {
        await axios.post(this.slackWebhookUrl, { text: summary });
        console.log('[✓] Alert dispatched to Slack.');
      } catch (err: any) {
        console.error('[!] Failed to send Slack alert:', err.message);
      }
    }

    // Resend Email Dispatch
    if (this.resend && alertEmailTo && metrics.potentialInversions > 15) {
      try {
        await this.resend.emails.send({
          from: 'alerts@resend.dev',
          to: alertEmailTo,
          subject: `[High Inversion Alert] Solana Block Ordering Anomaly in Slot ${metrics.slot}`,
          html: `
            <h2>Solana Block Ordering Forensic Report</h2>
            <p><strong>Slot:</strong> ${metrics.slot}</p>
            <p><strong>Total Transactions:</strong> ${metrics.totalTransactions}</p>
            <p><strong>Potential Priority Inversions:</strong> ${metrics.potentialInversions}</p>
            <p><strong>Max Priority Fee:</strong> ${metrics.maxPriorityFee} &mu;-lamports/CU</p>
            <h3>Write Lock Contention Hotspots</h3>
            <pre>${JSON.stringify(metrics.writeLockHotspots, null, 2)}</pre>
          `,
        });
        console.log(`[✓] Diagnostic report emailed to ${alertEmailTo} via Resend.`);
      } catch (err: any) {
        console.error('[!] Failed to send email via Resend:', err.message);
      }
    }
  }
}

// CLI Command Parser Setup
const program = new Command();

program
  .name('sbe-forensics')
  .description('Solana Block Engine Forensics & Transaction Ordering CLI')
  .version('1.0.0');

program
  .command('monitor')
  .description('Continuously monitor finalized blocks for ordering, locks, and priority fees')
  .option('-r, --rpc <url>', 'Solana JSON-RPC URL', 'https://api.mainnet-beta.solana.com')
  .option('-s, --slack <url>', 'Slack Incoming Webhook URL')
  .option('-k, --resend-key <key>', 'Resend API Key')
  .option('-e, --email <email>', 'Recipient email for alert reports')
  .action(async (options) => {
    console.log('[*] Initializing SBE-Forensics Agent...');
    const engine = new ForensicsEngine(
      options.rpc || process.env.SOLANA_RPC_URL,
      options.resendKey || process.env.RESEND_API_KEY,
      options.slack || process.env.SLACK_WEBHOOK_URL
    );

    const connection = new Connection(options.rpc || 'https://api.mainnet-beta.solana.com');
    let lastSlot = await connection.getSlot('confirmed');
    console.log(`[*] Subscribed to blockstream starting from confirmed slot: ${lastSlot}`);

    setInterval(async () => {
      try {
        const currentSlot = await connection.getSlot('confirmed');
        if (currentSlot > lastSlot) {
          for (let slot = lastSlot + 1; slot <= currentSlot; slot++) {
            const metrics = await engine.analyzeSlot(slot);
            if (metrics) {
              await engine.emitAlerts(metrics, options.email || process.env.ALERT_EMAIL);
            }
          }
          lastSlot = currentSlot;
        }
      } catch (err: any) {
        console.error('[!] Loop error:', err.message);
      }
    }, 4000);
  });

program
  .command('inspect-slot <slot>')
  .description('Perform instant static forensics on a historical block slot')
  .option('-r, --rpc <url>', 'Solana JSON-RPC URL', 'https://api.mainnet-beta.solana.com')
  .action(async (slotStr, options) => {
    const slot = parseInt(slotStr, 10);
    console.log(`[*] Inspecting slot ${slot}...`);
    const engine = new ForensicsEngine(options.rpc);
    const metrics = await engine.analyzeSlot(slot);
    if (metrics) {
      console.log(JSON.stringify(metrics, null, 2));
    }
  });

program.parse(process.argv);