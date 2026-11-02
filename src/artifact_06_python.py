#!/usr/bin/env python3
"""
Enterprise Solana Validator Block Ordering Auditor
Inspects live blocks, calculates scheduler efficiency, and dispatches incident alerts.
"""

import os
import sys
import json
import time
import requests
from dataclasses import dataclass
from typing import List, Dict, Any, Optional

SOLANA_RPC_URL = os.getenv("SOLANA_RPC_URL", "https://api.mainnet-beta.solana.com")
SLACK_WEBHOOK_URL = os.getenv("SLACK_WEBHOOK_URL", "")
DISCORD_WEBHOOK_URL = os.getenv("DISCORD_WEBHOOK_URL", "")
HOT_ACCOUNT_THRESHOLD_CU = 10_000_000  # 10M CU warning threshold (cap is 12M)

@dataclass
class TransactionMetrics:
    signature: str
    compute_units: int
    priority_fee_micro_lamports: int
    writable_accounts: List[str]
    order_in_block: int

class BlockAuditor:
    def __init__(self, rpc_url: str):
        self.rpc_url = rpc_url
        self.session = requests.Session()
        self.session.headers.update({"Content-Type": "application/json"})

    def get_latest_slot(self) -> int:
        payload = {"jsonrpc": "2.0", "id": 1, "method": "getSlot", "params": [{"commitment": "confirmed"}]}
        res = self.session.post(self.rpc_url, json=payload).json()
        return res["result"]

    def get_block(self, slot: int) -> Optional[Dict[str, Any]]:
        payload = {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getBlock",
            "params": [
                slot,
                {
                    "encoding": "json",
                    "transactionDetails": "full",
                    "maxSupportedTransactionVersion": 0,
                    "rewards": False
                }
            ]
        }
        res = self.session.post(self.rpc_url, json=payload).json()
        return res.get("result")

    def audit_block(self, slot: int):
        block = self.get_block(slot)
        if not block:
            print(f"[-] Slot {slot} skipped or skipped by leader.")
            return

        transactions = block.get("transactions", [])
        total_txs = len(transactions)
        account_cu_counter: Dict[str, int] = {}
        parsed_txs: List[TransactionMetrics] = []

        for idx, tx_data in enumerate(transactions):
            meta = tx_data.get("meta") or {}
            cu_consumed = meta.get("computeUnitsConsumed", 200_000)
            tx = tx_data.get("transaction", {})
            message = tx.get("message", {})
            account_keys = message.get("accountKeys", [])
            signatures = tx_data.get("transaction", {}).get("signatures", [])
            sig = signatures[0] if signatures else f"unknown_{idx}"

            # Aggregate per-account CU
            for acc in account_keys:
                acc_pubkey = acc if isinstance(acc, str) else acc.get("pubkey")
                account_cu_counter[acc_pubkey] = account_cu_counter.get(acc_pubkey, 0) + cu_consumed

            parsed_txs.append(TransactionMetrics(
                signature=sig,
                compute_units=cu_consumed,
                priority_fee_micro_lamports=0,
                writable_accounts=[acc if isinstance(acc, str) else acc.get("pubkey") for acc in account_keys],
                order_in_block=idx
            ))

        # Check for Hot Account Saturation (Pump.fun or Raydium pool saturation)
        hot_accounts = {k: v for k, v in account_cu_counter.items() if v >= HOT_ACCOUNT_THRESHOLD_CU}
        if hot_accounts:
            self.dispatch_alert(
                slot=slot,
                alert_type="HOT_ACCOUNT_SATURATION",
                details={
                    "total_transactions": total_txs,
                    "saturated_accounts": hot_accounts,
                    "max_cu_cap": 12_000_000
                }
            )

        print(f"[+] Slot {slot} Audited: {total_txs} txs processed. Hot accounts (>10M CU): {len(hot_accounts)}")

    def dispatch_alert(self, slot: int, alert_type: str, details: Dict[str, Any]):
        msg = (
            f"🚨 *Solana Validator Block Alert* 🚨\n"
            f"*Type:* `{alert_type}`\n"
            f"*Slot:* `{slot}`\n"
            f"