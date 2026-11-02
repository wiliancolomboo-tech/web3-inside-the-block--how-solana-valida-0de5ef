import { Connection, PublicKey } from "@solana/web3.js";
import axios from "axios";

export interface AccountContentionMetric {
  account: string;
  totalCuConsumed: number;
  utilizationPercent: number;
  contentionEvents: number;
}

export interface BlockAnalysisReport {
  slot: number;
  blockhash: string;
  transactionCount: number;
  totalComputeUnits: number;
  blockCapacityPercent: number;
  hotAccounts: AccountContentionMetric[];
  priorityFeeP50: number;
  priorityFeeP90: number;
  bundleCount: number;
}

export class SolanaBlockOrderInspector {
  private connection: Connection;
  private discordWebhookUrl?: string;
  private readonly MAX_ACCOUNT_CU = 12_000_000;
  private readonly MAX_BLOCK_CU = 48_000_000;

  constructor(rpcEndpoint: string, discordWebhookUrl?: string) {
    this.connection = new Connection(rpcEndpoint, "confirmed");
    this.discordWebhookUrl = discordWebhookUrl;
  }

  public async analyzeSlot(slot: number): Promise<BlockAnalysisReport | null> {
    try {
      const block = await this.connection.getBlock(slot, {
        maxSupportedTransactionVersion: 0,
        transactionDetails: "full",
      });

      if (!block || !block.transactions) {
        console.warn(`[WARN] Slot ${slot} yielded no block data or was skipped.`);
        return null;
      }

      let totalBlockCu = 0;
      const accountCuMap: Map<string, { cu: number; hits: number }> = new Map();
      const priorityPrices: number[] = [];
      let detectedBundles = 0;

      for (const tx of block.transactions) {
        // Extract compute units from metadata or default estimates
        const cuConsumed = tx.meta?.computeUnitsConsumed ?? 200_000;
        totalBlockCu += cuConsumed;

        const message = tx.transaction.message;
        const accountKeys = message.getAccountKeys();

        // Extract priority fees from logs / instruction analysis
        let priorityPrice = 0;
        for (const ix of message.compiledInstructions) {
          const programId = accountKeys.get(ix.programIdIndex);
          if (
            programId &&
            programId.toBase58() === "ComputeBudget111111111111111111111111111111"
          ) {
            // Byte 0 = type (3 = SetComputeUnitPrice)
            if (ix.data.length >= 9 && ix.data[0] === 3) {
              const view = new DataView(
                ix.data.buffer,
                ix.data.byteOffset,
                ix.data.byteLength
              );
              priorityPrice = Number(view.getBigUint64(1, true));
            }
          }
        }
        priorityPrices.push(priorityPrice);

        // Account contention profiling
        for (let idx = 0; idx < accountKeys.length; idx++) {
          const isWritable = message.isAccountWritable(idx);
          if (isWritable) {
            const pubkeyStr = accountKeys.get(idx)!.toBase58();
            const curr = accountCuMap.get(pubkeyStr) || { cu: 0, hits: 0 };
            curr.cu += cuConsumed;
            curr.hits += 1;
            accountCuMap.set(pubkeyStr, curr);
          }
        }

        // Detect tip accounts (e.g. Jito tip payment recipients)
        const JITO_TIP_ACCOUNTS = new Set([
          "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5",
          "HFqU5x63VTqvQss8hp11i4wVV8bD44PvwucfZ2bU7gRe",
          "Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY",
          "ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt6iGPaS49",
          "DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh",
          "ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt",
          "DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL",
          "3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT",
        ]);

        for (let idx = 0; idx < accountKeys.length; idx++) {
          if (JITO_TIP_ACCOUNTS.has(accountKeys.get(idx)!.toBase58())) {
            detectedBundles++;
            break;
          }
        }
      }

      priorityPrices.sort((a, b) => a - b);
      const p50 = priorityPrices[Math.floor(priorityPrices.length * 0.5)] || 0;
      const p90 = priorityPrices[Math.floor(priorityPrices.length * 0.9)] || 0;

      const hotAccounts: AccountContentionMetric[] = [];
      for (const [account, data] of accountCuMap.entries()) {
        if (data.cu > this.MAX_ACCOUNT_CU * 0.3) {
          // Flag if exceeding 30% of account limit
          hotAccounts.push({
            account,
            totalCuConsumed: data.cu,
            utilizationPercent: (data.cu / this.MAX_ACCOUNT_CU) * 100,
            contentionEvents: data.hits,
          });
        }
      }

      hotAccounts.sort((a, b) => b.totalCuConsumed - a.totalCuConsumed);

      const report: BlockAnalysisReport = {
        slot,
        blockhash: block.blockhash,
        transactionCount: block.transactions.length,
        totalComputeUnits: totalBlockCu,
        blockCapacityPercent: (totalBlockCu / this.MAX_BLOCK_CU) * 100,
        hotAccounts,
        priorityFeeP50: p50,
        priorityFeeP90: p90,
        bundleCount: detectedBundles,
      };

      if (report.hotAccounts.length > 0 || report.blockCapacityPercent > 85) {
        await this.dispatchEnterpriseAlert(report);
      }

      return report;
    } catch (error) {
      console.error(`[ERROR] Failed to analyze slot ${slot}:`, error);
      return null;
    }
  }

  private async dispatchEnterpriseAlert(report: BlockAnalysisReport): Promise<void> {
    if (!this.discordWebhookUrl) return;

    const topContested = report.hotAccounts[0];
    const payload = {
      username: "Solana BankingStage Telemetry Guard",
      avatar_url: "https://solana.com/src/img/branding/solanaLogoMark.png",
      embeds: [
        {
          title: `⚠️ Slot ${report.slot} Contention & Saturation Alert`,
          color: 0xff4500,
          fields: [
            {
              name: "Block Capacity",
              value: `${report.totalComputeUnits.toLocaleString()} CU (${report.blockCapacityPercent.toFixed(1)}%)`,
              inline: true,
            },
            {
              name: "P90 Priority Fee",
              value: `${report.priorityFeeP90.toLocaleString()} μLamports/CU`,
              inline: true,
            },
            {
              name: "Detected MEV Bundles",
              value: `${report.bundleCount}`,
              inline: true,
            },
            {
              name: "Top Hot Account (Write Contention)",
              value: topContested
                ? `\`${topContested.account}\`\nConsumed: ${topContested.totalCuConsumed.toLocaleString()} CU (${topContested.utilizationPercent.toFixed(1)}% Cap)`
                : "None",
              inline: false,
            },
          ],
          footer: {
            text: "Deterministic Validator Scheduler Operations",
          },
          timestamp: new Date().toISOString(),
        },
      ],
    };

    try {
      await axios.post(this.discordWebhookUrl, payload, { timeout: 3000 });
      console.log(`[INFO] Telemetry alert successfully dispatched for slot ${report.slot}`);
    } catch (postError) {
      console.error("[ERROR] Failed to post alert to webhook:", postError);
    }
  }
}

// Interactive CLI Runner
if (require.main === module) {
  const rpc = process.env.SOLANA_RPC_URL || "https://api.mainnet-beta.solana.com";
  const webhook = process.env.DISCORD_WEBHOOK_URL;
  const inspector = new SolanaBlockOrderInspector(rpc, webhook);

  console.log(`[*] Solana BankingStage Block Inspector Initialized on ${rpc}`);
  (async () => {
    const conn = new Connection(rpc, "confirmed");
    const currentSlot = await conn.getSlot("confirmed");
    console.log(`[*] Inspecting block at slot: ${currentSlot}`);
    const report = await inspector.analyzeSlot(currentSlot);
    console.log("[+] Inspection Output:", JSON.stringify(report, null, 2));
  })();
}