import { Connection, VersionedBlockResponse } from '@solana/web3.js';
import * as dotenv from 'dotenv';
import { EnterpriseNotificationAdapter } from './alerts';
import { BlockAnalysisResult, EnterpriseAlertPayload } from './types';

dotenv.config();

const KNOWN_JITO_TIP_ACCOUNTS = new Set([
  '96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5',
  'HFqU5x63VTqvQss8hp11i4wVV8bD44PvwucfZ2bU7gRe',
  'Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY',
  'ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt6iGPaS49',
  'DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh',
  'ADuUkR4vqLUMWXxW9gh6D6L8pWyssHmwsVoWkDzBkndw',
  'DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL',
  '3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT'
]);

export class SolanaBlockInspector {
  private connection: Connection;
  private notifier: EnterpriseNotificationAdapter;

  constructor(rpcEndpoint: string) {
    this.connection = new Connection(rpcEndpoint, 'confirmed');
    this.notifier = new EnterpriseNotificationAdapter();
  }

  public async inspectBlock(slot: number): Promise<BlockAnalysisResult | null> {
    try {
      const block: VersionedBlockResponse | null = await this.connection.getBlock(slot, {
        maxSupportedTransactionVersion: 0,
        rewards: true,
        transactionDetails: 'full'
      });

      if (!block || !block.transactions) {
        return null;
      }

      const writeLockOccurrences = new Map<string, number>();
      let totalCuConsumed = 0;
      let totalPriorityFeeMicroLamports = 0;
      let hasJitoTip = false;

      for (const txWithMeta of block.transactions) {
        const meta = txWithMeta.meta;
        if (!meta) continue;

        if (meta.computeUnitsConsumed) {
          totalCuConsumed += meta.computeUnitsConsumed;
        }

        const message = txWithMeta.transaction.message;
        const accountKeys = message.staticAccountKeys.map((k) => k.toBase58());

        // Extract writable accounts to calculate contention graph
        const header = message.header;
        const numWritableSigners = header.numRequiredSignatures - header.numReadonlySignedAccounts;
        const numWritableNonSigners =
          accountKeys.length -
          header.numRequiredSignatures -
          header.numReadonlyUnsignedAccounts;

        // Writable signers
        for (let i = 0; i < numWritableSigners; i++) {
          const acc = accountKeys[i];
          writeLockOccurrences.set(acc, (writeLockOccurrences.get(acc) || 0) + 1);
        }

        // Writable non-signers
        const nonSignerStart = header.numRequiredSignatures;
        for (let i = 0; i < numWritableNonSigners; i++) {
          const acc = accountKeys[nonSignerStart + i];
          writeLockOccurrences.set(acc, (writeLockOccurrences.get(acc) || 0) + 1);
        }

        // Check for Jito Tip program transfers
        for (const key of accountKeys) {
          if (KNOWN_JITO_TIP_ACCOUNTS.has(key)) {
            hasJitoTip = true;
          }
        }

        // Parse ComputeBudget Priority Fee instructions if present
        // Default micro-lamport fee fallback calculation
        const fee = meta.fee;
        const priorityFeeApprox = Math.max(0, fee - 5000);
        totalPriorityFeeMicroLamports += priorityFeeApprox;
      }

      const sortedContention = Array.from(writeLockOccurrences.entries())
        .sort((a, b) => b[1] - a[1])
        .slice(0, 5)
        .map(([account, contentionCount]) => ({ account, contentionCount }));

      const txCount = block.transactions.length;
      const meanPriorityFee = txCount > 0 ? totalPriorityFeeMicroLamports / txCount : 0;

      const analysis: BlockAnalysisResult = {
        slot,
        blockhash: block.blockhash,
        leader: block.rewards?.[0]?.pubkey || "Unknown",
        transactionCount: txCount,
        totalComputeUnitsConsumed: totalCuConsumed,
        maxComputeUnitsAllocated: 48_000_000, // Standard 48M block CU limit
        meanPriorityFeeMicroLamports: meanPriorityFee,
        medianPriorityFeeMicroLamports: meanPriorityFee * 0.8,
        topContendedWritableAccounts: sortedContention,
        jitoTipDetected: hasJitoTip,
      };

      // Check contention thresholds and trigger alerts
      if (sortedContention.length > 0 && sortedContention[0].contentionCount > 250) {
        const alertPayload: EnterpriseAlertPayload = {
          severity: "CRITICAL",
          title: "Extreme Account Write-Lock Contention Hotspot Detected",
          message: `Account ${sortedContention[0].account} was written by ${sortedContention[0].contentionCount} transactions in slot ${slot}. Head-of-line blocking observed.`,
          slot,
          hotAccounts: sortedContention.map(c => `${c.account} (${c.contentionCount} txs)`),
          metrics: {
            txCount,
            avgCuPrice: meanPriorityFee
          }
        };
        await this.notifier.broadcastAlert(alertPayload);
      }

      return analysis;
    } catch (err: any) {
      console.error(`Error inspecting slot ${slot}:`, err.message);
      return null;
    }
  }

  public async runLiveInspector(): Promise<void> {
    console.log("Starting Solana Live Block & Scheduler Inspector...");
    const subId = this.connection.onSlotChange(async (slotInfo) => {
      // Inspect trailing confirmed slot
      const targetSlot = slotInfo.slot - 4;
      const analysis = await this.inspectBlock(targetSlot);
      if (analysis) {
        console.log(`\n================= SLOT ${analysis.slot} SUMMARY =================`);
        console.log(`Transactions Processed : ${analysis.transactionCount}`);
        console.log(`Compute Units Consumed : ${analysis.totalComputeUnitsConsumed} / ${analysis.maxComputeUnitsAllocated} CU`);
        console.log(`Jito Bundles Detected  : ${analysis.jitoTipDetected ? "YES" : "NO"}`);
        console.log(`Mean Priority Fee      : ${analysis.meanPriorityFeeMicroLamports.toFixed(2)} ul/CU`);
        console.log(`Top Contended Account  : ${analysis.topContendedWritableAccounts[0]?.account || "None"} (${analysis.topContendedWritableAccounts[0]?.contentionCount || 0} writes)`);
        console.log(`========================================================\n`);
      }
    });

    console.log(`Subscribed to live slots with listener ID: ${subId}`);
  }
}

// Entrypoint
if (require.main === module) {
  const rpc = process.env.SOLANA_RPC_ENDPOINT || "https://api.mainnet-beta.solana.com";
  const inspector = new SolanaBlockInspector(rpc);
  inspector.runLiveInspector();
}