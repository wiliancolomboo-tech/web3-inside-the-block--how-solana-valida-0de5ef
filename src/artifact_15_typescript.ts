import axios from 'axios';
import { EnterpriseAlertPayload } from './types';

export class EnterpriseNotificationAdapter {
  private discordWebhookUrl?: string;
  private resendApiKey?: string;
  private alertEmailRecipient?: string;

  constructor() {
    this.discordWebhookUrl = process.env.DISCORD_WEBHOOK_URL;
    this.resendApiKey = process.env.RESEND_API_KEY;
    this.alertEmailRecipient = process.env.ALERT_EMAIL_RECIPIENT;
  }

  public async broadcastAlert(payload: EnterpriseAlertPayload): Promise<void> {
    console.log(`[ALERT BROADCAST] [${payload.severity}] ${payload.title}`);

    // 1. Dispatch Webhook (Slack / Discord format)
    if (this.discordWebhookUrl) {
      try {
        await axios.post(this.discordWebhookUrl, {
          username: "Solana Block Scheduler Monitor",
          embeds: [
            {
              title: `[${payload.severity}] ${payload.title}`,
              description: payload.message,
              color: payload.severity === "CRITICAL" ? 15158332 : 3447003,
              fields: [
                { name: "Slot", value: payload.slot.toString(), inline: true },
                { name: "Transactions", value: payload.metrics.txCount.toString(), inline: true },
                { name: "Avg Fee (ul/CU)", value: payload.metrics.avgCuPrice.toFixed(2), inline: true },
                { name: "Hot Accounts", value: payload.hotAccounts.join("\n") || "None" }
              ],
              timestamp: new Date().toISOString()
            }
          ]
        });
      } catch (err: any) {
        console.error("Failed to transmit Webhook alert:", err.message);
      }
    }

    // 2. Dispatch Email via Resend API
    if (this.resendApiKey && this.alertEmailRecipient && payload.severity === "CRITICAL") {
      try {
        await axios.post(
          "https://api.resend.com/emails",
          {
            from: "validator-telemetry@enterprise.solana",
            to: this.alertEmailRecipient,
            subject: `CRITICAL: Solana Contention Anomaly at Slot ${payload.slot}`,
            html: `
              <h2>Solana Validator Block Scheduler Alert</h2>
              <p><strong>Severity:</strong> ${payload.severity}</p>
              <p><strong>Description:</strong> ${payload.message}</p>
              <ul>
                <li><strong>Slot:</strong> ${payload.slot}</li>
                <li><strong>Tx Count:</strong> ${payload.metrics.txCount}</li>
                <li><strong>Average CU Price:</strong> ${payload.metrics.avgCuPrice} micro-lamports</li>
                <li><strong>Contended Accounts:</strong> ${payload.hotAccounts.join(", ")}</li>
              </ul>
            `
          },
          {
            headers: {
              Authorization: `Bearer ${this.resendApiKey}`,
              "Content-Type": "application/json"
            }
          }
        );
      } catch (err: any) {
        console.error("Failed to transmit Resend email notification:", err.message);
      }
    }
  }
}