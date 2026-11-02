import { Connection, BlockResponse, VersionedBlockResponse } from "@solana/web3.js";
import axios from "axios";
import * as dotenv from "dotenv";

dotenv.config();

interface AccountContentionMetrics {
  pubkey: string;
  totalCuRequested: number;
  writeHitCount: number;
  readHitCount: number;
}

interface BlockAuditReport {
  slot: number;
  blockhash: string;
  totalTransactions: number;
  totalComputeUnits: number;
  totalPriorityFeesLamports: number;
  highestContentionAccounts: AccountContentionMetrics[];
  topPriorityFees: number[];
}

export class SolanaBlockTelemetryAuditor {
  private connection: Connection;
  private webhookUrl: string | undefined;

  constructor(rpcUrl: string, webhookUrl?: string) {
    this.connection = new Connection(rpcUrl, {
      commitment: "confirmed",
      confirmTransactionInitialTimeout: 30000,
    });
    this.webhookUrl = webhookUrl;
  }

  public async auditSlot(slotNumber: number): Promise<BlockAuditReport | null> {
    try {
      const block: VersionedBlockResponse | null = await this.connection.getBlock(slotNumber, {
        maxSupportedTransactionVersion: 0,
        rewards: false,
        transactionDetails: "full",
      });

      if (!block) {
        console.warn(`[!] Slot ${slotNumber} skipped or not found.`);
        return null;
      }

      const accountMap: Map<string, AccountContentionMetrics> = new Map();
      let totalComputeUnits = 0;
      let totalPriorityFeesLamports = 0;
      const priorityFeeDistribution: number[] = [];

      for (const tx of block.transactions) {
        // Approximate compute units based on compute budget instruction parsing or standard allowance
        const cuLimit = 200_000; 
        totalComputeUnits += cuLimit;

        // Parse meta and fee
        const fee = tx.meta?.fee || 5000;
        const priorityFee = Math.max(0, fee - 5000);
        totalPriorityFeesLamports += priorityFee;
        priorityFeeDistribution.push(priorityFee);

        // Account lock contention parsing
        const message = tx.transaction.message;
        const staticAccountKeys = message.getAccountKeys();

        for (let i = 0; i < staticAccountKeys.length; i++) {
          const key = staticAccountKeys.get(i)!.toBase58();
          const isWritable = message.isAccountWritable(i);

          const current = accountMap.get(key) || {
            pubkey: key,
            totalCuRequested: 0,
            writeHitCount: 0,
            readHitCount: 0,
          };

          current.totalCuRequested += cuLimit;
          if (isWritable) {
            current.writeHitCount += 1;
          } else {
            current.readHitCount += 1;
          }
          accountMap.set(key, current);
        }
      }

      const sortedContention = Array.from(accountMap.values())
        .sort((a, b) => b.writeHitCount - a.writeHitCount)
        .slice(0, 5);

      priorityFeeDistribution.sort((a, b) => b - a);

      const report: BlockAuditReport = {
        slot: slotNumber,
        blockhash: block.blockhash,
        totalTransactions: block.transactions.length,
        totalComputeUnits,
        totalPriorityFeesLamports,
        highestContentionAccounts: sortedContention,
        topPriorityFees: priorityFeeDistribution.slice(0, 5),
      };

      await this.dispatchEnterpriseAlert(report);
      return report;
    } catch (err: any) {
      console.error(`[-] Failed to audit slot ${slotNumber}:`, err.message);
      return null;
    }
  }

  private async dispatchEnterpriseAlert(report: BlockAuditReport): Promise<void> {
    if (!this.webhookUrl) return;

    const payload = {
      text: `🚨 *Solana Validator Block Analysis: Slot ${report.slot}* 🚨`,
      blocks: [
        {
          type: "header",
          text: {
            type: "plain_text",
            text: `Block Audit: Slot ${report.slot}`,
            emoji: true,
          },
        },
        {
          type: "section",
          fields: [
            {
              type: "mrkdwn",
              text: `*Total Txs:* ${report.totalTransactions}`,
            },
            {
              type: "mrkdwn",
              text: `*Est Compute Units:* ${(report.totalComputeUnits / 1e6).toFixed(2)}M CU`,
            },
            {
              type: "mrkdwn",
              text: `*Priority Fees Collected:* ${(report.totalPriorityFeesLamports / 1e9).toFixed(4)} SOL`,
            },
            {
              type: "mrkdwn",
              text: `*Blockhash:* \`${report.blockhash.slice(0, 12)}...\``,
            },
          ],
        },
        {
          type: "section",
          text: {
            type: "mrkdwn",
            text: `*Hotspot Write Contention (Max Sequential Stalls):*\n${report.highestContentionAccounts
              .map(
                (a, idx) =>
                  `• #${idx + 1} \`${a.pubkey}\`: *${a.writeHitCount}* writes, ${(a.totalCuRequested / 1e6).toFixed(2)}M CU`
              )
              .join("\n")}`,
          },
        },
      ],
    };

    try {
      await axios.post(this.webhookUrl, payload, { timeout: 5000 });
      console.log(`[+] Dispatched audit report for slot ${report.slot} to external webhook.`);
    } catch (e: any) {
      console.warn(`[!] Webhook dispatch failed: ${e.message}`);
    }
  }

  public async runLiveStream(): Promise<void> {
    console.log("[*] Starting Solana Block Ordering Telemetry Monitor...");
    let lastSlot = await this.connection.getSlot("finalized");

    setInterval(async () => {
      try {
        const currentSlot = await this.connection.getSlot("finalized");
        if (currentSlot > lastSlot) {
          for (let s = lastSlot + 1; s <= currentSlot; s++) {
            await this.auditSlot(s);
          }
          lastSlot = currentSlot;
        }
      } catch (err: any) {
        console.error("[!] Polling error:", err.message);
      }
    }, 2000);
  }
}

// Direct Execution Entrypoint
if (require.main === module) {
  const RPC_URL = process.env.SOLANA_RPC_URL || "https://api.mainnet-beta.solana.com";
  const WEBHOOK = process.env.DISCORD_SLACK_WEBHOOK_URL;
  const auditor = new SolanaBlockTelemetryAuditor(RPC_URL, WEBHOOK);
  auditor.runLiveStream();
}