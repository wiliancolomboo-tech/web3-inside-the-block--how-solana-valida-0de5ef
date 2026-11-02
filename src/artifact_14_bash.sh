# 1. Clone repository & test the Rust banking stage scheduler
git clone https://github.com/enterprise-solana/solana-block-ordering-engine.git
cd solana-block-ordering-engine
cargo check --release
cargo test -- --nocapture

# 2. Build TypeScript telemetry auditor CLI
cd ../solana-block-telemetry-cli
npm install
npm run build

# 3. Launch live telemetry daemon
export SOLANA_RPC_URL="https://api.mainnet-beta.solana.com"
export DISCORD_SLACK_WEBHOOK_URL="https://api.example.com/webhooks/alerts/placeholder"
npm start