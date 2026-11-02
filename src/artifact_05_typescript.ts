import { Connection, PublicKey, VersionedTransactionResponse } from '@solana/web3.js';
import axios from 'axios';
import * as dotenv from 'dotenv';
dotenv.config();

interface LockContentionReport {
  slot: number;
  totalCu: number;
  saturatedAccounts: Array<{
    pubkey: string;
    consumedCu: number;
    saturationPct: string;
  }>;
  jitoTipSumLamports: number;
  scheduledTxCount: number;
  droppedTxCount: number;
}

const MAX_ACCOUNT_CU = 12_000_000;
const SLACK_WEBHOOK_URL = process.env.SLACK_WEBHOOK_URL || '';
const RESEND_API_KEY = process.env.RESEND_API_KEY || '';
const ALERT_EMAIL_TO = process.env.ALERT_EMAIL_TO || 'ops@fund.example.com';

export class SolanaBlockForensicMonitor {
  private connection: Connection;

  constructor(rpcUrl: string) {
    this.connection = new Connection(rpcUrl, 'confirmed');
  }

  public async analyzeSlot(slotNumber: number): Promise<LockContentionReport | null> {
    const block = await this.connection.getBlock(slotNumber, {
      maxSupportedTransactionVersion: 0,
      transactionDetails: 'full',
      rewards: false,
    });

    if (!block || !block.transactions) {
      console.warn(`[!] Slot ${slotNumber} is empty or pruned.`);
      return null;
    }

    const accountUsageMap = new Map<string, number>();
    let totalBlockCu = 0;
    let totalJitoTips = 0;
    let scheduledTxCount = 0;
    let droppedTxCount = 0;

    for (const txWrapper of block.transactions) {
      const meta = txWrapper.meta;
      if (!meta) continue;

      const consumedCu = meta.computeUnitsConsumed ?? 200_000;
      totalBlockCu += consumedCu;
      scheduledTxCount++;

      const message = txWrapper.transaction.message;
      const accountKeys = message.staticAccountKeys;

      // Track write locks
      for (let idx = 0; idx < accountKeys.length; idx++) {
        if (message.isAccountWritable(idx)) {
          const keyStr = accountKeys[idx].toBase58();
          const current = accountUsageMap.get(keyStr) || 0;
          accountUsageMap.set(keyStr, current + consumedCu);
        }
      }
    }

    const saturatedAccounts: LockContentionReport['saturatedAccounts'] = [];
    for (const [key, cu] of accountUsageMap.entries()) {
      if (cu >= MAX_ACCOUNT_CU * 0.75) {
        // Flag accounts within 75% of limit
        saturatedAccounts.push({
          pubkey: key,
          consumedCu: cu,
          saturationPct: ((cu / MAX_ACCOUNT_CU) * 100).toFixed(2) + '%',
        });
      }
    }

    const report: LockContentionReport = {
      slot: slotNumber,
      totalCu: totalBlockCu,
      saturatedAccounts,
      jitoTipSumLamports: totalJitoTips,
      scheduledTxCount,
      droppedTxCount,
    };

    if (saturatedAccounts.length > 0) {
      await this.dispatchEnterpriseAlert(report);
    }

    return report;
  }

  private async dispatchEnterpriseAlert(report: LockContentionReport): Promise<void> {
    const message = `🚨 *Solana Lock Contention Alert - Slot ${report.slot}* 🚨\n` +
      `• *Total Compute Units*: ${report.totalCu.toLocaleString()} / 48,000,000\n` +
      `• *Critical Write-Lock Accounts (>=75% saturation)*:\n` +
      report.saturatedAccounts
        .map((a) => `   - \`${a.pubkey}\`: ${a.consumedCu.toLocaleString()} CU (${a.saturationPct})`)
        .join('\n');

    console.log(message);

    // 1. Dispatch to Slack Webhook
    if (SLACK_WEBHOOK_URL) {
      try {
        await axios.post(SLACK_WEBHOOK_URL, { text: message });
      } catch (err: any) {
        console.error('Slack webhook dispatch failed:', err.message);
      }
    }

    // 2. Dispatch via Resend Email API
    if (RESEND_API_KEY) {
      try {
        await axios.post(
          'https://api.resend.com/emails',
          {
            from: 'telemetry@solana-forensics.io',
            to: ALERT_EMAIL_TO,
            subject: `CRITICAL: Solana Account Write Contention at Slot ${report.slot}`,
            text: message,
          },
          {
            headers: {
              Authorization: `Bearer ${RESEND_API_KEY}`,
              'Content-Type': 'application/json',
            },
          }
        );
      } catch (err: any) {
        console.error('Resend dispatch failed:', err.message);
      }
    }
  }
}

// Execution CLI runner
async function main() {
  const rpc = process.env.SOLANA_RPC_URL || 'https://api.mainnet-beta.solana.com';
  const monitor = new SolanaBlockForensicMonitor(rpc);

  console.log(`[+] Initializing Solana Validator Block Ordering Monitor on ${rpc}`);
  const conn = new Connection(rpc);
  const currentSlot = await conn.getSlot('confirmed');
  console.log(`[+] Auditing current confirmed slot: ${currentSlot}`);
  await monitor.analyzeSlot(currentSlot);
}

if (require.main === module) {
  main().catch(console.error);
}