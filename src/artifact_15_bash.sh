# 1. Clone repository
git clone https://github.com/sponsor-org/solana-block-ordering-engine.git
cd solana-block-ordering-engine

# 2. Compile native release binary
cargo build --release

# 3. Execute dry-run deterministic simulation
./target/release/sbei simulate --threads 4

# 4. Run with real Slack enterprise alerting
./target/release/sbei simulate --threads 8 --webhook-url "https://api.example.com/webhooks/alerts/placeholder"

# 5. Launch turnkey Docker daemon
docker-compose up -d --build

# 6. Query healthcheck endpoint
curl -s http://localhost:8080/health | jq .