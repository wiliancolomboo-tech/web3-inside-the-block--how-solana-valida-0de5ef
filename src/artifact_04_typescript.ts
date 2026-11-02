import { Connection, PublicKey } from "@solana/web3.js";
import axios from "axios";
import http from "http";
import dotenv from "dotenv";

dotenv.config();

interface ValidatorMetrics {
  currentSlot: number;
  totalTransactions: number;
  totalComputeUnits: number;
  saturatedAccounts: Array<{ pubkey: string; cuUsage: number; saturationPct: number }>;
  jitoTipEstimatesSol: number;
  timestamp: string;
}

const RPC_HTTP = process.env.SOLANA_RPC_HTTP || "https://api.mainnet-beta.solana.com";
const RPC_WS = process.env.SOLANA_RPC_WS || "wss://api.mainnet-beta.solana.com";
const WEBHOOK_URL = process.env.ENTERPRISE_WEBHOOK_URL || "";
const PORT = parseInt(process.env.METRICS_PORT || "9100", 10);
const ACCOUNT_CU_CAP = 12_000_000;
const BLOCK_CU_CAP = 48_000_000;

class SolanaBlockTelemetryDaemon {
  private connection: Connection;
  private latestMetrics: ValidatorMetrics | null = null;

  constructor() {
    this.connection = new Connection(RPC_HTTP, {
      wsEndpoint: RPC_WS,
      commitment: "confirmed",
    });
  }

  public async start(): Promise<void> {
    console.log(`[DAEMON] Initializing Solana Block Ingress Observer on ${RPC_HTTP}`);
    this.startPrometheusServer();

    this.connection.onSlotUpdate(async (slotUpdate) => {
      if (slotUpdate.type === "createdBank") {
        await this.inspectSlot(slotUpdate.slot);
      }
    });

    console.log("[DAEMON] Telemetry daemon running and awaiting slot notifications...");
  }

  private async inspectSlot(slot: number): Promise<void> {
    try {
      const block = await this.connection.getBlock(slot, {
        maxSupportedTransactionVersion: 0,
        rewards: false,
        transactionDetails: "full",
      });

      if (!block || !block.transactions) return;

      let blockCu = 0;
      const accountCuMap: Map<string, number> = new Map();

      for (const tx of block.transactions) {
        const consumed = tx.meta?.computeUnitsConsumed || 200_000;
        blockCu += consumed;

        const keys = tx.transaction.message.getAccountKeys();
        for (let i = 0; i < keys.length; i++) {
          const isWritable = tx.transaction.message.isAccountWritable(i);
          if (isWritable) {
            const pk = keys.get(i)!.toBase58();
            accountCuMap.set(pk, (accountCuMap.get(pk) || 0) + consumed);
          }
        }
      }

      const saturated: Array<{ pubkey: string; cuUsage: number; saturationPct: number }> = [];
      for (const [account, cu] of accountCuMap.entries()) {
        const pct = (cu / ACCOUNT_CU_CAP) * 100;
        if (pct >= 75.0) {
          saturated.push({ pubkey: account, cuUsage: cu, saturationPct: pct });
        }
      }

      this.latestMetrics = {
        currentSlot: slot,
        totalTransactions: block.transactions.length,
        totalComputeUnits: blockCu,
        saturatedAccounts: saturated,
        jitoTipEstimatesSol: 0.05, // Baseline estimation
        timestamp: new Date().toISOString(),
      };

      if (saturated.length > 0) {
        await this.dispatchAlert(this.latestMetrics);
      }
    } catch (err: any) {
      console.error(`[ERROR] Slot ${slot} analysis error: ${err.message}`);
    }
  }

  private async dispatchAlert(metrics: ValidatorMetrics): Promise<void> {
    if (!WEBHOOK_URL) return;

    const payload = {
      text: `🚨 *High Account Write-Lock Saturation Detected!*`,
      blocks: [
        {
          type: "header",
          text: {
            type: "plain_text",
            text: `⚠️ Write-Lock Contention Alert: Slot ${metrics.currentSlot}`,
          },
        },
        {
          type: "section",
          fields: [
            { type: "mrkdwn", text: `*Block CU:* ${metrics.totalComputeUnits.toLocaleString()} / 48M` },
            { type: "mrkdwn", text: `*Total Txs:* ${metrics.totalTransactions}` },
            {
              type: "mrkdwn",
              text: `*Saturated Accounts:* ${metrics.saturatedAccounts.length}`,
            },
          ],
        },
        {
          type: "section",
          text: {
            type: "mrkdwn",
            text: metrics.saturatedAccounts
              .map(
                (a) =>
                  `• \`${a.pubkey}\`: *${a.saturationPct.toFixed(1)}%* (${a.cuUsage.toLocaleString()} CU)`
              )
              .join("\n"),
          },
        },
      ],
    };

    try {
      await axios.post(WEBHOOK_URL, payload, { timeout: 3000 });
    } catch (e: any) {
      console.error(`[ALERT] Webhook delivery failed: ${e.message}`);
    }
  }

  private startPrometheusServer(): void {
    const server = http.createServer((req, res) => {
      if (req.url === "/metrics") {
        res.writeHead(200, { "Content-Type": "text/plain; version=0.0.4" });
        if (!this.latestMetrics) {
          res.end("# No metrics recorded yet\n");
          return;
        }

        const m = this.latestMetrics;
        const out = [
          `# HELP solana_block_cu Total Compute Units consumed in block`,
          `# TYPE solana_block_cu gauge`,
          `solana_block_cu{slot="${m.currentSlot}"} ${m.totalComputeUnits}`,
          `# HELP solana_block_txs Total transactions in block`,
          `# TYPE solana_block_txs gauge`,
          `solana_block_txs{slot="${m.currentSlot}"} ${m.totalTransactions}`,
          `# HELP solana_saturated_accounts Number of accounts above 75% capacity`,
          `# TYPE solana_saturated_accounts gauge`,
          `solana_saturated_accounts{slot="${m.currentSlot}"} ${m.saturatedAccounts.length}`,
        ].join("\n");

        res.end(out + "\n");
      } else if (req.url === "/healthz") {
        res.writeHead(200, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ status: "healthy", slot: this.latestMetrics?.currentSlot ?? 0 }));
      } else {
        res.writeHead(404);
        res.end();
      }
    });

    server.listen(PORT, () => {
      console.log(`[PROMETHEUS] Exporter live on http://0.0.0.0:${PORT}/metrics`);
    });
  }
}

if (require.main === module) {
  const daemon = new SolanaBlockTelemetryDaemon();
  daemon.start().catch((err) => {
    console.error(`[FATAL] Daemon exited: ${err}`);
    process.exit(1);
  });
}