# 1. Clone repository and verify tests
git clone https://github.com/sponsor-org/solana-block-engine-inspector.git
cd solana-block-engine-inspector
cargo test --workspace

# 2. Build and launch container stack
docker compose up -d --build

# 3. Stream real-time block engine ordering logs
docker compose logs -f sbei-inspector