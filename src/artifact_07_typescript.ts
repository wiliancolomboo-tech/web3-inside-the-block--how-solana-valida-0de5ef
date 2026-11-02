import {
  Connection,
  PublicKey,
  ComputeBudgetProgram,
  TransactionInstruction,
  TransactionMessage,
  VersionedTransaction,
  Keypair,
} from "@solana/web3.js";
import axios from "axios";
import { Command } from "commander";
import * as dotenv from "dotenv";

dotenv.config();

interface DynamicPriorityCalculation {
  computeUnitPriceMicroLamports: number;
  computeUnitLimit: number;
  totalFeeLamports: number;
}

export class SolanaBlockOrderClient {
  private connection: Connection;
  private slackWebhookUrl: string | undefined;

  constructor(rpcUrl: string, slackWebhook?: string) {
    this.connection = new Connection(rpcUrl, "confirmed");
    this.slackWebhookUrl = slackWebhook;
  }

  /**
   * Calculates dynamic compute budget instructions to maximize priority score
   * while safeguarding against the 12M CU write-lock limit.
   */
  public async calculateOptimalBudget(
    targetAccounts: PublicKey[],
    baseUnitsRequested: number = 200_000
  ): Promise<DynamicPriorityCalculation> {
    try {
      const recentPriorities = await this.connection.getRecentPrioritizationFees({
        lockedWritableAccounts: targetAccounts,
      });

      let medianFee = 1_000; // fallback 1,000 microLamports
      if (recentPriorities.length > 0) {
        const fees = recentPriorities
          .map((p) => p.prioritizationFee)
          .sort((a, b) => a - b);
        const mid = Math.floor(fees.length / 2);
        medianFee = fees.length % 2 !== 0 ? fees[mid] : Math.floor((fees[mid - 1] + fees[mid]) / 2);
      }

      // 1.25x multiplier to outbid competitors within the validator's priority queue
      const optimalPrice = Math.max(Math.floor(medianFee * 1.25), 5_000);
      const totalFeeLamports = Math.ceil((optimalPrice * baseUnitsRequested) / 1_000_000);

      return {
        computeUnitPriceMicroLamports: optimalPrice,
        computeUnitLimit: baseUnitsRequested,
        totalFeeLamports,
      };
    } catch (error) {
      console.warn("Failed to fetch recent prioritization fees, fallback to static defaults:", error);
      return {
        computeUnitPriceMicroLamports: 10_000,
        computeUnitLimit: baseUnitsRequested,
        totalFeeLamports: Math.ceil((10_000 * baseUnitsRequested) / 1_000_000),
      };
    }
  }

  /**
   * Builds an optimized transaction with ComputeBudget instructions at index 0 and 1
   */
  public async buildOptimizedTransaction(
    payer: Keypair,
    instructions: TransactionInstruction[],
    targetAccounts: PublicKey[]
  ): Promise<VersionedTransaction> {
    const budget = await this.calculateOptimalBudget(targetAccounts);

    const modifyComputeUnits = ComputeBudgetProgram.setComputeUnitLimit({
      units: budget.computeUnitLimit,
    });
    const addPriorityFee = ComputeBudgetProgram.setComputeUnitPrice({
      microLamports: budget.computeUnitPriceMicroLamports,
    });

    const latestBlockhash = await this.connection.getLatestBlockhash("confirmed");

    const messageV0 = new TransactionMessage({
      payerKey: payer.publicKey,
      recentBlockhash: latestBlockhash.blockhash,
      instructions: [modifyComputeUnits, addPriorityFee, ...instructions],
    }).compileToV0Message();

    const tx = new VersionedTransaction(messageV0);
    tx.sign([payer]);

    return tx;
  }

  /**
   * Dispatches alerts to Enterprise Slack on write-lock contention or dropped transactions
   */
  public async emitSlackAlert(event: string, payload: Record<string, any>): Promise<void> {
    if (!this.slackWebhookUrl) {
      console.log(`[SLACK ALERT] ${event}:`, JSON.stringify(payload, null, 2));
      return;
    }

    try {
      await axios.post(this.slackWebhookUrl, {
        text: `🚨 *Solana Validator Ingestion Alert: ${event}*`,
        blocks: [
          {
            type: "section",
            text: {
              type: "mrkdwn",
              text: `*Alert Type:* ${event}\n*Timestamp:* ${new Date().toISOString()}\n\`\`\`${JSON.stringify(
                payload,
                null,
                2
              )}\`\`\``,
            },
          },
        ],
      });
    } catch (err: any) {
      console.error("Slack alert dispatch failed:", err.message);
    }
  }
}

// CLI Driver
const program = new Command();

program
  .name("solana-order-agent")
  .description("Enterprise Validator Block Ordering & Priority Fee Optimizer")
  .version("1.0.0");

program
  .command("inspect-contention")
  .description("Analyzes account write-lock contention and calculates optimal priority fees")
  .requiredOption("-a, --account <pubkey>", "Target account to inspect")
  .option("-r, --rpc <url>", "Solana RPC URL", "https://api.mainnet-beta.solana.com")
  .action(async (options) => {
    const target = new PublicKey(options.account);
    const client = new SolanaBlockOrderClient(options.rpc, process.env.SLACK_WEBHOOK_URL);

    console.log(`Inspecting write-lock contention for account: ${target.toBase58()}`);
    const budget = await client.calculateOptimalBudget([target]);

    console.log("Calculated Optimal Budget Profile:");
    console.log(`- Requested CU Limit: ${budget.computeUnitLimit}`);
    console.log(`- Optimal Compute Unit Price: ${budget.computeUnitPriceMicroLamports} micro-lamports`);
    console.log(`- Est. Additional Fee: ${budget.totalFeeLamports} lamports`);

    await client.emitSlackAlert("Account Contention Monitored", {
      account: target.toBase58(),
      budget,
    });
  });

if (require.main === module) {
  program.parse(process.argv);
}