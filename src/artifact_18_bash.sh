# ====================================================================
# 1. Clone & Build the Core Rust Scheduler Engine
# ====================================================================
git clone https://github.com/sponsor-org/inside-the-block-solana.git
cd inside-the-block-solana/solana-block-engine

# Run full comprehensive test suite
cargo test --verbose

# Run release benchmark simulation
cargo run --release

# ====================================================================
# 2. Build & Launch the Real-Time Block Inspector Service
# ====================================================================
cd ../solana-block-inspector

npm install
npm run build

# Configure environment variables
cp .env.example .env
# Edit .env with your SOLANA_RPC_ENDPOINT, DISCORD_WEBHOOK_URL, and RESEND_API_KEY

# Launch live telemetry stream
npm run start